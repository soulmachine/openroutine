# 06 — Hot reload & trust flagging

**What to build:** Add or edit a `.cron.md` and the schedule updates immediately — no reinstall step — with watcher events treated as a latency optimization over a periodic reconciling rescan, never as the source of truth. Mid-run file changes behave predictably: an edit lets the active Run finish under the old definition, next Tick uses the new one; a delete lets the Run finish and be logged, then unschedules the Task and prunes its state entry (Run logs retained). The trust model is enforced as visibility: newly discovered Tasks — and definition changes to existing ones — are flagged loudly in `list` and the daemon log ("registering a Project means trusting its committers"), never blocked.

**Blocked by:** 02 — `list`, required `description:`, and Broken.

**Status:** resolved

- [x] Dropping a new Task file schedules it without a restart; editing its cron takes effect from the next Tick
- [x] Editing a Task mid-run leaves the active Run on the old definition; the next Run uses the new one
- [x] Deleting a Task mid-run lets the Run complete and be recorded, then unschedules and prunes state while its Run logs remain
- [x] A missed watcher event is healed by the reconciling rescan — the schedule converges to what's on disk
- [x] A newly discovered or changed Task is visibly flagged in `list` and the daemon log, and still auto-schedules
- [x] Flagging clears once the Task has been seen (no permanent noise)

## Comments

**Delivered.** 98 tests; clippy and rustfmt clean.

The schedule follows disk without a restart: a `notify` watcher makes reloads prompt, and a thirty-second rescan is what makes them certain — an event that is missed, coalesced, or unavailable costs latency, never correctness. A burst of changes (a branch checkout) settles into one reload.

Review caught a regression this ticket introduced, reproduced by the reviewer and now locked in by two tests (logged as **Q65**): because reload recomputed every Task's next Tick, a rescan landing between Ticks stepped over a pending one and recorded neither a Run nor a Skip. A reload now carries forward the pending Tick of any Task whose digest is unchanged, so only genuinely edited Tasks are re-planned.

Other review fixes: announcements are edge-triggered, so a never-run or Broken Task says its piece once per change instead of 2,880 times a day; pruning now asks "did a readable Project fail to produce this Task?" rather than probing the file, because a directory that merely could not be listed is not evidence of deletion (a test locks that down, and caught that `is_dir()` succeeds on an unreadable directory); the digest is taken from the same read that assessed the file, so an edit landing between two reads can no longer hide; and `list` degrades to empty state rather than dying on a damaged state file, which is supposed to be disposable.

A test-harness bug surfaced too: the stub Agent named its records with `mktemp`'s random suffix, so `calls()` returned them in arbitrary order. Records now carry a sequence number.

Trust flagging reads "seen" as "has started a Run in its current form" (**Q64**). A Task that can never run therefore stays flagged in `list` — honest, and no longer noisy in the log.

Not covered: nothing at the process boundary proves the periodic rescan heals a *dropped* watcher event, since there is no way to disable the watcher from outside. The rescan path is covered at the clock seam instead.
