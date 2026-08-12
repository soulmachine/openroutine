# OpenRoutine v1 — full product spec

Status: implemented and released as **1.0.0**. Amended 2026-08-12 for the file-registration redesign that 1.0 ships: directory registration, scanning, and hot reload replaced by explicit per-file registration and explicit Reload; identity is an Id derived from the written `name`; a Run is bounded by silence rather than by total runtime. See `docs/adr/0002` and DECISIONS Q75–Q92. Withdrawn stories are kept in place so the numbering stays stable.

## Problem Statement

Developers now trust coding agents with recurring, unattended work — nightly dependency audits, TODO triage, docs-drift PRs — but the only schedulers for that work are the vendors' own. Claude Code Routines run in Anthropic's cloud, configured in a web UI tied to a claude.ai account, metered by subscription caps. ChatGPT's scheduled tasks come closer to home — project-scoped tasks in the macOS desktop app run against your local checkout — but they are created conversationally, stored in an OpenAI account, fire only while the GUI app is running, capped per plan, limited to hourly at most, and may auto-pause when left unattended. In both, the schedule cannot be `git diff`'d, code-reviewed, or moved to the other vendor's agent. A user who wants "my repo's recurring agent work, defined in my repo, run on my machine, by whatever agent I choose" has no tool.

## Solution

OpenRoutine: a single Rust binary whose daemon is the scheduler, the REST API, and the local web UI. A Task is one markdown file, registered with the daemon by path — frontmatter for metadata (name, description, schedule, agent, and per-task knobs), body as the prompt. One machine-global Daemon (see ADR-0001) holds an explicit registry of task files (see ADR-0002), schedules everything in-process with deterministic jitter, runs each Task through a user-configured agent command template (Claude Code, Codex, or anything with a CLI), and records Runs and Skips as plain files. Definitions are markdown the human owns; state is JSON the machine owns; everything rehydrates from disk at startup and on explicit Reload, and each Task re-reads its own file as it is about to Run — nothing watched, nothing polled.

The whole definition, in one file:

```markdown
---
name: todo-digest
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: claude
---

Review all open TODO and FIXME comments in this repository.
For any that are trivially fixable, fix them and open a pull
request. Summarize everything else in reports/todo-digest.md.
```

## The scheduler is the daemon — not launchd, not crontab

OpenRoutine deliberately does **not** reuse macOS launchd or Linux crontab. The daemon carries its own in-process scheduler, and the OS supervises exactly one process while scheduling nothing.

This is a considered trade, because launchd and cron are where agent jobs quietly go to die: they run tasks in a stripped environment with no `PATH`, shell profile, or API keys your agent CLI expects; their definitions live in per-OS dialects (plists on one machine, crontab lines on another); and they offer no first-class overlap control, timezone handling, or structured logging. Owning the scheduler buys identical behavior on macOS and Linux, tasks that run with a real environment — every run launches through your login shell, so `PATH`, profile, and API keys match your terminal, with per-task `env:` frontmatter and config `[env]` overrides layered on top — reload without a restart, and honest bookkeeping: runs missed while the daemon is down are recorded as skips in the state file rather than silently dropped.

The cost is equally plain: if the daemon isn't running, schedules don't fire. That's why `openroutine install` registers the daemon with your service manager — for process supervision only, and without sudo: a systemd user unit with lingering enabled on Linux, so it starts at boot with no login session, or a per-user LaunchAgent on macOS, which starts at login (pair it with auto-login on a headless machine). Either way the daemon runs headless and is restarted if it dies. Windows is an explicit non-goal for v1.

## Agent-agnostic by construction

OpenRoutine never talks to a model API. An "agent" is just a command template, so any CLI that accepts a prompt works — including your own:

```toml
[agents.claude]
cmd = "claude -p {prompt}"

[agents.codex]
cmd = "codex exec {prompt}"
```

Switching a task from one agent to another is a one-line frontmatter edit, visible in the diff like any other change. Per-task `model:` and `permission_mode:` fields travel the same way, injected through `{model}` and `{permission_mode}` template placeholders as single arguments — never free-form flags.

## Storage: markdown for definitions, JSON for state

