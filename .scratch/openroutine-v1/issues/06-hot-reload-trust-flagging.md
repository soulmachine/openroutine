# 06 — Hot reload & trust flagging

**What to build:** Add or edit a `.cron.md` and the schedule updates immediately — no reinstall step — with watcher events treated as a latency optimization over a periodic reconciling rescan, never as the source of truth. Mid-run file changes behave predictably: an edit lets the active Run finish under the old definition, next Tick uses the new one; a delete lets the Run finish and be logged, then unschedules the Task and prunes its state entry (Run logs retained). The trust model is enforced as visibility: newly discovered Tasks — and definition changes to existing ones — are flagged loudly in `list` and the daemon log ("registering a Project means trusting its committers"), never blocked.

**Blocked by:** 02 — `list`, required `description:`, and Broken.

**Status:** ready-for-agent

- [ ] Dropping a new Task file schedules it without a restart; editing its cron takes effect from the next Tick
- [ ] Editing a Task mid-run leaves the active Run on the old definition; the next Run uses the new one
- [ ] Deleting a Task mid-run lets the Run complete and be recorded, then unschedules and prunes state while its Run logs remain
- [ ] A missed watcher event is healed by the reconciling rescan — the schedule converges to what's on disk
- [ ] A newly discovered or changed Task is visibly flagged in `list` and the daemon log, and still auto-schedules
- [ ] Flagging clears once the Task has been seen (no permanent noise)
