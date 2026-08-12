# 02 — `list`, required `description:`, and Broken

**What to build:** `openroutine list` reads Task files and state straight from disk — working whether or not the Daemon runs — and shows every Task with its id, description, schedule, and status. The full v1 validation story lands here: `description:` is required; a missing description, an invalid cron string, an unknown agent name, or unparseable frontmatter makes the Task **Broken** — visible in `list` with its exact error, never firing, its schedule stopped. Unknown frontmatter keys warn (surfaced in `list`) but the Task still runs.

**Blocked by:** 01 — Walking skeleton.

**Status:** resolved

- [x] `list` works with the Daemon stopped, from disk alone
- [x] A Task missing `description:` shows as Broken with an error naming the missing field, and never fires
- [x] An invalid cron string or unknown agent shows as Broken with the parse/validation error verbatim
- [x] A Broken Task that is fixed on disk returns to healthy on the next scan
- [x] An unknown frontmatter key produces a visible warning in `list` while the Task keeps firing
- [x] Healthy rows show id, description, schedule, and next fire time

## Comments

**Delivered.** 44 tests; clippy and rustfmt clean. `list` prints a STATUS column, keeps the author's description on Broken rows, and carries per-Task warnings and errors as indented detail lines under each row.

Three things worth knowing downstream:

- **A malformed Agent template now makes its Tasks Broken** (logged as Q58). An unterminated quote, an empty template, or `{prompt}` in the program position used to fail at fire time with nothing on disk; it is caught when the Task is scanned instead.
- **Warnings distinguish "unknown" from "not implemented yet."** Keys in the v1 schema that this build ignores — `disabled`, `timeout`, `at`, `env`, `cwd`, `jitter`, `model`, `permission_mode`, `catch_up`, plus reserved `tz`/`on_failure` — say so explicitly. Calling `disabled: true` an unknown key while the Task fires anyway was actively misleading. Each ticket that implements one should drop it from `NOT_YET_HONOURED` in `src/task.rs`.
- **The test suite now sits entirely on the two seams `spec.md` permits** (logged as Q59): `tests/task_parsing.rs` and `tests/agent_command.rs` are gone, their coverage re-expressed through `list` and through real Runs. The argv-safety cases got stronger in the move — a prompt full of shell syntax is proven inert against an actual spawn, not against a string builder.

`default_agent` config resolution landed here because deciding whether an omitted `agent:` makes a Task Broken requires it; both review axes judged that in scope rather than creep.
