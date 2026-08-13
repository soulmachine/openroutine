---
name: openroutine
description: Schedule or manage recurring tasks on this machine with openroutine — authoring a task file, or listing, firing, pausing, and debugging registered ones. Use for any recurring job that needs this host's files, keychain, Messages, or local CLIs, in place of the cloud schedulers (/schedule, CronCreate) that run on Anthropic's infrastructure and cannot reach this machine, and in place of hand-written launchd plists or crontab entries.
---

# openroutine — scheduled tasks on this machine

`openroutine` (<https://openroutine.dev>) is "your AI agents' crontab, as markdown": one
markdown file per task, one Rust daemon running them. It is the scheduler for anything
recurring that must touch *this host*, and it is what replaces a hand-written launchd
plist or `crontab` entry.

Read the authoritative reference first — both stay current on their own:

- `~/github.com/soulmachine/openroutine/README.md` — the task file format, the CLI, what
  `reload` does after an edit, the REST fire endpoint, and the login-shell `PATH`
  requirement that agent CLIs trip over.
- `openroutine help <command>` — every flag.

The rest of this skill is only what those don't say: how openroutine is set up here.

## This machine

The daemon is installed and running under launchd. `openroutine install` re-registers it
without sudo if that ever breaks, and `openroutine status` prints the daemon, the config
path, the state path, and a health summary.

Task files live in one of two places, by scope:

- **Project** — `<repo>/.openroutine/<name>.cron.md`, so the task is diffed and reviewed
  alongside the code it serves.
- **Machine** — `~/.config/openroutine/<name>.md` for anything not tied to a repo.

Either way, register the file with `openroutine add <file>`, which records its absolute
path in the config. Writing the file alone schedules nothing.

## Choosing the agent

A task names an `agent:`, or inherits the config's default. `~/.config/openroutine/config.toml`
holds the command templates; which one a task wants:

- `claude` (the default when a task names none) or `codex` — the task is a judgement call
  and needs a model.
- `shell` — the task is a deterministic script. No model, so no API spend and no latency;
  the task body is the command line.
- `osascript` — the task is raw AppleScript. macOS judges Apple Events by the responsible
  process, so driving Messages or another scriptable app has to come from openroutine
  itself rather than through an agent in between.
