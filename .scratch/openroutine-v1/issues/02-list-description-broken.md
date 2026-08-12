# 02 — `list`, required `description:`, and Broken

**What to build:** `openroutine list` reads Task files and state straight from disk — working whether or not the Daemon runs — and shows every Task with its id, description, schedule, and status. The full v1 validation story lands here: `description:` is required; a missing description, an invalid cron string, an unknown agent name, or unparseable frontmatter makes the Task **Broken** — visible in `list` with its exact error, never firing, its schedule stopped. Unknown frontmatter keys warn (surfaced in `list`) but the Task still runs.

**Blocked by:** 01 — Walking skeleton.

**Status:** ready-for-agent

- [ ] `list` works with the Daemon stopped, from disk alone
- [ ] A Task missing `description:` shows as Broken with an error naming the missing field, and never fires
- [ ] An invalid cron string or unknown agent shows as Broken with the parse/validation error verbatim
- [ ] A Broken Task that is fixed on disk returns to healthy on the next scan
- [ ] An unknown frontmatter key produces a visible warning in `list` while the Task keeps firing
- [ ] Healthy rows show id, description, schedule, and next fire time
