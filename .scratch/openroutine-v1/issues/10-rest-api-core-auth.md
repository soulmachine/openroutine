# 10 — REST API core + bearer auth

**What to build:** The daemon listens on 127.0.0.1 (config override possible, loudly discouraged; token mandatory either way) with a bearer token generated on first `serve`, stored 0600 in the state dir, printable and rotatable via `openroutine token [--rotate]`. Every endpoint — reads included, since task lists leak prompts — requires the token. Surface: task list and task detail (schedule, next fire, recent Skips embedded), paged run history, run detail, plain-text log fetch, and the fire endpoint. Fire accepts an optional `text` payload (≤ 64 KB; 413 above) delivered after the Task's body inside the documented `<run-context>` wrapper with its fixed preamble — caller-supplied, informational, never instructions; fire on an already-running Task returns 409 with the active run id. Contract discipline throughout: structured error objects with proper status codes, null-vs-empty distinguished, `nextFireAt` genuinely null for Manual and Completed Tasks.

**Blocked by:** 03 — Scheduling semantics core; 04 — Schedule forms.

**Status:** ready-for-agent

- [ ] Every endpoint returns 401 without the token; `token` prints it and `--rotate` invalidates the old one
- [ ] Firing a Manual Task via curl with `text` starts a Run whose prompt ends with the wrapped payload after the unmodified body; the response carries the run id and log path
- [ ] A >64 KB `text` gets 413; fire during an active Run gets 409 naming that run id; the Run still never overlaps itself
- [ ] Task detail embeds recent Skips and a `nextFireAt` that is null for Manual/Completed and populated (jittered) for cron Tasks
- [ ] Run history pages correctly and log fetch returns the exact `output.log` bytes
- [ ] Errors are structured objects with accurate HTTP status codes; empty collections are `[]`, unknown values are null
