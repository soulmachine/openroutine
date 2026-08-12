# Agent instructions

## The project, in one screen

OpenRoutine is a single Rust binary whose daemon is the scheduler, the REST API, and the local
web UI. A **Task** is one markdown file — frontmatter for metadata, body as the prompt —
**Registered** by absolute path in `tasks = [...]` in `~/.config/openroutine/config.toml`.
Nothing is discovered: there is no directory registration, no scanning, no filename rule, and
no file watcher.

Five things worth knowing before you change anything:

- **Identity is derived.** The frontmatter `name` is prose; the **Id** comes from it by
  lowercasing, turning whitespace runs into hyphens, dropping anything outside `[a-z0-9_-]`,
  and trimming the ends. The Id is what the state file, `runs/<id>/`, the API paths, and the
  CLI all use. Uniqueness is checked on the Id, never the name.
- **Definitions change at two moments, both narrow.** A **Reload** re-reads the config and
  every registered file — startup, `openroutine reload`, `POST /v1/reload`, or the automatic
  one after `add`/`remove`. A **Refresh** re-reads *one* file on the run path, as that Task is
  about to run. Nothing is watched and nothing is polled.
- **Runs are bounded by silence, not duration.** There is no cap on how long a Run may take;
  it is ended after `idle_timeout` (config, machine-wide, default 15m) without output.
- **Every Tick becomes exactly one Run or one Skip.** That invariant is load-bearing — check
  it before changing anything in `src/daemon.rs`.
- **Broken is visible, never silent.** A Task that cannot run is still listed, with its error.

Read `CONTEXT.md` for the vocabulary before writing prose or naming anything, and consult
`DECISIONS.md` before reopening a settled question — most "open" questions are answered there.
`.scratch/openroutine-v1/spec.md` is the full design; `docs/adr/` holds the two architectural
decisions.

## After installing a new binary: verify the Messages permission, don't assume it

**Every time you replace the installed binary — `cargo install --path . --force` — check that
openroutine can still drive Messages, while the user is at the keyboard.** Tasks on this machine
send iMessages through `osascript`, and a lost grant means the last step of an unattended Run
hangs or is denied at whatever hour it fires, with nobody there to click anything.

Two facts decide whether a rebuild costs you the grant, and they pull in opposite directions:

- TCC attributes an Apple Event to the **responsible process** — for a scheduled Run that is
  `openroutine` (launchd → openroutine → agent → osascript), not your shell. So approving from a
  terminal grants the wrong client and proves nothing. A test has to go *through* openroutine.
- Whether the grant survives depends on the entry's **client type**. A **path-based** row
  (`client_type = 1`) authorises on the executable's path, so replacing the binary at the same
  path keeps working even though its code hash changed. A bundle/hash-pinned row is invalidated
  by any rebuild.

On this machine the row is path-based, so **rebuilds have survived it** — verified after the
1.0.0 install: the stored `csreq` still pinned the 0.2.0 cdhash, the new binary hashed
differently, and the Apple Event succeeded anyway with the TCC row untouched. Do not generalise
that to a freshly user-approved grant, which macOS will likely pin to the code hash instead.

Check it, rather than reasoning about it:

```bash
sqlite3 -header ~/Library/Application\ Support/com.apple.TCC/TCC.db \
  "select auth_value, client_type, hex(csreq) from access
     where client like '%openroutine%' and service='kTCCServiceAppleEvents';"
codesign -d -r- ~/.cargo/bin/openroutine 2>&1 | grep -oE 'H"[0-9a-f]+"'
```

`auth_value = 2` is granted; `client_type = 1` is path-based. A `csreq` that does not contain the
current cdhash only matters if the row is *not* path-based.

The definitive test is an actual Apple Event through openroutine — cheap, no agent invocation, no
API spend, via a probe task whose "agent" is osascript itself:

```toml
# config.toml, temporarily
[agents.osascript]
cmd = "/usr/bin/osascript -e {prompt}"
```

```markdown
---
name: tcc probe
description: Trigger the Messages consent dialog through openroutine
agent: osascript
---

tell application "Messages" to get name
```

`openroutine add tcc-probe.md && openroutine run tcc-probe` fires it. Exit 0 with `Messages` in
the log means the grant holds; a dialog means it did not, so have the user approve it. Then
`openroutine remove tcc-probe` and drop the agent block. Firing the real task is a poor
substitute: it costs an agent run, sends a real message, and — as happened on 2026-08-12 — an
empty hour never reaches its messaging step at all, leaving the grant untested.

## Testing

Two seams, and no others: the **process boundary** (integration tests spawn the real binary in
a temp XDG sandbox and assert only on CLI output, API responses, and the files written, with
stub agent scripts standing in for a real agent CLI), and the **clock** (the scheduling core
takes an injected clock). No test reaches into private functions or internal structs. Adding a
test hook to the product to make a test easier is the wrong move.

## Agent skills

### Issue tracker

Issues live as local markdown files under `.scratch/<feature>/` in this repo. See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary — the five canonical role names used as-is (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — one `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

<!-- swe-workflow:decision-logging -->
## Decision logging (swe-workflow)

When you make a decision on the user's behalf they'd want to review — auto-answering a question they'd otherwise be asked, an irreversible/hard-to-undo action, a tradeoff, a deviation from the spec, or resolving a grill question by your own exploration — record it per the `log-decisions` skill: in a swe-workflow ship build (a worktree), stage to `DECISIONS.staged.md` (promoted to `DECISIONS.md` at close-out); otherwise append to `DECISIONS.md` directly. **Look in the repo first** — most "open" questions are already answered there. Decide-and-log when an artifact grounds the call (verify if irreversible); when it's reversible but unsettled, log a best-guess **assumption** (`Outcome: assumed`) and proceed; **escalate** only what's irreversible and needs human context — and always the catastrophic (data loss, migration, spend, public-interface break).
<!-- /swe-workflow:decision-logging -->

<!-- swe-workflow:engineering-discipline -->
## Engineering discipline (swe-workflow)

Test-first development via the `tdd` skill is **not** a repo-wide rule — it is scoped to swe-workflow's ship and ship-all builds, where the plan names it in `task_plan.md`. Don't impose red → green → refactor on ad-hoc edits outside those flows.
<!-- /swe-workflow:engineering-discipline -->