OpenRoutine splits storage across two plain-text formats, with a strict rule about which owns what.

**The task definition is the markdown file — all of it.** Every metadata field (name, description, schedule, agent, `disabled`, and anything added later) lives in the frontmatter, and the prompt is the body. There is no second place to look: the file is the complete, self-contained definition, which is what makes tasks portable, diffable, and reviewable in a pull request.

**Run state goes to `scheduled-tasks.json`.** Things the machine learns by running — last run time, the tick it was scheduled for, recorded skips, pause toggles — are not part of the definition and don't belong in git. OpenRoutine keeps them in a single JSON file outside the repo, keyed back to the task files by the Id derived from the frontmatter `name`, with the resolved working directory always spelled out:

```json
{
  "scheduledTasks": [
    {
      "id": "todo-digest",
      "filePath": "/home/you/repos/myrepo/tasks/todo-digest.md",
      "cwd": "/home/you/repos/myrepo/tasks",
      "lastRunAt": "2026-08-11T02:00:04.113Z",
      "lastScheduledFor": "2026-08-11T02:00:00.000Z"
    }
  ],
  "recordedSkips": {}
}
```

The state file is machine-owned and disposable: delete it and you lose run history, not tasks.

This is deliberately the same layout — and the same filename — that Claude Desktop uses internally for its local scheduled tasks (as of Claude.app 2.1.222): a `scheduled-tasks.json` for schedule and run state, per-task markdown files for the prompts, plain text end to end, no SQLite anywhere in the path. OpenRoutine diverges on one point: the cron expression lives in the markdown frontmatter rather than the JSON, so the definition never depends on the state file. The likeness is provenance, not an interop promise: OpenRoutine's schema evolves freely.

## REST API and local web UI

The same daemon that schedules also listens on localhost, adding the two things a plain scheduler can't give you: programmatic triggers and a place to look.

**The REST API** mirrors the shape of Claude Routines' API trigger, minus the cloud. Every task gets a fire endpoint, so alerting systems, deploy pipelines, and git hooks can start a run with an authenticated POST:

```bash
curl -X POST http://127.0.0.1:7373/v1/tasks/todo-digest/fire \
  -H "Authorization: Bearer $OPENROUTINE_TOKEN" \
  -d '{"text": "Sentry alert SEN-4521 fired in prod. Stack trace attached."}'
```

The optional `text` field is delivered to the agent alongside the task's saved prompt, wrapped and labeled as untrusted run-specific context rather than as instructions — the same discipline Routines applies to its fire payloads, and for the same reason: anyone who can reach the endpoint can send text, so text must not be able to redefine the task. The response returns a run id and the path to its log. The rest of the API is reads plus the operational controls: `GET /v1/tasks` lists every registered task with its schedule and next fire time, `GET /v1/tasks/<name>/runs` pages through history, an SSE endpoint live-tails any run's log, `POST /v1/reload` re-reads the config and every registered file, and pause, resume, and `POST /v1/runs/<name>/<run>/cancel` cover the runaway case.

**The web UI** is the local, single-user answer to claude.ai/code/routines and ChatGPT's Scheduled page: every task with its schedule and next fire time, per-task run history with full logs, and run-now, pause, and resume controls (per task, or a global pause for the whole daemon) — served from the daemon, viewable in any browser, no account anywhere.

The daemon binds to `127.0.0.1` by default and requires a bearer token generated on first `serve`, so a stray process on your machine can't fire your agents. A config override can widen the bind, but the docs discourage it loudly and the token stays mandatory either way. And it holds no private state: everything it serves is read from the registered task files, `scheduled-tasks.json`, and the run logs, and everything it writes goes back to those same files. Restart it and it rehydrates entirely from disk.

## Design principles

One binary, one process, zero databases. The daemon is the scheduler, the API, and the UI in a single static Rust binary with no runtime dependencies; the OS supervises exactly one process and schedules nothing. All durable data is two plain-text formats — markdown you own, JSON the machine owns — so everything that defines behavior is a file in git, and everything the tool generates is a file you can read. If OpenRoutine disappeared tomorrow, your tasks would still be legible markdown, ready for whatever runs them next.

## User Stories

