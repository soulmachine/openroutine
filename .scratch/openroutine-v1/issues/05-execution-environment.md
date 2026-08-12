# 05 — Execution environment & limits

**What to build:** Runs behave exactly as if the user launched the agent in their terminal. Every Run launches through the user's login shell so `PATH`, profile, and API keys are fresh per run, with per-task `env:` frontmatter and config `[env]` layered on top (task wins); the prompt still passes as a single argv token, never spliced into a shell string. `cwd:` overrides the default Project-root working directory (relative paths resolve against the Project). Per-task `model:` and `permission_mode:` inject through `{model}`/`{permission_mode}` template placeholders as single arguments — an unset field omits its template segment; a set field whose placeholder the template lacks warns and is ignored; an unknown `{token}` in any template is a config-load error. Timeouts (default 1h, `timeout: none` to opt out) kill the Run's entire process group: SIGTERM, 10-second grace, SIGKILL, status `timed-out`. Each `output.log` is capped (configurable): at the cap, writing stops, a truncation marker is appended, and the Run continues.

**Blocked by:** 01 — Walking skeleton.

**Status:** ready-for-agent

- [ ] A stub Agent observes login-shell environment plus `[env]` plus `env:` in the correct precedence, and the correct cwd with and without `cwd:`
- [ ] A prompt containing shell metacharacters and flag-like text arrives as one inert argv element
- [ ] `model:`/`permission_mode:` reach the stub as single arguments; unset fields leave no empty segment; a template missing the placeholder warns and ignores; `{promt}` in config fails at load
- [ ] A stub that spawns a child and hangs is fully dead (child included) after timeout; the Run records `timed-out`
- [ ] A stub writing past the output cap yields a capped log with a truncation marker and an otherwise normal Run
- [ ] Default timeout applies when `timeout:` is absent; `timeout: none` runs unbounded
