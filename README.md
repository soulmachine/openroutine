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

## Status

**Design phase — no code yet.** The design is complete and recorded:

- [openroutine.md](openroutine.md) — the full design: scheduling semantics, storage, agent contract, API, and security model
- [CONTEXT.md](CONTEXT.md) — the project glossary
- [docs/adr/](docs/adr/) — architecture decisions of record

## License

See [LICENSE](LICENSE).