1. As a developer, I want a Task to be a single markdown file with frontmatter and a prompt body, so that my scheduled agent work is diffable, greppable, and code-reviewable like any other file in my repo.
2. As a developer, I want crontab syntax (standard 5-field plus `@hourly`/`@daily`/`@weekly`/`@monthly`) in the frontmatter, so that everything I already know about cron applies.
3. As a developer, I want a required `description:` field surfaced everywhere tasks are listed, so that every schedule is glanceable without reading prompts.
4. As a developer, I want a one-shot Task via `at:` with an RFC 3339 timestamp, so that I can schedule a single future run ("migrate the DB Saturday 6am") the same way I schedule recurring work.
5. As a developer, I want a Task with no `cron:` and no `at:` to be a one-shot that runs once as soon as the daemon takes it in, so that "run this job once" is a file registration, not a fake schedule. *(Amended for 1.0: replaces the Manual Task — a completed one-shot still runs on demand via Fire.)*
6. As a developer, I want `disabled: true` in frontmatter, so that turning a task off is a reviewable commit, not invisible machine state.
7. As a developer, I want per-task `model:`, `permission_mode:`, `env:`, `cwd:`, and `jitter:` fields, so that one file fully describes how its run behaves.
8. As a developer, I want unknown frontmatter keys to warn but not break the Task, so that a task file written for a newer OpenRoutine doesn't brick an older daemon.
9. As a developer, I want a Broken Task to appear in every list with its exact parse error and a stopped schedule, so that a typo'd cron string can never make a task silently vanish — the launchd failure this tool exists to fix.
10. As a developer with several repos, I want one Daemon running task files registered individually from anywhere on the machine (`openroutine add <file.md>`), so that one port, one boot service, and one UI cover my whole machine. *(Amended for 1.0: was directory/Project registration.)*
11. As a developer, I want every task to carry a mandatory `name:` in its frontmatter, unique across the machine, so that identity is explicit in the file itself and survives the file being moved or renamed. *(Amended for 1.0: was `<project>/<stem>` ids derived from the filesystem.)*
12. As a developer, I want the daemon to read exactly the files I registered — no directory scanning, no filename convention, no gitignore rules — so that nothing can ever be scheduled that I didn't explicitly hand to the tool. *(Amended for 1.0: was recursive `.cron.md` discovery.)*
13. As a developer, I want definitions re-read at daemon startup and on explicit reload (`openroutine reload`, `POST /v1/reload`, and automatically after `add`/`remove`), with nothing watched or polled in between, so that the running schedule never changes behind my back. *(Amended for 1.0: was hot reload on file events.)*
13a. As a task author, I want a task to re-read its own file at the moment it is about to run, so that the prompt I just fixed is the one that runs — without a watcher, a timer, or a reload I have to remember. *(Added for 1.0, DECISIONS Q82.)*
14. As a security-conscious developer, I want newly registered or changed Tasks flagged loudly in the CLI, UI, and daemon log, so that an edit that arrived by `git pull` never goes unnoticed — registering a file means trusting whoever can edit it.
15. *(Withdrawn for 1.0: `CRONTAB.md` generation was dropped with directory registration; `openroutine list` answers "what runs here, and when?".)*
16. *(Withdrawn for 1.0 with story 15: the per-project `crontab_md` opt-out went with the feature.)*
17. As a user, I want `openroutine serve` to run the daemon in the foreground and `openroutine install` to register it — sudo-free — with systemd (user unit, lingering enabled) or launchd (per-user LaunchAgent), so that my tasks fire on a headless box without a GUI or root.
18. As a user, I want a second `serve` to refuse to start with a clear error, so that two daemons never race over one state file.
19. As a user whose machine was off, I want missed Ticks recorded as one collapsed Skip per outage window (`from`, `to`, count, reason `daemon-down`), so that a vacation shows as one honest entry, not thousands.
20. As a user, I want opt-in `catch_up: true` to fire one run for the most recent missed Tick (7-day lookback) at startup, so that a nightly digest still appears the morning after a power cut — but only when I asked for that.
21. As a user, I want a Tick that comes due while the previous Run is still going to become a Skip with reason `overlap`, so that a stuck 6-hour run never creates a backlog storm.
22. As a user, I want fire times offset by deterministic per-task jitter (id-derived, up to 5 minutes, capped at half the interval, `jitter: 0` to opt out), so that twenty midnight tasks don't stampede a vendor API at 00:00:00.
23. As a user, I want cron evaluated in my machine's local timezone with pinned DST rules (skipped wall-times fire at the next valid instant; repeated wall-times fire once), so that my 2am task behaves predictably in March and November.
24. As a user, I want every Run launched through my login shell with per-task `env:` and config `[env]` layered on top, so that agents find the same `PATH`, profile, and API keys they'd have in my terminal — even under a boot-started daemon.
25. As a user, I want the prompt handed to the agent as a single argument via the `{prompt}` placeholder (never spliced into a shell string) or piped to stdin when there's no placeholder, so that a prompt can never inject flags or shell syntax.
26. As a user, I want a run ended when its agent goes quiet for too long — SIGTERM to the run's process group, a 10-second grace, then SIGKILL, after a machine-wide 15 minutes of silence (config `idle_timeout`, no per-task override) — so that a hung agent and all its children die instead of running forever, while a long run that keeps producing output is never interrupted. *(Amended for 1.0, DECISIONS Q89: was a 1-hour cap on total runtime.)*
27. As a user, I want an optional `max_parallel` config cap where a blocked Tick waits for a slot (delayed, never skipped) and the Run records both scheduled and started times, so that I can bound agent concurrency without losing runs.
28. As an incident responder, I want a persisted global pause — `pause --all`, an API endpoint, a UI toggle, and an env var at serve start — so that I can stop all firing immediately while the daemon, API, and UI stay up for inspection.
29. As a user, I want the state file to be disposable: atomic writes, and a corrupt file renamed aside and regenerated with a loud warning, so that state damage costs me history, never tasks.
30. As a user, I want run retention capped (keep last 50 per task, configurable) and each `output.log` capped with a truncation marker, so that a chatty 15-minute task can't eat my disk.
31. As a new user, I want `openroutine init` to write my agent config and print a copy-pasteable sample task, so that the first five minutes require no documentation. *(Amended for 1.0: init writes config only; it registers nothing and writes no sample file to disk.)*
32. As a user, I want `list` and `logs` to read straight from disk, so that they work even when the daemon is stopped.
33. As a user, I want `openroutine run <task>` to fire through the API and explain a 409 (already running, with the active run id), so that operating a task is explicit and honest.
34. As a task author, I want `run --dry-run` to print the fully resolved plan — exact argv after placeholder substitution, delivery mode, cwd, env override keys, next jittered fire times — computed offline, executing nothing, so that I can verify a task before its first tick.
35. As a user, I want `logs <task> --follow` to tail the current run live, and `status` to summarize the daemon at a glance, so that day-two operations stay in the terminal.
36. As an integrator, I want every Task to have an authenticated REST fire endpoint, so that alerting systems, deploy pipelines, and git hooks can start runs with a POST.
37. As an integrator, I want an optional `text` payload (≤ 64 KB, 413 above) delivered inside a documented `<run-context>` wrapper labeled as untrusted, informational, non-instructional content, so that anyone who can reach the endpoint can inform a run but never redefine the task.
38. As an integrator, I want pause/resume per task, `cancel` on a running Run, an SSE live-tail of any run log, and paged run history over the API, so that everything the UI can do, a script can do.
39. As a security-conscious user, I want the daemon bound to 127.0.0.1 by default with a bearer token required on every endpoint — reads included, since task lists leak prompts — and the token stored 0600, printable and rotatable via `openroutine token`, so that a stray local process can't fire or read my agents.
40. As a user who accepts the risk, I want a config bind override that keeps the token mandatory and is loudly discouraged in docs, so that a tailnet setup is possible without OpenRoutine pretending it's safe.
41. As a user, I want API responses to distinguish null (unknown) from empty (none found), and `nextFireAt` to be genuinely null for Completed Tasks, so that clients never parse sentinel values.
42. As a user, I want a local web UI — every task with schedule and next fire, run history with full logs, run-now/pause/resume/cancel, the global pause toggle — with no account anywhere, so that I get the vendors' dashboard experience against my own machine.
43. As a UI user, I want `openroutine dashboard` to log me in via a one-time tokenized URL that sets a session cookie, so that auth is one command and the long-lived token never sits in browser storage. *(Renamed from `open` after 1.0.0, DECISIONS Q94; `open` stays as an alias.)*
44. As a UI user, I want One-shot Tasks shown with a countdown or absolute time — never presented as recurring — and Completed ones hidden behind a toggle, so that the list reflects reality.
45. As a user, I want the UI to be read-and-operate only, permanently, showing the file path to edit instead of an editor, so that files remain the only authoring surface and `git diff` stays the whole truth.
46. As a task author, I want editing a fired One-shot's definition to re-arm it — a new `at:` moment, or any content change for the ASAP kind — with completion recorded atomically in state as part of firing, so that a one-shot fires exactly once per moment, never zero times, never twice.
47. As a user, I want every Run recorded as a per-run directory with a metadata file (status: running/succeeded/failed/timed-out/interrupted, exit code, trigger, scheduled and actual times) and a merged `output.log` opening with a structured header, so that history is plain files I can read, grep, and back up.
48. *(Withdrawn for 1.0: discovery no longer exists — only explicitly registered files are read — and the symlink rules and generated-file write refusals went with it and with CRONTAB.md.)*
49. *(Withdrawn for 1.0 with story 15.)*
50. As a mission-conscious user, I want my task files to remain legible markdown that any future tool could run, so that adopting OpenRoutine is never a lock-in of its own.

