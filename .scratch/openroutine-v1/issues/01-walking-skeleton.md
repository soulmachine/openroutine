# 01 — Walking skeleton: file → tick → run → record

**What to build:** The narrowest complete path through the whole product. A user writes one `.cron.md` (description, cron, agent, prompt body) in a configured Project, starts `openroutine serve`, and when the cron expression comes due the configured Agent command template runs with the prompt — substituted for `{prompt}` as a single argument, or piped to stdin when the template has no placeholder — and the Run is recorded on disk: a per-run directory holding a metadata file (status, exit code, trigger, scheduled and actual times) and a merged `output.log` opening with a structured header, plus updated latest-run pointers in `scheduled-tasks.json`. Happy path only: one Project, cron schedules only, no jitter/skips/hot-reload yet.

This ticket also establishes the repo's two test seams, which every later ticket builds on: (1) the process-boundary harness — integration tests spawn the real binary against temp XDG config/state dirs and a temp Project, with stub Agent templates (scripts recording argv, env, cwd, stdin and emitting chosen output/exit codes/delays); (2) the injected clock — the daemon reads time through a single clock source, and due-tick computation lives in a pure library core testable against synthetic clocks.

**Blocked by:** None — can start immediately.

**Status:** resolved

- [x] A cron Task in a configured Project fires at its tick (driven via the clock seam in tests) and the stub Agent receives the exact prompt body
- [x] `{prompt}` templates receive the body as one argv element; templates without a placeholder receive it on stdin
- [x] The Run directory exists with metadata (status `succeeded`/`failed`, exit code, trigger `schedule`, scheduled + actual timestamps) and header-prefixed `output.log`
- [x] `scheduled-tasks.json` carries the Task's id, absolute file path, and latest-run pointers; timestamps are UTC
- [x] Integration tests run the real binary in sandboxed temp dirs, offline, leaving no files outside them
- [x] The scheduling core is a pure surface with no wall-clock reads; nothing in the daemon calls system time directly except through the injected clock

## Comments

**Delivered.** 34 tests across five files; clippy and rustfmt clean. Two decisions worth knowing downstream:

- **The zone is injected alongside the clock** (`Daemon::with_zone`, `src/zone.rs`, chrono-tz + iana-time-zone). `Daemon::new` still resolves the host zone, so behaviour is unchanged — but a hardcoded local zone would have made ticket 03's DST cases untestable, so the seam had to exist from the start.
- **Runs are started, not awaited, by the scheduler.** `tick()` records the Run as `running` on disk, spawns the Agent, and returns; a background task finalizes `run.json`. Review found that awaiting inline made the Daemon ignore SIGTERM for the length of a Run and stall every other Task — unacceptable for a supervised daemon (ticket 12) and wrong against unlimited concurrency (`spec.md`).

Known gap, deliberately not closed here: nothing drives a full Run *through the spawned binary*, because the only trigger in this slice is the schedule and a wall-clock scheduling test is forbidden by `spec.md` §Testing Decisions. Ticket 09/11 close it — see the note added there.
