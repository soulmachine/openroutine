# OpenRoutine v1 — full product spec

Status: ready-for-agent

## Problem Statement

Developers now trust coding agents with recurring, unattended work — nightly dependency audits, TODO triage, docs-drift PRs — but the only schedulers for that work are the vendors' own. Claude Code Routines run in Anthropic's cloud, configured in a web UI tied to a claude.ai account, metered by subscription caps. ChatGPT's scheduled tasks live in an OpenAI account, fire only while the desktop app is running, and are capped per plan. In both, the schedule cannot be `git diff`'d, code-reviewed, or moved to the other vendor's agent. A user who wants "my repo's recurring agent work, defined in my repo, run on my machine, by whatever agent I choose" has no tool.

## Solution

OpenRoutine: a single Rust binary whose daemon is the scheduler, the REST API, and the local web UI. A Task is one `.cron.md` file in the user's repo — frontmatter for metadata (description, schedule, agent, and per-task knobs), body as the prompt. One machine-global Daemon (see ADR-0001) watches every registered Project, schedules everything in-process with deterministic jitter, runs each Task through a user-configured agent command template (Claude Code, Codex, or anything with a CLI), and records Runs and Skips as plain files. Definitions are markdown the human owns; state is JSON the machine owns; everything rehydrates from disk.

## User Stories

