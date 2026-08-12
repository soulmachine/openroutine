# 08 — Durability: single instance, atomic state, recovery, retention

**What to build:** The daemon earns "honest bookkeeping" under failure. A second `serve` against the same state dir exits with a clear error (lock-based single instance). Every state write is atomic (temp file + rename), so a crash mid-write never leaves a torn file. An unparseable state file on load is renamed aside with a timestamped suffix and regenerated fresh, with a loud warning — the state file is disposable by design: history lost, Tasks untouched. Run retention keeps the last 50 Runs per Task (configurable globally and per Project), pruning after each run. The Daemon writes its own diagnostics to a log file in the state dir and to stderr.

**Blocked by:** 01 — Walking skeleton.

**Status:** ready-for-agent

- [ ] A second `serve` refuses to start, names the running instance's lock, and exits non-zero
- [ ] Killing the daemon (SIGKILL) mid-operation never produces an unparseable state file — it's either the old or the new content
- [ ] A hand-corrupted state file is renamed aside on startup, a fresh one is created, a loud warning is logged, and every Task on disk still schedules
- [ ] The 51st Run of a Task prunes the oldest; the cap is configurable
- [ ] Daemon diagnostics appear both on stderr and in the state-dir log file
- [ ] A Run in flight when the daemon dies is recorded as `interrupted` on the next startup
