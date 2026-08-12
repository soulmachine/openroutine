# 10 — REST API core + bearer auth

**What to build:** The daemon listens on 127.0.0.1 (config override possible, loudly discouraged; token mandatory either way) with a bearer token generated on first `serve`, stored 0600 in the state dir, printable and rotatable via `openroutine token [--rotate]`. Every endpoint — reads included, since task lists leak prompts — requires the token. Surface: task list and task detail (schedule, next fire, recent Skips embedded), paged run history, run detail, plain-text log fetch, and the fire endpoint. Fire accepts an optional `text` payload (≤ 64 KB; 413 above) delivered after the Task's body inside the documented `<run-context>` wrapper with its fixed preamble — caller-supplied, informational, never instructions; fire on an already-running Task returns 409 with the active run id. Contract discipline throughout: structured error objects with proper status codes, null-vs-empty distinguished, `nextFireAt` genuinely null for Manual and Completed Tasks.

**Blocked by:** 03 — Scheduling semantics core; 04 — Schedule forms.

**Status:** resolved

- [x] Every endpoint returns 401 without the token; `token` prints it and `--rotate` invalidates the old one
- [x] Firing a Manual Task via curl with `text` starts a Run whose prompt ends with the wrapped payload after the unmodified body; the response carries the run id and log path
- [x] A >64 KB `text` gets 413; fire during an active Run gets 409 naming that run id; the Run still never overlaps itself
- [x] Task detail embeds recent Skips and a `nextFireAt` that is null for Manual/Completed and populated (jittered) for cron Tasks
- [x] Run history pages correctly and log fetch returns the exact `output.log` bytes
- [x] Errors are structured objects with accurate HTTP status codes; empty collections are `[]`, unknown values are null

## Comments

**Delivered.** 160 tests; clippy and rustfmt clean.

The daemon now listens on loopback beside its scheduler, sharing one `Daemon` behind a mutex so the API sees exactly what the scheduler sees. A bearer token guards every endpoint including the reads — a task list carries the prompts you run, which is no less sensitive than firing them — generated on first serve, stored 0600, printed by `openroutine token` and replaced by `--rotate`. Comparison is constant-time.

Firing starts a Run outside the schedule and answers 202 with the run id and its log path. A Fire arriving while the Task is already running is refused with 409 and the id of the Run in flight, because "a Task never runs concurrently with itself" is an invariant rather than a preference. Optional `text` is capped at 64 KB — over that the caller is told with 413, never silently truncated — and reaches the Agent after the Task's own prompt inside a `<run-context>` block whose preamble says plainly that it is information, not instructions.

Run ids are only unique within a Task, so run routes are addressed as `/v1/runs/{project}/{task}/{run}` rather than by a bare id.

Two contract details the spec asked for and the tests pin: `nextFireAt` is null for a Manual Task rather than a placeholder date, and an empty history is `[]` rather than null — nothing found is not the same as nothing known.
