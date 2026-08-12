# 08 — Durability: single instance, atomic state, recovery, retention

**What to build:** The daemon earns "honest bookkeeping" under failure. A second `serve` against the same state dir exits with a clear error (lock-based single instance). Every state write is atomic (temp file + rename), so a crash mid-write never leaves a torn file. An unparseable state file on load is renamed aside with a timestamped suffix and regenerated fresh, with a loud warning — the state file is disposable by design: history lost, Tasks untouched. Run retention keeps the last 50 Runs per Task (configurable globally and per Project), pruning after each run. The Daemon writes its own diagnostics to a log file in the state dir and to stderr.

**Blocked by:** 01 — Walking skeleton.

**Status:** resolved

- [x] A second `serve` refuses to start, names the running instance's lock, and exits non-zero
- [x] Killing the daemon (SIGKILL) mid-operation never produces an unparseable state file — it's either the old or the new content
- [x] A hand-corrupted state file is renamed aside on startup, a fresh one is created, a loud warning is logged, and every Task on disk still schedules
- [x] The 51st Run of a Task prunes the oldest; the cap is configurable
- [x] Daemon diagnostics appear both on stderr and in the state-dir log file
- [x] A Run in flight when the daemon dies is recorded as `interrupted` on the next startup

## Comments

**Delivered.** 125 tests; clippy and rustfmt clean.

The daemon now holds an exclusive lock for its whole life, so a second `serve` refuses with the first one's pid and lock path rather than racing it. State is written beside itself and renamed over, and a test kills the daemon with SIGKILL six times over, parsing the state file after each — it is always whole. A state file that cannot be parsed is set aside under a timestamped name and a fresh one takes its place: history is lost, Tasks are not. Runs are capped per Task, globally or per Project, and the daemon keeps its own log beside its state.

Review caught a bug this ticket shipped into the commit itself: when the config failed to load, `serve` created **`daemon.log` in whatever directory it was started from** — the staged diff included one at the repo root. The log layer is now built only after the config says where the state lives.

Other review fixes: `run.json` is written atomically too, since it is state like any other and this ticket's own promise covers it; recovery, pruning, and lock failures all say what happened instead of returning silently; `flock` distinguishes "someone else holds it" from a filesystem that cannot lock at all, rather than reporting the same confident wrong answer either way; run-id collision suffixes are zero-padded so `-010` still sorts after `-002`, because name order is what age order relies on; and a retention cap of zero is refused at load, since it would delete the Run that is starting.

Two decisions logged. **Q68**: recovering interrupted Runs is an explicit step the lock holder performs, not something a scan does — the inference "still running means abandoned" is only true under the lock, and the code now says so. **Q69**: `State::load` never writes; quarantining damaged state belongs to the Daemon, under its lock, so `list` cannot rename files in the state directory.

Not covered: the atomicity claim holds for a killed process, not for power loss — nothing is fsynced. Worth revisiting only if someone actually runs this somewhere that matters.
