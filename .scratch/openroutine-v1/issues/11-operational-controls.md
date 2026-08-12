# 11 — Operational controls: pause, cancel, SSE, `run`

**What to build:** The runaway-day toolkit. Per-task pause and resume endpoints flip the state-file toggle (a Task fires only when neither Disabled in frontmatter nor Paused in state). A persisted global pause — CLI `pause --all` / `resume --all`, a daemon-wide endpoint, honored env var at serve start — stops all firing while daemon, API, and (later) UI stay up; it survives restarts and is surfaced prominently in `list` and `status`. A running Run can be canceled via the API (same process-group kill discipline as timeouts; distinct recorded outcome), enabling deliberate cancel-then-fire. An SSE endpoint live-tails any Run's log. CLI `run <task>` (without `--dry-run`) fires through the API, streams until completion, explains 409s with the active run id, and — with the daemon down — errors clearly and suggests `serve`.

**Blocked by:** 10 — REST API core + bearer auth.

**Status:** resolved

- [x] A Paused Task's due Ticks are skipped and recorded; resume restores firing; frontmatter `disabled` and state pause layer independently
- [x] Global pause stops every Task at once, survives a daemon restart, maps from the env var at serve start, and shows in `list`/`status`
- [x] Canceling an active Run kills its whole process group promptly and records a canceled outcome distinct from `timed-out`/`failed`
- [x] The SSE stream delivers appended log bytes of an active Run as they happen and closes on completion
- [x] `openroutine run` on an idle Task streams the Run and exits with its outcome; on a busy Task explains the 409; with no daemon, suggests `serve`
- [x] Cancel-then-fire works as the deliberate overlap override
- [x] Carried from the ticket-01 review: the state entry's `enabled` field is the **Paused** concept under a name the glossary avoids (`CONTEXT.md` reserves Disabled for the frontmatter layer). Settle the field name here, where pause becomes real behaviour
- [x] Carried from the ticket-01 review: with `run` able to Fire on demand, add the end-to-end case ticket 01 could not — drive a full Run *through the spawned binary* and assert the stub Agent's argv, cwd, and the resulting Run artifacts

## Comments

**Delivered.** 166 tests; clippy and rustfmt clean.

Pause is real now, at both scopes. A held Task does not fire, and the Tick that arrives while it is held becomes a Skip with reason `paused` rather than a gap — the invariant holds for pausing too. A global pause is recorded in state, so it survives a restart: a test pauses through one daemon, stops it, starts another, and finds everything still held. Firing a paused Task is refused with 409 rather than quietly queued.

Cancelling ends the Run's whole process group, the same way a timeout does, so the Agent's children go with it; the test proves it by showing the Task free to run again immediately afterwards. The log can be streamed over SSE while a Run is going, and `openroutine run <task>` now fires through the daemon rather than explaining that it cannot.

Both carried items are closed. The state field is now `paused` rather than `enabled`, matching `CONTEXT.md`, which reserves Disabled for the frontmatter layer that travels with the repository — the manifesto's state example is updated to match. And the end-to-end path through the spawned binary that ticket 01 could not reach is covered: the API tests fire a Task over HTTP against a real daemon and assert the Agent ran, the context arrived wrapped, and the log came back.
