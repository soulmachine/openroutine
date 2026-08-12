# 03 — Scheduling semantics core: jitter, DST, Skips

**What to build:** The scheduling behavior that distinguishes OpenRoutine from naive cron, all in the pure clock-seamed core. Fire times are offset by deterministic per-task jitter — derived from the Task id, window up to 5 minutes, capped at half the interval for frequent tasks, `jitter: 0` for exact ticks — and Runs record both scheduled and actual start. Cron evaluates in host-local time with pinned DST rules: wall-times skipped by spring-forward fire at the next valid instant; repeated fall-back times fire once. Every due Tick becomes exactly one Run or one recorded Skip: a Tick due while the previous Run is active becomes a per-tick Skip with reason `overlap`; ticks missed while the Daemon was down collapse at startup into one Skip entry per outage window (`from`, `to`, count, reason `daemon-down`), computed from the last-scheduled pointer. Skip lists are capped per Task.

**Blocked by:** 01 — Walking skeleton.

**Status:** ready-for-agent

- [ ] The same Task id always yields the same jitter offset; `jitter: 0` fires exactly on the tick; the offset never exceeds half the interval for frequent tasks
- [ ] Table-driven pure-core tests cover a spring-forward night (skipped wall-time fires at next valid instant) and a fall-back night (repeated wall-time fires once)
- [ ] With a long-running stub Agent, an overlapping Tick is skipped and recorded with reason `overlap`, and the Task never runs concurrently with itself
- [ ] Restarting after simulated downtime produces exactly one collapsed `daemon-down` Skip entry carrying the window and missed count
- [ ] Skip records per Task are capped; the newest survive
- [ ] Run metadata distinguishes scheduled tick from actual (jittered) start
- [ ] Carried from the ticket-01 review: `next_fire_after` is strictly-after, so a reload landing exactly on a Tick instant drops that Tick. Decide whether it fires or is recorded as a Skip, and pin it with a case
