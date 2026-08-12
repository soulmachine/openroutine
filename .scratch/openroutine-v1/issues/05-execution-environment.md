# 05 — Execution environment & limits

**What to build:** Runs behave exactly as if the user launched the agent in their terminal. Every Run launches through the user's login shell so `PATH`, profile, and API keys are fresh per run, with per-task `env:` frontmatter and config `[env]` layered on top (task wins); the prompt still passes as a single argv token, never spliced into a shell string. `cwd:` overrides the default Project-root working directory (relative paths resolve against the Project). Per-task `model:` and `permission_mode:` inject through `{model}`/`{permission_mode}` template placeholders as single arguments — an unset field omits its template segment; a set field whose placeholder the template lacks warns and is ignored; an unknown `{token}` in any template is a config-load error. Timeouts (default 1h, `timeout: none` to opt out) kill the Run's entire process group: SIGTERM, 10-second grace, SIGKILL, status `timed-out`. Each `output.log` is capped (configurable): at the cap, writing stops, a truncation marker is appended, and the Run continues.

**Blocked by:** 01 — Walking skeleton.

**Status:** resolved

- [x] A stub Agent observes login-shell environment plus `[env]` plus `env:` in the correct precedence, and the correct cwd with and without `cwd:`
- [x] A prompt containing shell metacharacters and flag-like text arrives as one inert argv element
- [x] `model:`/`permission_mode:` reach the stub as single arguments; unset fields leave no empty segment; a template missing the placeholder warns and ignores; `{promt}` in config fails at load
- [x] A stub that spawns a child and hangs is fully dead (child included) after timeout; the Run records `timed-out`
- [x] A stub writing past the output cap yields a capped log with a truncation marker and an otherwise normal Run
- [x] Default timeout applies when `timeout:` is absent; `timeout: none` runs unbounded

## Comments

**Delivered.** 86 tests; clippy and rustfmt clean.

Runs now launch as `$SHELL -l -c 'exec "$0" "$@"' [/usr/bin/env -- K=V …] <program> <args>`. The shell reads the profile so `PATH`, shims, and API keys match a terminal, then `exec`s the real command with our arguments as untouched argv — the prompt never becomes shell syntax. `env` applies overrides *after* the profile, which is what makes them actually win.

Review found a security hole this ticket introduced, now closed and logged as **Q62**: a Task's `env:` key of `--split-string` was passed to `env` as an option, letting a committed task file run a command of its own choosing and bypass the config-owned template. Overrides now go through `env --` and names are restricted to the portable shape. A test asserts the attack leaves no trace.

Other review fixes: the duration parser still used chrono's panicking constructors on its string path, so `timeout: 999999999999d` in one task file would have killed the daemon (now Broken, one task only); the kill path signalled the process group *after* reaping the leader, so a reused pid could have been signalled; `cwd:` accepted absolute paths and `../` climbs out of the Project; a failed log capture was swallowed; and `default_timeout` was re-parsed at fire time, so a bad value produced a Tick that was neither a Run nor a Skip — it is resolved once at load now.

Two things carried forward:

- **`SHELL` is often unset under launchd**, where the fallback is `/bin/sh` rather than the user's shell — which undercuts the whole point on a boot-started daemon. Ticket 12 owns the service definition and should set it; noted there.
- **The login-shell invocation itself has no direct assertion** — the tests prove the environment and the override layering, not the `-l` flag. `run --dry-run` (ticket 09) prints the resolved argv, which is where that becomes observable at the process boundary; noted there.

One rule worth knowing: an unset `model:`/`permission_mode:` drops its own argument *and* a preceding `-`-prefixed token, so `--model {model}` leaves nothing behind. A template putting a bare `{model}` after an unrelated boolean flag would lose that flag too.