## Implementation Decisions

- **Topology**: one machine-global Daemon per ADR-0001; task files registered individually in config (`tasks = [...]`, canonical absolute paths) per ADR-0002; canonical Task Id is derived from the frontmatter `name` by the same conversion Claude Desktop applies to a name typed in its UI (lowercase, whitespace runs to single hyphens, drop anything outside `[a-z0-9_-]`, trim the ends), so the Id is always safe as a directory and a URL segment while the name stays readable. Uniqueness is checked on the Id, never the name: two names deriving one Id is refused by `add` and, on Reload, resolved by config order — first wins, the later file is Broken with a collision error. No migration from the v1 `[[projects]]` model — an old config gets a friendly error pointing at `openroutine add <file.md>`; orphaned `runs/<project>/<task>/` history is left where it lies.
- **Storage split**: definitions are the registered markdown files, whole and only; runtime state is a single `scheduled-tasks.json` (per task: id, absolute `filePath`, the **resolved** `cwd` — always present even when the frontmatter omits it — latest-run pointers, `recordedSkips`, pause toggles, one-shot completions, global pause) under the XDG state dir; run history is per-run directories `runs/<name>/<run-id>/` holding `run.json` + `output.log`; run id = sortable compact UTC start timestamp with a collision suffix. All state timestamps UTC. The Claude-Desktop-style layout is homage, not an interop contract.
- **Config**: one TOML under the XDG config dir on both platforms: the `tasks` registry, agent templates (built-in `claude`/`codex`/`gemini`, overridable), `default_agent` (unset by default — omitted `agent:` is Broken until set), global `[env]`, `max_parallel`, retention and output caps, bind override. Repo-level agent definitions are rejected by design (a PR must not be able to change what command runs).
- **Frontmatter schema v1**: `name` (required; free prose, from which the Task's Id is derived — see DECISIONS Q88 — the Id must come out 2–50 characters and contain a letter or digit), `description` (required), non-empty prompt body (required), `cron` xor `at` (both = Broken; **neither = a one-shot that fires once, ASAP after the daemon takes it in**), `agent`, `disabled`, `env`, `cwd` (optional; absolute allowed, relative resolves against the task file's directory, default is that directory), `jitter`, `model`, `permission_mode`, `catch_up`. Reserved: `tz`, `on_failure`. Unknown keys warn-and-run. No filename rule: any registered markdown file qualifies; `.cron.md` is a convention, not a mechanism.
- **Scheduling semantics**: 5-field cron + `@` aliases, no seconds; host-local time; DST pinned (skip → next valid instant, repeat → once); deterministic id-derived jitter, window ≤ 5 min capped at half the interval; every due Tick becomes exactly one Run or one recorded Skip (`overlap` per-tick; `daemon-down` collapsed per outage window, capped list); no queueing; `catch_up: true` = one run for the most recent miss within 7 days, at startup. One-shot completion is written to state atomically as part of firing — keyed to the `at:` value, or for the ASAP kind to the definition digest — and editing the definition re-arms it.
- **Reload model**: the Daemon reads config and every registered file at startup and on explicit Reload — `openroutine reload`, `POST /v1/reload`, and automatically (best-effort) after `add`/`remove`. No file watcher, no rescan interval, nothing polled. A config that stops parsing keeps the previous config; a registered file that stops parsing becomes a Broken task. New and changed definitions are flagged loudly at Reload (digest-based), never blocked.
- **Refresh model** (DECISIONS Q82–Q84, Q92): separately from Reload, a Task re-reads *its own* file at the moment it is about to Run — scheduled Tick, Fire, or catch-up. mtime is a pre-filter held in memory; a changed mtime causes a read, and only a changed content digest counts as a change. The refreshed definition is what runs. A definition that changed into something unrunnable — it no longer loads, it switched itself off, or it renamed itself — **withdraws**: the Run is skipped and the Task holds no schedule at all until a Reload says what it has become. A One-shot whose moment moved before it arrived is only **re-armed**: that one Tick is skipped and the Task stays scheduled, at the moment it now names. A cron Tick absent from the new schedule still runs, since it was legitimately due when it was planned. All four record a Skip with reason `definition-changed`, carrying a `detail` saying which it was. The config is never re-read on this path, and no other Task's file is touched. Boundary, accepted: a Completed One-shot, a Disabled Task, and a Broken Task never fire, so only a Reload reaches them — flipping `disabled` back to `false` does not take effect on its own.
- **Execution**: runs launch through the user's login shell with `{prompt}`/`{model}`/`{permission_mode}` substituted as single argv tokens (unset field → template segment omitted; unknown template token → config-load error; no placeholder → body on stdin); per-run process group; TERM → 10s → KILL; cwd defaults to the task file's directory; runs record scheduled vs actual start.
- **CLI**: `serve, install, uninstall, add, remove [--delete], reload, list, run [--dry-run], logs [--follow], status, dashboard, init`. `add` registers one existing markdown file after hard validation (parse, mandatory fields, unique name, known agent) — it scaffolds nothing; `remove` unregisters by name or path and deletes the file only under `--delete`; `reload` reloads everything and reports per-task results; `init` writes the starter config (empty `tasks = []`) and prints a sample task. Reads come from disk; actions go through the daemon API; `run` without the daemon errors and suggests `serve`. There is deliberately no `exec` — one execution path. `--dry-run` prints the resolved plan and executes nothing.
- **API contract**: task list/detail (recent Skips embedded), paged run history, fire (optional `text` ≤ 64 KB in a documented `<run-context>` wrapper; 409 with active run id when running; 413 above cap), pause/resume per task, global pause/resume, reload, run detail, run cancel, SSE log stream, plain-text log fetch. Single-segment task paths (`/v1/tasks/{name}`, `/v1/runs/{name}/{run}`). Bearer token on every endpoint; errors as a structured `error` object with proper status codes; null-vs-empty distinguished; `nextFireAt` nullable.
- **UI**: served by the daemon from assets embedded in the binary (no CDN, no runtime Node); read-and-operate only, permanently; auth via one-time tokenized URL → session cookie, with manual token paste fallback; one-shots as countdowns; Completed behind a toggle.
- **Durability**: flock single-instance; atomic temp-file+rename state writes; corrupt state renamed aside and regenerated loudly; retention keep-last-50 per task; `output.log` capped with a truncation marker; daemon's own diagnostics to a log file in the state dir + stderr.
- **Trust model**: registering a file trusts whoever can edit it — new and changed Tasks always auto-schedule at Reload and are always loudly flagged; no approval gate.
- **Stack**: async single binary on tokio; axum-class HTTP; DST-correct cron crate; serde; MSRV recent stable; permissive-license deps only. (The notify/ignore watching-and-walking crates left with discovery.)
- **Milestones**: v0.1 core daemon + CLI → v0.2 REST API + install → v0.3 web UI → the file-registration redesign, released as **1.0.0** (DECISIONS Q93). The gate this bullet set — nothing called 1.0 until all three exist and are stable — is met. From 1.0 the frontmatter schema, CLI, REST API, and on-disk layout are a stable surface; breaking any of them waits for 2.0.