1. As a developer, I want a Task to be a single `.cron.md` file with frontmatter and a prompt body, so that my scheduled agent work is diffable, greppable, and code-reviewable like any other file in my repo.
2. As a developer, I want crontab syntax (standard 5-field plus `@hourly`/`@daily`/`@weekly`/`@monthly`) in the frontmatter, so that everything I already know about cron applies.
3. As a developer, I want a required `description:` field surfaced everywhere tasks are listed, so that every schedule is glanceable without reading prompts.
4. As a developer, I want a one-shot Task via `at:` with an RFC 3339 timestamp, so that I can schedule a single future run ("migrate the DB Saturday 6am") the same way I schedule recurring work.
5. As a developer, I want a Manual Task (no `cron:`, no `at:`) that only runs when Fired, so that alert-driven prompts live in my repo without a fake schedule.
6. As a developer, I want `disabled: true` in frontmatter, so that turning a task off is a reviewable commit, not invisible machine state.
7. As a developer, I want per-task `timeout:` (with an explicit `none`), `model:`, `permission_mode:`, `env:`, `cwd:`, and `jitter:` fields, so that one file fully describes how its run behaves.
8. As a developer, I want unknown frontmatter keys to warn but not break the Task, so that a task file written for a newer OpenRoutine doesn't brick an older daemon.
9. As a developer, I want a Broken Task to appear in every list with its exact parse error and a stopped schedule, so that a typo'd cron string can never make a task silently vanish — the launchd failure this tool exists to fix.
10. As a developer with several repos, I want one Daemon watching multiple registered Projects, each with a unique name, so that one port, one boot service, and one UI cover my whole machine.
11. As a developer, I want Task ids namespaced `<project>/<name>`, so that two repos can both have `nightly.cron.md` without conflict.
12. As a developer, I want task files discovered recursively while honoring `.gitignore`, so that I can organize a `tasks/` folder freely and never schedule something out of `node_modules`.
13. As a developer, I want hot reload — add or edit a file and the schedule updates immediately — so that there is no install/reload step to forget.
14. As a security-conscious developer, I want newly discovered or changed Tasks flagged loudly in the CLI, UI, and daemon log, so that a `git pull` that brings someone's task file never goes unnoticed (registering a Project means trusting its committers).
15. As a team lead, I want a deterministic `CRONTAB.md` generated at each Project root — description, schedule, agent, derived from definitions only — so that "what runs here and when?" is answerable in a code review, and the file never churns from run state.
16. As a repo owner, I want a per-project opt-out of `CRONTAB.md` generation, so that OpenRoutine never writes into a repo that doesn't want it.
17. As a user, I want `openroutine serve` to run the daemon in the foreground and `openroutine install` to register it — sudo-free — with systemd (user unit, lingering enabled) or launchd (per-user LaunchAgent), so that my tasks fire on a headless box without a GUI or root.
18. As a user, I want a second `serve` to refuse to start with a clear error, so that two daemons never race over one state file.
19. As a user whose machine was off, I want missed Ticks recorded as one collapsed Skip per outage window (`from`, `to`, count, reason `daemon-down`), so that a vacation shows as one honest entry, not thousands.
20. As a user, I want opt-in `catch_up: true` to fire one run for the most recent missed Tick (7-day lookback) at startup, so that a nightly digest still appears the morning after a power cut — but only when I asked for that.
21. As a user, I want a Tick that comes due while the previous Run is still going to become a Skip with reason `overlap`, so that a stuck 6-hour run never creates a backlog storm.
22. As a user, I want fire times offset by deterministic per-task jitter (id-derived, up to 5 minutes, capped at half the interval, `jitter: 0` to opt out), so that twenty midnight tasks don't stampede a vendor API at 00:00:00.
23. As a user, I want cron evaluated in my machine's local timezone with pinned DST rules (skipped wall-times fire at the next valid instant; repeated wall-times fire once), so that my 2am task behaves predictably in March and November.
24. As a user, I want every Run launched through my login shell with per-task `env:` and config `[env]` layered on top, so that agents find the same `PATH`, profile, and API keys they'd have in my terminal — even under a boot-started daemon.
25. As a user, I want the prompt handed to the agent as a single argument via the `{prompt}` placeholder (never spliced into a shell string) or piped to stdin when there's no placeholder, so that a prompt can never inject flags or shell syntax.
26. As a user, I want timeouts enforced with SIGTERM to the run's process group, a 10-second grace, then SIGKILL — default 1 hour — so that a hung agent and all its children die instead of running forever.
27. As a user, I want an optional `max_parallel` config cap where a blocked Tick waits for a slot (delayed, never skipped) and the Run records both scheduled and started times, so that I can bound agent concurrency without losing runs.
28. As an incident responder, I want a persisted global pause — `pause --all`, an API endpoint, a UI toggle, and an env var at serve start — so that I can stop all firing immediately while the daemon, API, and UI stay up for inspection.
29. As a user, I want the state file to be disposable: atomic writes, and a corrupt file renamed aside and regenerated with a loud warning, so that state damage costs me history, never tasks.
30. As a user, I want run retention capped (keep last 50 per task, configurable) and each `output.log` capped with a truncation marker, so that a chatty 15-minute task can't eat my disk.
31. As a new user, I want `openroutine init` to scaffold my first task and agent config, so that the first five minutes require no documentation.
32. As a user, I want `list` and `logs` to read straight from disk, so that they work even when the daemon is stopped.
33. As a user, I want `openroutine run <task>` to fire through the API and explain a 409 (already running, with the active run id), so that operating a task is explicit and honest.
34. As a task author, I want `run --dry-run` to print the fully resolved plan — exact argv after placeholder substitution, delivery mode, cwd, timeout, env override keys, next jittered fire times — computed offline, executing nothing, so that I can verify a task before its first tick.
35. As a user, I want `logs <task> --follow` to tail the current run live, and `status` to summarize the daemon at a glance, so that day-two operations stay in the terminal.
36. As an integrator, I want every Task to have an authenticated REST fire endpoint, so that alerting systems, deploy pipelines, and git hooks can start runs with a POST.
37. As an integrator, I want an optional `text` payload (≤ 64 KB, 413 above) delivered inside a documented `<run-context>` wrapper labeled as untrusted, informational, non-instructional content, so that anyone who can reach the endpoint can inform a run but never redefine the task.
38. As an integrator, I want pause/resume per task, `cancel` on a running Run, an SSE live-tail of any run log, and paged run history over the API, so that everything the UI can do, a script can do.
39. As a security-conscious user, I want the daemon bound to 127.0.0.1 by default with a bearer token required on every endpoint — reads included, since task lists leak prompts — and the token stored 0600, printable and rotatable via `openroutine token`, so that a stray local process can't fire or read my agents.
40. As a user who accepts the risk, I want a config bind override that keeps the token mandatory and is loudly discouraged in docs, so that a tailnet setup is possible without OpenRoutine pretending it's safe.
41. As a user, I want API responses to distinguish null (unknown) from empty (none found), and `nextFireAt` to be genuinely null for Manual and Completed Tasks, so that clients never parse sentinel values.
42. As a user, I want a local web UI — every task with schedule and next fire, run history with full logs, run-now/pause/resume/cancel, the global pause toggle — with no account anywhere, so that I get the vendors' dashboard experience against my own machine.
43. As a UI user, I want `openroutine open` to log me in via a one-time tokenized URL that sets a session cookie, so that auth is one command and the long-lived token never sits in browser storage.
44. As a UI user, I want One-shot Tasks shown with a countdown or absolute time — never presented as recurring — and Completed ones hidden behind a toggle, so that the list reflects reality.
45. As a user, I want the UI to be read-and-operate only, permanently, showing the file path to edit instead of an editor, so that files remain the only authoring surface and `git diff` stays the whole truth.
46. As a task author, I want editing a fired One-shot's `at:` to re-arm it, and completion recorded atomically in state as part of firing (watcher events being optimization only), so that a one-shot fires exactly once per timestamp — never zero times, never twice.
47. As a user, I want every Run recorded as a per-run directory with a metadata file (status: running/succeeded/failed/timed-out/interrupted, exit code, trigger, scheduled and actual times) and a merged `output.log` opening with a structured header, so that history is plain files I can read, grep, and back up.
48. As a security-conscious user, I want discovery to never follow symlinks out of a Project and generated-file writes to refuse symlinked targets, so that a malicious link can't pull foreign tasks in or redirect writes out.
49. As an AI agent working in a repo, I want `CRONTAB.md` sitting next to `README.md` and `AGENTS.md`, so that I can answer "what runs here?" without any tool access.
50. As a mission-conscious user, I want my task files to remain legible markdown that any future tool could run, so that adopting OpenRoutine is never a lock-in of its own.

