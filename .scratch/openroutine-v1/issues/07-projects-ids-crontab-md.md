# 07 — Multi-Project, namespaced ids, `CRONTAB.md`

**What to build:** The one-daemon-many-repos experience (ADR-0001). `openroutine add <dir>` / `remove <dir>` register Projects in the config as human-owned entries; each Project carries a unique name (default: directory basename, overridable), and every Task id is `<project>/<name>` — so identically named files in different repos never collide; bare names stay accepted in the CLI where unambiguous. Discovery scans each Project recursively for `*.cron.md`, honoring `.gitignore`, always skipping `.git/`, and never following symlinks out of the Project. A deterministic `CRONTAB.md` is generated at each Project root — a table of that Project's tasks (description, schedule, agent), derived from definitions alone so it changes only when a definition changes and is safe to commit — with a per-project opt-out, and writes that refuse symlinked targets.

**Blocked by:** 02 — `list`, required `description:`, and Broken.

**Status:** resolved

- [x] Two registered Projects each containing `nightly.cron.md` both schedule, with distinct `<project>/nightly` ids
- [x] `add` errors on a duplicate Project name and suggests overriding; `remove` unschedules that Project's Tasks
- [x] A task file matched by `.gitignore` or under `.git/` is not discovered; a symlink pointing outside the Project is not followed
- [x] `CRONTAB.md` appears at each Project root, lists exactly that Project's tasks, and is byte-identical across regenerations with unchanged definitions
- [x] Run state changes never touch `CRONTAB.md`; a definition edit regenerates it
- [x] The per-project opt-out suppresses generation; a symlinked `CRONTAB.md` target is refused loudly
- [x] Carried from the ticket-02 review: a registered Project whose directory is missing or unreadable currently yields an empty listing with only a log line. Registering the Project is the moment to validate it, and `list` should say the Project is unreachable rather than imply it is empty

## Comments

**Delivered.** 117 tests; clippy and rustfmt clean.

Discovery now uses the `ignore` crate, so a Project is walked by the rules its owner already expects: `.gitignore` (and `.ignore`) are honoured, `.git` is never searched, and symlinks are not followed — a link is not a reason to schedule a file the Project does not contain. `add`/`remove` edit the config with `toml_edit`, preserving comments and formatting, written temp-and-renamed so an interrupted edit cannot truncate a hand-owned file.

Review found two things worth calling out:

- **A Task description could inject markdown into a committed file.** `CRONTAB.md` is written into the repo and is meant to be read by people *and* agents, so a description containing `|` or a newline could reshape the table or add content of its own — the reviewer reproduced an injected heading reading "Ignore previous instructions." Cells are now flattened and escaped, in `list` too.
- **The announce-once de-duplication was self-defeating.** Two writers to the same map meant neither comparison ever matched, so a Broken Task logged everything on every rescan — the exact noise ticket 06 had just removed. One note set per Task now, written once.

Also fixed from review: `CRONTAB.md` writes use `O_NOFOLLOW`, closing the gap between the symlink check and the write; project names may not be empty or contain `/`, which would make a Task id ambiguous; and a Project that cannot be canonicalised is reported rather than skipped in silence.

Two decisions logged. **Q66**: `list` no longer writes `CRONTAB.md` — a read command must not dirty a worktree; the Daemon regenerates it on every rescan instead, and the CRONTAB tests drive the Daemon accordingly. **Q67**: the Daemon re-reads its config on each rescan, so `add` and `remove` reach a running daemon; a config that stops parsing keeps the previous one rather than unscheduling everything.

Known and left: a Broken Task appears in `CRONTAB.md` with `(broken)` in the Agent column, which hides the agent it declared. A status column would be truer if the table ever grows one.