## Testing Decisions

Two seams, confirmed with the user; no others.

- **The process boundary (primary)**: integration tests spawn the real `openroutine` binary in a sandbox — temp `$XDG_CONFIG_HOME`/`$XDG_STATE_HOME`, temp task files registered in a generated config — and assert only on public surfaces: CLI exit codes and output, REST API responses, and the designed file artifacts (`scheduled-tasks.json`, `runs/` tree). The agent contract doubles as the execution probe: tests register stub agent templates — scripts recording argv, env, cwd, and stdin, emitting chosen output, exit codes, and delays — so delivery mode, env layering, timeout kills, output capture, and caps are all observable without any test hook in the product. The fire endpoint (and `run`) exercises the whole run path without waiting for a tick; the two ways a definition changes each get their own regression: an edit must reach a Run through Refresh without any Reload, and an edit to a Task that is not due must NOT take effect until `openroutine reload` runs.
- **The clock (the one new seam)**: the daemon reads time through a single injected clock; the scheduling core — due-tick computation, jitter offsets, DST transitions, Skip collapsing, `at:` and `catch_up` semantics — is a pure library surface tested deterministically against synthetic clocks and synthetic prior state. Jitter determinism (same id → same offset), spring-forward/fall-back nights, and outage-window collapse get exhaustive table-driven cases here.
- A good test observes external behavior only: what files exist, what the API returns, what the process printed, what the stub agent received. No test reaches into internal structs, module layouts, or private functions; no test depends on wall-clock sleeps for scheduling assertions (stub-agent delays for overlap/cancel tests are bounded and explicit).
- Prior art: none — this is a greenfield repo. Conventions to establish: Rust workspace-standard integration tests driving the binary, and library unit tests for the pure core; tests must pass offline with no network and leave nothing outside their temp dirs.

