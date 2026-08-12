# OpenRoutine

> Your AI agents' crontab, as markdown.

**Website:** [openroutine.dev](https://openroutine.dev)

**OpenRoutine** is an open-source alternative to [Claude Code Routines](https://code.claude.com/docs/en/routines) and to [ChatGPT's scheduled tasks](https://learn.chatgpt.com/docs/automations?surface=app) in the macOS desktop app, written in Rust — one markdown file per task, crontab syntax in the frontmatter, any AI coding agent underneath. Drop a `.cron.md` file in your repo and Claude Code, Codex, or any agent you configure runs it on schedule. One daemon is the scheduler, the RESTful API, and the local web UI.

## Why

The major vendors have proved the idea: coding agents are good enough to work unattended — nightly dependency audits, backlog grooming, docs-drift PRs. But each vendor's scheduler locks the recurring work to its own stack. Claude Code's Routines run on Anthropic-managed cloud infrastructure, with configuration living in a web UI tied to your claude.ai account, metered against subscription limits and daily caps. ChatGPT's scheduled tasks come closer to home — project-scoped tasks in the macOS desktop app run against your local checkout — but the tasks are created conversationally, stored in your OpenAI account, fire only while the GUI app is running, and are capped per plan, limited to hourly at most, and may auto-pause when left unattended. In both cases, the one thing you can't do is `git diff` your schedule — and neither will ever run the other's agent.

OpenRoutine takes the opposite position on each of those. A scheduled agent task is just a prompt with a schedule attached, so it should live as a markdown file in your repo — diffed, code-reviewed, and greppable like everything else. It should run on your own machine, under a small headless daemon rather than a GUI app or someone else's cloud. And it shouldn't care which agent executes it.

## How it works

A task is a single `*.cron.md` file. The frontmatter says **what**, **when**, and **who**; the body is the prompt.

```markdown
---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: claude
timeout: 30m
---

Review all open TODO and FIXME comments in this repository.
For any that are trivially fixable, fix them and open a pull
request. Summarize everything else in reports/todo-digest.md.
```

Start the daemon with `openroutine serve`, or register it once as a boot service with `openroutine install`. The daemon scans every registered project, parses each frontmatter schedule, and schedules everything in-process — each fire time offset by a small deterministic jitter (tunable per task with `jitter:`, down to `0` for exact ticks) so a machine full of midnight tasks doesn't stampede a vendor API. A task can also carry a one-shot `at:` timestamp instead of `cron:`, or no schedule at all: a manual task that runs only when fired.

When a task fires, a runner strips the frontmatter and hands the body to the configured agent — substituted for the command template's `{prompt}` placeholder as a single argument, never spliced into a shell string, or piped to stdin when the template has no placeholder — under a timeout and overlap protection (a task never runs concurrently with itself), and logs each run to a local directory.

Task files are watched: add or edit a `.cron.md` and the schedule updates immediately — no reinstall step. That immediacy is a trust statement — registering a project means trusting its committers, so newly discovered tasks are flagged loudly in the CLI and UI, never silently blocked.

## The scheduler is the daemon — not launchd, not crontab

OpenRoutine deliberately does **not** reuse macOS launchd or Linux crontab. The daemon carries its own in-process scheduler, and the OS supervises exactly one process while scheduling nothing.

This is a considered trade, because launchd and cron are where agent jobs quietly go to die: they run tasks in a stripped environment with no `PATH`, shell profile, or API keys your agent CLI expects; their definitions live in per-OS dialects (plists on one machine, crontab lines on another); and they offer no first-class overlap control, timezone handling, or structured logging. Owning the scheduler buys identical behavior on macOS and Linux, tasks that run with a real environment — every run launches through your login shell, so `PATH`, profile, and API keys match your terminal, with per-task `env:` frontmatter and config `[env]` overrides layered on top — hot-reload on file changes, and honest bookkeeping: runs missed while the daemon is down are recorded as skips in the state file rather than silently dropped.

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

**The task definition is the markdown file — all of it.** Every metadata field (description, schedule, agent, timeout, `disabled`, and anything added later) lives in the frontmatter, and the prompt is the body. There is no second place to look: the `.cron.md` file is the complete, self-contained definition, which is what makes tasks portable, diffable, and reviewable in a pull request.

**Run state goes to `scheduled-tasks.json`.** Things the machine learns by running — last run time, the tick it was scheduled for, recorded skips, pause toggles — are not part of the definition and don't belong in git. OpenRoutine keeps them in a single JSON file outside the repo, keyed back to the task files by id — `<project>/<filename stem>`, e.g. `myrepo/todo-digest`:

```json
{
  "scheduledTasks": [
    {
      "id": "myrepo/todo-digest",
      "filePath": "/home/you/repos/myrepo/tasks/todo-digest.cron.md",
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
curl -X POST http://127.0.0.1:7373/v1/tasks/myrepo/todo-digest/fire \
  -H "Authorization: Bearer $OPENROUTINE_TOKEN" \
  -d '{"text": "Sentry alert SEN-4521 fired in prod. Stack trace attached."}'
```

The optional `text` field is delivered to the agent alongside the task's saved prompt, wrapped and labeled as untrusted run-specific context rather than as instructions — the same discipline Routines applies to its fire payloads, and for the same reason: anyone who can reach the endpoint can send text, so text must not be able to redefine the task. The response returns a run id and the path to its log. The rest of the API is reads plus the operational controls: `GET /v1/tasks` lists every discovered task with its schedule and next fire time, `GET /v1/tasks/<id>/runs` pages through history, an SSE endpoint live-tails any run's log, and pause, resume, and `POST /v1/runs/<id>/cancel` cover the runaway case.

**The web UI** is the local, single-user answer to claude.ai/code/routines and ChatGPT's Scheduled page: every task with its schedule and next fire time, per-task run history with full logs, and run-now, pause, and resume controls (per task, or a global pause for the whole daemon) — served from the daemon, viewable in any browser, no account anywhere.

The daemon binds to `127.0.0.1` by default and requires a bearer token generated on first `serve`, so a stray process on your machine can't fire your agents. A config override can widen the bind, but the docs discourage it loudly and the token stays mandatory either way. And it holds no private state: everything it serves is read from the `*.cron.md` files, `scheduled-tasks.json`, and the run logs, and everything it writes goes back to those same files. Restart it and it rehydrates entirely from disk.

## Compared to the vendor schedulers

|  | Claude Code Routines | ChatGPT scheduled tasks | OpenRoutine |
| :-- | :-- | :-- | :-- |
| Where it runs | Anthropic-managed cloud (or org-hosted environments) | OpenAI cloud; project tasks run locally, but only while the desktop app is running | Your machine, under a headless daemon supervised by launchd/systemd |
| Where tasks are defined | Web UI / `/schedule`, stored in your claude.ai account | Chat or the Scheduled page, stored in your OpenAI account | `*.cron.md` files in your repo |
| Agents | Claude Code only | GPT models only | Any agent CLI: Claude Code, Codex, Gemini, your own |
| Triggers | Schedules, API endpoint, GitHub events | Schedules and change-monitoring, hourly at most | Cron, one-shot, and manual schedules; REST fire endpoint |
| Management UI | claude.ai web UI | Scheduled page in the app | Local web UI, no account |
| Limits | Subscription usage, daily run caps | 3–15 active tasks by plan; unattended tasks may auto-pause | Whatever your hardware tolerates |
| Availability | Research preview, paid plans | Paid plans | Open source |

The trade is honest in every direction: Routines gives you cloud execution, GitHub-event triggers, and managed sandboxing; ChatGPT gives you a polished cross-device inbox and change-monitoring; OpenRoutine gives you local files, local execution, no caps, and the freedom to swap the agent.

## CRONTAB.md

Alongside your tasks, OpenRoutine generates a `CRONTAB.md` at the root of each registered project — a table of that project's tasks: description, schedule, agent. It is derived from the task definitions alone, never from run state, so it changes only when a definition changes and is safe to commit. It sits next to `README.md` and `AGENTS.md` and serves the same purpose for humans and agents alike: one glanceable answer to "what runs here, and when?" — while live status (last run, next fire) stays in the web UI and API.

## Design principles

One binary, one process, zero databases. The daemon is the scheduler, the API, and the UI in a single static Rust binary with no runtime dependencies; the OS supervises exactly one process and schedules nothing. All durable data is two plain-text formats — markdown you own, JSON the machine owns — so everything that defines behavior is a file in git, and everything the tool generates is a file you can read. If OpenRoutine disappeared tomorrow, your tasks would still be legible markdown, ready for whatever runs them next.
