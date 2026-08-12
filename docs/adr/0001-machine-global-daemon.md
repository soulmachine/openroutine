# One machine-global daemon, not one per project

OpenRoutine runs a single daemon per machine, watching a set of registered projects — not one daemon per repo. The pitch is "your AI agents' crontab", and crontab is per-user-per-machine; a single daemon is also the only shape under which one port (7373), one boot-service registration, and one web UI stay coherent.

## Considered Options

- **Per-project daemon** — simpler mental model ("start it in your repo"), but N projects means N processes, N launchd/systemd registrations, port conflicts, and N web UIs. Rejected.
- **Machine-global daemon** (chosen) — one supervised process; repos are registered as projects (`openroutine add <dir>`), each with a unique name defaulting to its basename.

## Consequences

- Task ids are namespaced `<project>/<filename stem>`, so identically named files in different projects can never collide.
- One config (`~/.config/openroutine/config.toml`) and one state root (`~/.local/state/openroutine/`) serve the whole machine.
- Per-repo artifacts (`CRONTAB.md`) are generated once per project, each listing only that project's tasks.