## Implementation Decisions

- **Topology**: one machine-global Daemon per ADR-0001; Projects registered in config with unique names (default: directory basename); canonical Task id `<project>/<filename stem>`; rename or move = new Task identity.
- **Storage split**: definitions are the `.cron.md` files, whole and only; runtime state is a single `scheduled-tasks.json` (absolute `filePath`s, latest-run pointers, `recordedSkips`, pause toggles, one-shot completions, global pause) under the XDG state dir; run history is per-run directories `runs/<project>/<task>/<run-id>/` holding `run.json` + `output.log`; run id = sortable compact UTC start timestamp with a collision suffix. All state timestamps UTC. The Claude-Desktop-style layout is homage, not an interop contract.
- **Config**: one TOML under the XDG config dir on both platforms: registered projects, agent templates (built-in `claude`/`codex`/`gemini`, overridable), `default_agent` (unset by default — omitted `agent:` is Broken until set), global `[env]`, `max_parallel`, retention and output caps, per-project `crontab_md` opt-out, bind override. Repo-level agent definitions are rejected by design (a PR must not be able to change what command runs).
- **Frontmatter schema v1**: `description` (required), `cron` xor `at` (both = Broken; neither = Manual), `agent`, `timeout` (default 1h, `none` allowed), `disabled`, `env`, `cwd` (relative to Project root), `jitter`, `model`, `permission_mode`, `catch_up`. Reserved: `tz`, `on_failure`. Unknown keys warn-and-run.
- **Scheduling semantics**: 5-field cron + `@` aliases, no seconds; host-local time; DST pinned (skip → next valid instant, repeat → once); deterministic id-derived jitter, window ≤ 5 min capped at half the interval; every due Tick becomes exactly one Run or one recorded Skip (`overlap` per-tick; `daemon-down` collapsed per outage window, capped list); no queueing; `catch_up: true` = one run for the most recent miss within 7 days, at startup. One-shot completion is written to state atomically as part of firing and keyed to the `at` value; file watchers are a latency optimization over a reconciling rescan.
- **Execution**: runs launch through the user's login shell with `{prompt}`/`{model}`/`{permission_mode}` substituted as single argv tokens (unset field → template segment omitted; unknown template token → config-load error; no placeholder → body on stdin); per-run process group; TERM → 10s → KILL; cwd defaults to the Project root; runs record scheduled vs actual start.
- **CLI**: `serve, install, uninstall, add, remove, list, run [--dry-run], logs [--follow], status, open, init`. Reads come from disk; actions go through the daemon API; `run` without the daemon errors and suggests `serve`. There is deliberately no `exec` — one execution path. `--dry-run` prints the resolved plan and executes nothing.
- **API contract**: task list/detail (recent Skips embedded), paged run history, fire (optional `text` ≤ 64 KB in a documented `<run-context>` wrapper; 409 with active run id when running; 413 above cap), pause/resume per task, global pause/resume, run detail, run cancel, SSE log stream, plain-text log fetch. Bearer token on every endpoint; errors as a structured `error` object with proper status codes; null-vs-empty distinguished; `nextFireAt` nullable.
- **UI**: served by the daemon from assets embedded in the binary (no CDN, no runtime Node); read-and-operate only, permanently; auth via one-time tokenized URL → session cookie, with manual token paste fallback; one-shots as countdowns; Completed behind a toggle.
- **Durability**: flock single-instance; atomic temp-file+rename state writes; corrupt state renamed aside and regenerated loudly; retention keep-last-50 per task; `output.log` capped with a truncation marker; daemon's own diagnostics to a log file in the state dir + stderr.
- **Trust model**: registering a Project trusts its committers — new and changed Tasks always auto-schedule and are always loudly flagged; no approval gate.
- **Stack**: async single binary on tokio; axum-class HTTP; notify-class watching; DST-correct cron crate; serde; MSRV recent stable; permissive-license deps only.
- **Milestones**: v0.1 core daemon + CLI + CRONTAB.md → v0.2 REST API + install → v0.3 web UI. Nothing is called 1.0 or publicly pitched until all three exist.

