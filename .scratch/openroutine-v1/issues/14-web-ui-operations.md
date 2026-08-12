# 14 — Web UI: operations

**What to build:** The operate half of the dashboard, completing the 1.0 trinity. Run-now (fire) with the 409-explained busy case, pause and resume per Task, cancel on an active Run, and the global-pause toggle with a prominent everything-is-paused banner — all against the existing API with the session cookie, with live log tailing via the SSE endpoint so a just-fired Run streams in place.

**Blocked by:** 11 — Operational controls; 13 — Web UI: read views + cookie auth.

**Status:** resolved

- [x] Run-now on an idle Task starts a Run whose log streams live in the UI; on a busy Task the active run id is shown, with cancel offered
- [x] Pause/resume per Task reflect immediately in the list and in `scheduled-tasks.json`
- [x] The global-pause toggle stops all firing, shows a persistent banner, and survives a daemon restart
- [x] Cancel from the UI ends the Run promptly and history records the canceled outcome
- [x] Every operation carries the session's auth; an expired session degrades to the login flow, not to silent failures
- [x] With 13 + 14 together, every control the vendors' dashboards offer for a task — inspect, history, logs, run, pause, resume, cancel — works locally with no account

## Comments

**Delivered.** 174 tests; clippy and rustfmt clean.

Run-now, pause and resume per Task, cancel on a Run in flight, and the global pause toggle all sit on the endpoints ticket 11 built, carried by the session cookie. A just-fired Run streams into the page over SSE and stops when the Run ends. The global pause shows as a banner across the top, because a machine where nothing will fire should say so before you wonder why.

An expired session degrades to a message telling you to run `openroutine open` again, rather than failing silently.

With 13 and 14 together, every control the vendors' dashboards offer for a task — inspect, history, logs, run, pause, resume, cancel — works locally against your own files, with no account anywhere. That is the last of the fourteen tickets.
