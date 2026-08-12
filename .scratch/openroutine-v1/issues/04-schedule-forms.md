# 04 — Schedule forms: `at:` one-shots, Manual, `catch_up`

**What to build:** The two non-cron schedule forms and the catch-up story. A Task with `at: <RFC 3339>` is a One-shot: it fires once at its (jittered) time, then becomes **Completed** in state — recorded atomically as part of firing and keyed to the `at` value, so a missed watcher event or restart can never re-fire it; editing `at:` to a new time re-arms the Task; `cron:` and `at:` together is Broken. A Task with neither is **Manual**: valid, fireable on demand, never ticked, showing "no schedule" and a genuinely null next-fire (no sentinel dates). `catch_up: true` fires one run for the most recent missed tick within a 7-day lookback at Daemon startup (for One-shots: runs once if its time passed unrun); the default remains skip-only.

**Blocked by:** 03 — Scheduling semantics core.

**Status:** resolved

- [x] An `at:` Task fires exactly once across arbitrary daemon restarts around its fire time, and state shows it Completed keyed to that timestamp
- [x] Editing the `at:` value re-arms the Task; it fires once at the new time
- [x] A Task with both `cron:` and `at:` is Broken; with neither it is a healthy Manual Task
- [x] Manual and Completed Tasks report a null next-fire everywhere — never a placeholder date
- [x] With `catch_up: true`, startup after downtime runs the single most recent miss (within 7 days) and records older misses as Skips; without it, skip-only
- [x] `list` renders One-shots with their absolute time and Manual Tasks as "no schedule"

## Comments

**Delivered.** 137 tests; clippy and rustfmt clean.

`Schedule` is now the three forms the design always described — a cron expression, a single `at:` moment, or nothing at all — rather than a cron wrapper with the other two implied. A Task naming both is Broken; a Task naming neither is Manual: healthy, listed as "no schedule", never ticked, waiting to be Fired.

A One-shot fires exactly once. State remembers the moment it answered, so restarting the daemon before, during, or after that instant changes nothing — a test drives five restarts around it and still sees one Run. Editing `at:` to a new moment gives the Task something to do again, because completion is keyed to the value, not to a flag.

`catch_up: true` earns exactly one Run for the most recent missed Tick, if it is less than a week old; everything else missed stays recorded as a Skip. Without it, a moment that passed unattended is recorded and the Task is Completed unrun — no catch-up, as the design has said since Q5.

Sequencing note: this ticket was implemented after 05–08 rather than in number order. Ticket 09's `--dry-run` criteria describe One-shot and Manual Tasks, so those concepts had to exist first; the frontier was wider than I had been tracking.