## Testing Decisions

Two seams, confirmed with the user; no others.

- **The process boundary (primary)**: integration tests spawn the real `openroutine` binary in a sandbox — temp `$XDG_CONFIG_HOME`/`$XDG_STATE_HOME`, temp Project directories — and assert only on public surfaces: CLI exit codes and output, REST API responses, and the designed file artifacts (`scheduled-tasks.json`, `runs/` tree, `CRONTAB.md`). The agent contract doubles as the execution probe: tests register stub agent templates — scripts recording argv, env, cwd, and stdin, emitting chosen output, exit codes, and delays — so delivery mode, env layering, timeout kills, output capture, and caps are all observable without any test hook in the product. Manual Tasks plus the fire endpoint (and `run`) exercise the whole run path without waiting for a tick.
- **The clock (the one new seam)**: the daemon reads time through a single injected clock; the scheduling core — due-tick computation, jitter offsets, DST transitions, Skip collapsing, `at:` and `catch_up` semantics — is a pure library surface tested deterministically against synthetic clocks and synthetic prior state. Jitter determinism (same id → same offset), spring-forward/fall-back nights, and outage-window collapse get exhaustive table-driven cases here.
- A good test observes external behavior only: what files exist, what the API returns, what the process printed, what the stub agent received. No test reaches into internal structs, module layouts, or private functions; no test depends on wall-clock sleeps for scheduling assertions (stub-agent delays for overlap/cancel tests are bounded and explicit).
- Prior art: none — this is a greenfield repo. Conventions to establish: Rust workspace-standard integration tests driving the binary, and library unit tests for the pure core; tests must pass offline with no network and leave nothing outside their temp dirs.

## Out of Scope

- Windows (explicit non-goal, stated in the doc).
- Notifications and result delivery (email, push, inboxes) and any `on_failure` behavior — the field name is reserved, nothing more.
- Automatic retries (agent runs aren't idempotent).
- GitHub-event triggers, change monitoring, or any watcher beyond task-file hot reload — the REST fire endpoint is the designed integration point.
- An `exec`-style direct execution verb; per-task `tz:`; `{file}`/`{task_id}` template tokens; per-agent `env`/`timeout` overrides; repo-level agent definitions; approval gates for new tasks.
- TLS, real multi-user auth, or any remote-access story beyond the discouraged bind override (tunnels are the answer).
- Claude Desktop interop guarantees — the shared layout is provenance only.
- Task authoring or editing in the web UI, permanently.
- Seconds-resolution cron.

## Further Notes

The authoritative design records are `openroutine.md` (manifesto), `CONTEXT.md` (glossary — use its vocabulary: Task, Project, Run, Skip, Tick, Fire, Dry run, Manual, One-shot, Completed, Disabled, Paused, Broken, Agent, Daemon), `DECISIONS.md` (54-entry append-only journal; consult before reopening anything), and `docs/adr/0001` (machine-global daemon). The mission frame for every judgment call: OpenRoutine is the open-source alternative to Claude Code Routines and ChatGPT scheduled tasks — local files, local execution, no caps, swappable agents. The two capability gaps conceded to the vendors (GitHub-event triggers; notification inbox) are deliberate and documented in the comparison table; do not quietly grow scope toward them.
