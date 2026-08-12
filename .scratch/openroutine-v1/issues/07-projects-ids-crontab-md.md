# 07 — Multi-Project, namespaced ids, `CRONTAB.md`

**What to build:** The one-daemon-many-repos experience (ADR-0001). `openroutine add <dir>` / `remove <dir>` register Projects in the config as human-owned entries; each Project carries a unique name (default: directory basename, overridable), and every Task id is `<project>/<name>` — so identically named files in different repos never collide; bare names stay accepted in the CLI where unambiguous. Discovery scans each Project recursively for `*.cron.md`, honoring `.gitignore`, always skipping `.git/`, and never following symlinks out of the Project. A deterministic `CRONTAB.md` is generated at each Project root — a table of that Project's tasks (description, schedule, agent), derived from definitions alone so it changes only when a definition changes and is safe to commit — with a per-project opt-out, and writes that refuse symlinked targets.

**Blocked by:** 02 — `list`, required `description:`, and Broken.

**Status:** ready-for-agent

- [ ] Two registered Projects each containing `nightly.cron.md` both schedule, with distinct `<project>/nightly` ids
- [ ] `add` errors on a duplicate Project name and suggests overriding; `remove` unschedules that Project's Tasks
- [ ] A task file matched by `.gitignore` or under `.git/` is not discovered; a symlink pointing outside the Project is not followed
- [ ] `CRONTAB.md` appears at each Project root, lists exactly that Project's tasks, and is byte-identical across regenerations with unchanged definitions
- [ ] Run state changes never touch `CRONTAB.md`; a definition edit regenerates it
- [ ] The per-project opt-out suppresses generation; a symlinked `CRONTAB.md` target is refused loudly
- [ ] Carried from the ticket-02 review: a registered Project whose directory is missing or unreadable currently yields an empty listing with only a log line. Registering the Project is the moment to validate it, and `list` should say the Project is unreachable rather than imply it is empty
