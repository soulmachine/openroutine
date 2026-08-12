# 13 — Web UI: read views + cookie auth

**What to build:** The local, no-account answer to claude.ai/code/routines and ChatGPT's Scheduled page — read side. All assets embed in the binary (no CDN, no runtime Node, works offline). `openroutine open` launches the browser at a one-time tokenized URL that sets a session cookie; manual token paste is the fallback; everything stays authenticated. Views: every Task across every Project with description, schedule, and next fire — One-shots as a countdown/absolute time (never presented as recurring), Manual as "no schedule", Broken with its error, newly-flagged Tasks marked, Completed hidden behind a toggle; per-task Run history with full logs and Skips; the file path shown for editing, because the UI never authors — files are the only authoring surface, permanently.

**Blocked by:** 10 — REST API core + bearer auth.

**Status:** resolved

- [x] The UI loads with zero network access beyond the daemon itself (fresh profile, offline)
- [x] `open` authenticates via the one-time URL → cookie; the raw bearer token never persists in browser storage; unauthenticated visits get the paste fallback
- [x] The task list renders all Projects with correct schedule forms: countdown for One-shots, "no schedule" for Manual, error text for Broken, flags for newly discovered
- [x] Completed One-shots appear only behind the include-completed toggle
- [x] Run history shows status, trigger, scheduled vs actual times, and full log content; Skips appear with their reasons
- [x] No create/edit affordances exist anywhere; each Task shows its file path instead

## Comments

**Delivered.** 174 tests; clippy and rustfmt clean.

The page is compiled into the binary — one hand-written HTML file, one stylesheet, one vanilla ES module, no bundler and no npm. A strict `default-src 'self'` policy ships with every asset, so the no-CDN rule is enforced by the browser rather than merely intended, and a test asserts no `http://` or `https://` appears anywhere in the page, script, or styles.

`openroutine open` puts the token in the URL once; the daemon exchanges it for an `HttpOnly; SameSite=Strict` session cookie and redirects to `/`, so the long-lived secret never lands in history or browser storage. The API accepts that cookie alongside the bearer header — same secret, same origin, just the shape a browser can send.

The list shows every Task including Broken ones with their error, marks new and changed Tasks, renders a One-shot's moment as a countdown and a Manual Task as "no schedule", and hides Completed behind a toggle. Each Task shows the path of the file to edit; there is no editor anywhere, and the footer says so.

Per Q56, coverage stops at the process boundary: content types, the CSP header, the one-time link setting a cookie, 401 without one, and the shape of the data the page needs. DOM behaviour is checked by hand. I could not complete that check through browser automation here — the extension has no permission for `127.0.0.1` — so the page's rendering remains verified only by reading it and by a syntax check of the client.
