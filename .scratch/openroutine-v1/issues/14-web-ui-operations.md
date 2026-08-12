# 14 — Web UI: operations

**What to build:** The operate half of the dashboard, completing the 1.0 trinity. Run-now (fire) with the 409-explained busy case, pause and resume per Task, cancel on an active Run, and the global-pause toggle with a prominent everything-is-paused banner — all against the existing API with the session cookie, with live log tailing via the SSE endpoint so a just-fired Run streams in place.

**Blocked by:** 11 — Operational controls; 13 — Web UI: read views + cookie auth.

**Status:** ready-for-agent

- [ ] Run-now on an idle Task starts a Run whose log streams live in the UI; on a busy Task the active run id is shown, with cancel offered
- [ ] Pause/resume per Task reflect immediately in the list and in `scheduled-tasks.json`
- [ ] The global-pause toggle stops all firing, shows a persistent banner, and survives a daemon restart
- [ ] Cancel from the UI ends the Run promptly and history records the canceled outcome
- [ ] Every operation carries the session's auth; an expired session degrades to the login flow, not to silent failures
- [ ] With 13 + 14 together, every control the vendors' dashboards offer for a task — inspect, history, logs, run, pause, resume, cancel — works locally with no account
