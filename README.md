# OpenRoutine

> Your AI agents' crontab, as markdown.

**OpenRoutine is the open-source alternative to [Claude Code Routines](https://code.claude.com/docs/en/routines) and [ChatGPT's scheduled tasks](https://learn.chatgpt.com/docs/automations?surface=app)** — the same unattended-agent idea, with the vendor locks removed. One markdown file per task, crontab syntax in the frontmatter, any AI coding agent underneath. Tasks live in your repo, run on your machine, and answer to no account, plan, or cap.

**Website:** [openroutine.dev](https://openroutine.dev)

## A task is a file

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

Drop that in your repo as `todo-digest.cron.md` and it is the complete definition — diffed, code-reviewed, and greppable like everything else you commit. A single Rust daemon (`openroutine serve`, or `openroutine install` once for boot) watches your registered projects, schedules everything in-process, runs each task through the agent CLI you configure, and serves a REST fire endpoint plus a local web UI for history, logs, and pause/resume. Swap `agent: claude` for `agent: codex` — one line in a diff — and the same task runs on the other vendor's agent, which is precisely the move neither vendor's scheduler will ever offer.

## Compared to the vendors

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

## Install

Requires a Rust toolchain; macOS and Linux only (Windows is an explicit non-goal).

```bash
cargo install --path .          # or: cargo build --release
openroutine init .              # writes a config and a sample task
openroutine list                # see what would run
openroutine serve               # run the scheduler in the foreground
openroutine install             # or register it as a boot service, no sudo
```

`init` writes `~/.config/openroutine/config.toml`, registers the directory you
name, and leaves a sample `hello.cron.md` beside it. Nothing else to configure.

## Using it

```bash
openroutine list                  # every task, its schedule, and its health
openroutine status                # is the daemon up, and what does it hold
openroutine run <task> --dry-run  # exactly what a run would do, spawning nothing
openroutine run <task>            # fire one now, through the daemon
openroutine logs <task> --follow  # tail the latest run
openroutine pause --all           # stop everything firing, keep the daemon up
openroutine open                  # the local web UI, no account
```

Tasks are files, so everything else is ordinary editing: drop a `.cron.md` in a
registered directory and it schedules within seconds; `git pull` one in and the
same happens, flagged as new so you notice. Turn one off with `disabled: true`
in its frontmatter — a change your reviewer can see.

The daemon also serves a REST API on `127.0.0.1:7373`, guarded by a bearer
token (`openroutine token`), so alerting systems and git hooks can fire a task:

```bash
curl -X POST http://127.0.0.1:7373/v1/tasks/myrepo/todo-digest/fire \
  -H "Authorization: Bearer $(openroutine token)" \
  -d '{"text": "Sentry alert SEN-4521 fired in prod."}'
```

The optional `text` reaches the agent labelled as caller-supplied context, not
as instructions — anyone who can reach the endpoint can send text, so text must
not be able to redefine the task.

## Status

**v1 is implemented**: scheduler, runner, REST API, and web UI, in one binary.
[openroutine.md](openroutine.md) is the full design; [CONTEXT.md](CONTEXT.md)
is the glossary; [docs/adr/](docs/adr/) records the architectural decisions.

Known limits, all deliberate: no GitHub-event triggers and no notifications
(the fire endpoint is the integration point); state is written atomically
against a killed process but is not fsynced against power loss; the web UI's
rendering is verified by hand rather than by a browser harness.

## License

See [LICENSE](LICENSE).
