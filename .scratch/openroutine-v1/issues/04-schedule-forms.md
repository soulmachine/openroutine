# 04 — Schedule forms: `at:` one-shots, Manual, `catch_up`

**What to build:** The two non-cron schedule forms and the catch-up story. A Task with `at: <RFC 3339>` is a One-shot: it fires once at its (jittered) time, then becomes **Completed** in state — recorded atomically as part of firing and keyed to the `at` value, so a missed watcher event or restart can never re-fire it; editing `at:` to a new time re-arms the Task; `cron:` and `at:` together is Broken. A Task with neither is **Manual**: valid, fireable on demand, never ticked, showing "no schedule" and a genuinely null next-fire (no sentinel dates). `catch_up: true` fires one run for the most recent missed tick within a 7-day lookback at Daemon startup (for One-shots: runs once if its time passed unrun); the default remains skip-only.

**Blocked by:** 03 — Scheduling semantics core.

**Status:** ready-for-agent

- [ ] An `at:` Task fires exactly once across arbitrary daemon restarts around its fire time, and state shows it Completed keyed to that timestamp
- [ ] Editing the `at:` value re-arms the Task; it fires once at the new time
- [ ] A Task with both `cron:` and `at:` is Broken; with neither it is a healthy Manual Task
- [ ] Manual and Completed Tasks report a null next-fire everywhere — never a placeholder date
- [ ] With `catch_up: true`, startup after downtime runs the single most recent miss (within 7 days) and records older misses as Skips; without it, skip-only
- [ ] `list` renders One-shots with their absolute time and Manual Tasks as "no schedule"
