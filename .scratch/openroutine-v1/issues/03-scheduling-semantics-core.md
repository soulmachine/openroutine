# 03 — Scheduling semantics core: jitter, DST, Skips

**What to build:** The scheduling behavior that distinguishes OpenRoutine from naive cron, all in the pure clock-seamed core. Fire times are offset by deterministic per-task jitter — derived from the Task id, window up to 5 minutes, capped at half the interval for frequent tasks, `jitter: 0` for exact ticks — and Runs record both scheduled and actual start. Cron evaluates in host-local time with pinned DST rules: wall-times skipped by spring-forward fire at the next valid instant; repeated fall-back times fire once. Every due Tick becomes exactly one Run or one recorded Skip: a Tick due while the previous Run is active becomes a per-tick Skip with reason `overlap`; ticks missed while the Daemon was down collapse at startup into one Skip entry per outage window (`from`, `to`, count, reason `daemon-down`), computed from the last-scheduled pointer. Skip lists are capped per Task.

**Blocked by:** 01 — Walking skeleton.

**Status:** resolved

- [x] The same Task id always yields the same jitter offset; `jitter: 0` fires exactly on the tick; the offset never exceeds half the interval for frequent tasks
- [x] Table-driven pure-core tests cover a spring-forward night (skipped wall-time fires at next valid instant) and a fall-back night (repeated wall-time fires once)
- [x] With a long-running stub Agent, an overlapping Tick is skipped and recorded with reason `overlap`, and the Task never runs concurrently with itself
- [x] Restarting after simulated downtime produces exactly one collapsed `daemon-down` Skip entry carrying the window and missed count
- [x] Skip records per Task are capped; the newest survive
- [x] Run metadata distinguishes scheduled tick from actual (jittered) start
- [x] Carried from the ticket-01 review: a Tick due at the reload instant now fires (Q60); a Tick whose jittered fire time hasn't arrived also survives a reload

## Comments

**Delivered.** 62 tests; clippy and rustfmt clean.

Review found the invariant "every Tick becomes exactly one Run or one Skip" was being broken in two places, and both are now closed:

- **A late scheduler used to lose Ticks.** Advancing jumped straight to the next Tick after *now*, so a suspended laptop or a long pass left the intervening Ticks with neither a Run nor a Skip. Advancing now walks Tick by Tick and records what it passed, under a new `missed` reason (Q61) — `overlap` and `daemon-down` both misdescribed it.
- **A Skipped Tick was counted twice.** Downtime was measured from the last *Run*, so a Tick already answered by an overlap Skip reappeared inside the next `daemon-down` entry. State now tracks `lastTickAt` — the last Tick answered, however it was answered — separately from the last Run's `lastScheduledFor`.

Also fixed from review: `parse_duration` used chrono's panicking constructors, so `jitter: 999999999999d` in a committed task file would have taken the whole Daemon down instead of marking one Task Broken; a truncated downtime count now says so in state rather than only in the log; and the `list` column showing a Tick is now labelled `NEXT TICK` rather than `NEXT FIRE`.

Two notes for later tickets:

- **`list` deliberately shows the Tick, not the jittered fire time** — someone comparing a row against the cron expression they wrote should see the time they wrote. The actual start is in each Run's record, alongside the Tick it answered.
- **Downtime detection runs only on a Daemon's first scan.** Ticket 06 adds periodic rescans; the `missed` path above is what covers Ticks lost while running, so the two must stay distinct.