## Out of Scope

- Windows (explicit non-goal, stated in the doc).
- Notifications and result delivery (email, push, inboxes) and any `on_failure` behavior — the field name is reserved, nothing more.
- Automatic retries (agent runs aren't idempotent).
- GitHub-event triggers, change monitoring, or any file watching or polling — definitions change at an explicit Reload, or when a Task re-reads its own file on the way to a Run; the REST fire endpoint is the designed integration point.
- An `exec`-style direct execution verb; per-task `tz:`; `{file}`/`{task_id}` template tokens; per-agent `env`/`idle_timeout` overrides; repo-level agent definitions; approval gates for new tasks.
- Running tasks in disposable git worktrees (Claude Desktop's `useWorktree`) — noted as possible future work, DECISIONS Q79.
- Importing Claude Desktop's `scheduled-tasks.json` — its layout was a design reference, not an contract; migration is a hand edit.
- TLS, real multi-user auth, or any remote-access story beyond the discouraged bind override (tunnels are the answer).
- Task authoring or editing in the web UI, permanently.
- Seconds-resolution cron.

## Further Notes

The authoritative design records are this spec (which absorbed the former `openroutine.md` manifesto — journal citations of `openroutine.md §X` resolve to the same-named sections here), `CONTEXT.md` (glossary — use its vocabulary: Task, Id, Registered, Run, Skip, Tick, Jitter, Idle timeout, Fire, Dry run, One-shot, Completed, Catch-up, Disabled, Paused, Interrupted, Reload, Refresh, Ready, Familiar, Broken, Agent, Daemon), `DECISIONS.md` (append-only journal; consult before reopening anything), and `docs/adr/` (0001 machine-global daemon; 0002 per-file registration, Reload, and Refresh). The mission frame for every judgment call: OpenRoutine is the open-source alternative to Claude Code Routines and ChatGPT scheduled tasks — local files, local execution, no caps, swappable agents. The two capability gaps conceded to the vendors (GitHub-event triggers; notification inbox) are deliberate and documented in the comparison table in `README.md`; do not quietly grow scope toward them.
