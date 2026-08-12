# 13 — Web UI: read views + cookie auth

**What to build:** The local, no-account answer to claude.ai/code/routines and ChatGPT's Scheduled page — read side. All assets embed in the binary (no CDN, no runtime Node, works offline). `openroutine open` launches the browser at a one-time tokenized URL that sets a session cookie; manual token paste is the fallback; everything stays authenticated. Views: every Task across every Project with description, schedule, and next fire — One-shots as a countdown/absolute time (never presented as recurring), Manual as "no schedule", Broken with its error, newly-flagged Tasks marked, Completed hidden behind a toggle; per-task Run history with full logs and Skips; the file path shown for editing, because the UI never authors — files are the only authoring surface, permanently.

**Blocked by:** 10 — REST API core + bearer auth.

**Status:** ready-for-agent

- [ ] The UI loads with zero network access beyond the daemon itself (fresh profile, offline)
- [ ] `open` authenticates via the one-time URL → cookie; the raw bearer token never persists in browser storage; unauthenticated visits get the paste fallback
- [ ] The task list renders all Projects with correct schedule forms: countdown for One-shots, "no schedule" for Manual, error text for Broken, flags for newly discovered
- [ ] Completed One-shots appear only behind the include-completed toggle
- [ ] Run history shows status, trigger, scheduled vs actual times, and full log content; Skips appear with their reasons
- [ ] No create/edit affordances exist anywhere; each Task shows its file path instead
