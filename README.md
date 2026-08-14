# OpenRoutine

> Your AI agents' crontab, as markdown.

**OpenRoutine is the open-source alternative to [Claude Code Routines](https://code.claude.com/docs/en/routines) and [ChatGPT's scheduled tasks](https://learn.chatgpt.com/docs/automations?surface=app)** — the same unattended-agent idea, with the vendor locks removed. One markdown file per task, crontab syntax in the frontmatter, any AI coding agent underneath. Tasks live in your repo, run on your machine, and answer to no account, plan, or cap.

**Website:** [openroutine.dev](https://openroutine.dev) — hand-written HTML under
[`site/`](site/), no build step, in English and [简体中文](https://openroutine.dev/zh/).
Cloudflare Workers Builds watches this repository, so a push to `main` that touches `site/`
deploys it; one that touches only the Rust project does not trigger a build.
[site/DEPLOY.md](site/DEPLOY.md) has the settings.

## A task is a file

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

Save that in your repo as `todo-digest.md`, register it once with `openroutine add todo-digest.md`, and it is the complete definition — diffed, code-reviewed, and greppable like everything else you commit. A single Rust daemon (`openroutine serve`, or `openroutine install` once for boot) runs your registered task files, schedules everything in-process, runs each task through the agent CLI you configure, and serves a REST fire endpoint plus a local web UI for history, logs, and pause/resume. Swap `agent: claude` for `agent: codex` — one line in a diff — and the same task runs on the other vendor's agent, which is precisely the move neither vendor's scheduler will ever offer.

## Compared to the vendors

|  | Claude Code Routines | ChatGPT scheduled tasks | OpenRoutine |
| :-- | :-- | :-- | :-- |
| Where it runs | Anthropic-managed cloud (or org-hosted environments) | OpenAI cloud; project tasks run locally, but only while the desktop app is running | Your machine, under a headless daemon supervised by launchd/systemd |
| Where tasks are defined | Web UI / `/schedule`, stored in your claude.ai account | Chat or the Scheduled page, stored in your OpenAI account | Markdown files in your repo |
| Agents | Claude Code only | GPT models only | Any agent CLI: Claude Code, Codex, Gemini, your own |
| Triggers | Schedules, API endpoint, GitHub events | Schedules and change-monitoring, hourly at most | Cron and one-shot schedules; REST fire endpoint |
| Management UI | claude.ai web UI | Scheduled page in the app | Local web UI, no account |
| Limits | Subscription usage, daily run caps | 3–15 active tasks by plan; unattended tasks may auto-pause | Whatever your hardware tolerates |
| Availability | Research preview, paid plans | Paid plans | Open source |

The trade is honest in every direction: Routines gives you cloud execution, GitHub-event triggers, and managed sandboxing; ChatGPT gives you a polished cross-device inbox and change-monitoring; OpenRoutine gives you local files, local execution, no caps, and the freedom to swap the agent.

## Install

Requires a Rust toolchain; macOS and Linux only (Windows is an explicit non-goal).

```bash
cargo install --path .          # or: cargo build --release
openroutine init                # writes a config and prints a sample task
openroutine add my-task.md      # register a task file you wrote
openroutine list                # see what would run
openroutine serve               # run the scheduler in the foreground
openroutine install             # or register it as a boot service, no sudo
```

`init` writes `~/.config/openroutine/config.toml` and prints a sample task file
to copy from; `add` registers each task file you write, after validating it.
Nothing else to configure. [Deploy](#deploy) covers leaving it running on a
machine you don't sit at.

## Using it

```bash
openroutine list                  # every task, its schedule, and its health
openroutine status                # is the daemon up, and what does it hold
openroutine reload                # re-read the config and every task file
openroutine run <task> --dry-run  # exactly what a run would do, spawning nothing
openroutine run <task>            # fire one now, through the daemon
openroutine logs <task> --follow  # tail the latest run
openroutine pause --all           # stop everything firing, keep the daemon up
openroutine dashboard             # the local web UI, no account
```

Tasks are files, so everything else is ordinary editing. A task re-reads its
own file as it is about to run, so the prompt you just fixed is the one that
runs — but nothing is watched or polled, so the schedule itself never moves
between runs. `openroutine reload` is what makes an edit visible everywhere
else, and `add` and `remove` ask for one themselves.

The one thing to remember: a task that isn't going to run can't notice that
you changed it. A finished one-shot, a broken task, and a task you switched
off with `disabled: true` all need `openroutine reload` — flipping `disabled`
back to `false` in the file does nothing on its own. (Turning a task off that
way is still the right move: it's a change your reviewer can see.)

The daemon also serves a REST API on `127.0.0.1:7373`, guarded by a bearer
token (`openroutine token`), so alerting systems and git hooks can fire a task:

```bash
curl -X POST http://127.0.0.1:7373/v1/tasks/todo-digest/fire \
  -H "Authorization: Bearer $(openroutine token)" \
  -d '{"text": "Sentry alert SEN-4521 fired in prod."}'
```

The optional `text` reaches the agent labelled as caller-supplied context, not
as instructions — anyone who can reach the endpoint can send text, so text must
not be able to redefine the task.

## Deploy

Everything above drives the daemon by hand. To leave it running unattended on
a machine you don't sit at — the Mac mini under the desk, a home server —
register it with the system's service manager:

```bash
cargo install --path .        # install to a stable path; see below
openroutine init              # write the config; prints a sample task
openroutine add hello.md      # register a task to prove it works
openroutine install           # register with launchd/systemd, no sudo
openroutine status            # daemon: running (pid …)
```

`install` records the path of the binary that registers it, so run it from the
installed copy rather than from `target/release/openroutine` — a `cargo clean`
should not be able to unmake your scheduler. It writes a per-user service and
never asks for sudo. `openroutine install --print` shows exactly what it would
write, and what it would run, without writing anything.

### Your agent must be on the login shell's PATH

The daemon runs every agent through a *login* shell, so a run gets the same
`PATH`, shims, and API keys your terminal has. A login shell is not an
interactive one: zsh reads `.zshenv` and `.zprofile` but **not** `.zshrc`, and
bash reads `.bash_profile` but not `.bashrc`. So an agent that only your
`.zshrc` puts on the `PATH` — anything in `~/.local/bin` is the usual case —
is found when you test by hand and missing once launchd starts the daemon:

```
zsh:1: command not found: claude
```

`openroutine list` warns before you get there. It samples the login shell the
way a service manager starts one — with `PATH` seeded to the bare
`/usr/bin:/bin:/usr/sbin:/sbin` a daemon inherits, never the `PATH` your
terminal happens to have — so it answers for the daemon rather than for you:

```
warning: "claude" is on your PATH here but not under a service manager, so
scheduled Runs will fail; move its PATH export into your login profile
```

Fix it by moving the `PATH` export into `.zprofile`, which repairs SSH and
cron sessions at the same time, or by naming the agent absolutely:

```toml
[agents.claude]
cmd = "/Users/you/.local/bin/claude -p {prompt}"
```

Verify the way the daemon will see it — a login shell with none of your
terminal's inherited environment:

```bash
env -i HOME="$HOME" SHELL=/bin/zsh PATH=/usr/bin:/bin /bin/zsh -lc 'command -v claude'
```

### macOS

`install` writes a **LaunchAgent** to `~/Library/LaunchAgents/`, so the daemon
starts at *login* rather than at boot, and launchd restarts it if it dies. On
a headless machine, pair it with auto-login (System Settings → Users & Groups
→ Automatically log in as), which requires FileVault to be off.

Auto-login is not only about the daemon starting. Agent CLIs keep credentials
in your **login keychain**, and that keychain is unlocked by the GUI login — a
service that starts without one finds it locked, and the agent reports itself
logged out. That is also why `install` does not write a root LaunchDaemon:
starting before anyone logs in is precisely the state in which the agent
cannot authenticate, so the one thing a LaunchDaemon buys is the one thing
that breaks it. The trade runs the other way too, and it is a real one:
auto-login with FileVault off means physical access is a logged-in desktop.

While you are there, stop the machine sleeping through its own schedules:

```bash
sudo pmset -c sleep 0 displaysleep 0 disksleep 0  # never sleep
sudo pmset -c autorestart 1 womp 1                # return after power loss
launchctl print gui/$(id -u)/dev.openroutine.daemon | grep state
```

### Linux

`install` writes a systemd user unit and enables lingering, so the daemon
starts at boot with no login session — the one platform where "no login
needed" holds without an asterisk.

```bash
systemctl --user status dev.openroutine.daemon
journalctl --user -u dev.openroutine.daemon -f
```

### Confirming it survives

```bash
openroutine run <task>   # a real run, end to end
openroutine logs <task>  # what the agent actually printed
```

Killing the daemon outright is a fair test: the service manager should bring
it back within seconds under a new pid. `openroutine uninstall` unregisters
it and leaves your config, state, and tasks untouched.

## Status

**Implemented**: scheduler, runner, REST API, and web UI, in one binary.
[.scratch/openroutine-v1/spec.md](.scratch/openroutine-v1/spec.md) is the full
design; [CONTEXT.md](CONTEXT.md) is the glossary; [docs/adr/](docs/adr/)
records the architectural decisions.

**1.0 breaks with 0.2.** Tasks are registered one file at a time rather than
by directory, so a `[[projects]]` config no longer loads — register each file
with `openroutine add <file.md>`. Task files now need a `name:`, their id is
derived from it, and `timeout:` is gone: a run is bounded by silence
(`idle_timeout` in config, 15m) rather than by total runtime. Run history from
before the change is orphaned rather than migrated.

**1.0 means the design has settled, not that the surface is frozen.** The
frontmatter schema, the CLI, the REST API, and the on-disk layout are all
documented and meant to last — but while the project has essentially no
installed base, a bad name is worth fixing rather than carrying to 2.0.
Breaking changes get a release note and a line in `DECISIONS.md`; they do not
get a version-number promise they would only strain against.

Known limits, all deliberate: no GitHub-event triggers and no notifications
(the fire endpoint is the integration point); state is written atomically
against a killed process but is not fsynced against power loss; the web UI's
rendering is verified by hand rather than by a browser harness.

## License

See [LICENSE](LICENSE).
