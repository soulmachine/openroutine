# 11 — Operational controls: pause, cancel, SSE, `run`

**What to build:** The runaway-day toolkit. Per-task pause and resume endpoints flip the state-file toggle (a Task fires only when neither Disabled in frontmatter nor Paused in state). A persisted global pause — CLI `pause --all` / `resume --all`, a daemon-wide endpoint, honored env var at serve start — stops all firing while daemon, API, and (later) UI stay up; it survives restarts and is surfaced prominently in `list` and `status`. A running Run can be canceled via the API (same process-group kill discipline as timeouts; distinct recorded outcome), enabling deliberate cancel-then-fire. An SSE endpoint live-tails any Run's log. CLI `run <task>` (without `--dry-run`) fires through the API, streams until completion, explains 409s with the active run id, and — with the daemon down — errors clearly and suggests `serve`.

**Blocked by:** 10 — REST API core + bearer auth.

**Status:** ready-for-agent

- [ ] A Paused Task's due Ticks are skipped and recorded; resume restores firing; frontmatter `disabled` and state pause layer independently
- [ ] Global pause stops every Task at once, survives a daemon restart, maps from the env var at serve start, and shows in `list`/`status`
- [ ] Canceling an active Run kills its whole process group promptly and records a canceled outcome distinct from `timed-out`/`failed`
- [ ] The SSE stream delivers appended log bytes of an active Run as they happen and closes on completion
- [ ] `openroutine run` on an idle Task streams the Run and exits with its outcome; on a busy Task explains the 409; with no daemon, suggests `serve`
- [ ] Cancel-then-fire works as the deliberate overlap override
- [ ] Carried from the ticket-01 review: the state entry's `enabled` field is the **Paused** concept under a name the glossary avoids (`CONTEXT.md` reserves Disabled for the frontmatter layer). Settle the field name here, where pause becomes real behaviour
- [ ] Carried from the ticket-01 review: with `run` able to Fire on demand, add the end-to-end case ticket 01 could not — drive a full Run *through the spawned binary* and assert the stub Agent's argv, cwd, and the resulting Run artifacts
