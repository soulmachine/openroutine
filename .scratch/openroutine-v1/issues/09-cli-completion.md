# 09 — v0.1 CLI completion: `init`, `status`, `logs`, `run --dry-run`

**What to build:** The rest of the day-one and day-two terminal experience, all disk-read (no daemon required). `openroutine init` scaffolds the first five minutes: config with the built-in agent templates, a registered Project, and a sample Task. `status` summarizes at a glance: daemon running or not, Project and Task counts, flagged/Broken counts, global-pause state. `logs <task>` prints the latest Run's output; `--follow` tails it live from disk. `run <task> --dry-run` prints the fully resolved execution plan — validation result, agent, exact argv after `{prompt}`/`{model}`/`{permission_mode}` substitution, delivery mode (argv vs stdin), cwd, timeout, env override keys, and the next fire times with deterministic jitter applied — executing nothing, recording nothing, working with the daemon stopped. (Plain `run` without `--dry-run` fires via the API and lands in ticket 11.)

**Blocked by:** 03 — Scheduling semantics core; 04 — Schedule forms; 05 — Execution environment.

**Status:** resolved

- [x] `init` in an empty directory produces a working setup whose sample Task passes `run --dry-run` immediately
- [x] `status` reports daemon liveness, counts, and global-pause state with the daemon stopped and running
- [x] `logs <task>` prints the latest Run's output; `--follow` streams appended output of an active Run from disk
- [x] `--dry-run` shows the exact argv (metacharacters inert), delivery mode, cwd, timeout, and env override keys a real Run would use, and spawns nothing
- [x] `--dry-run` on cron Tasks predicts jittered next fires; on One-shots the absolute time; on Manual Tasks "no schedule"; on Broken Tasks the validation error
- [x] Bare Task names work where unambiguous; ambiguous names list the `<project>/<name>` candidates
- [x] Carried from the ticket-05 review: `--dry-run` prints the resolved argv, which is the first chance to assert the login-shell invocation itself (`$SHELL -l -c 'exec "$0" "$@"' …`) at the process boundary — cover it here

## Comments

**Delivered.** 149 tests; clippy and rustfmt clean.

`init` writes a config and a sample Task that works as written — the test proves it by listing immediately afterwards and requiring nothing Broken. `status` reports whether a Daemon holds the lock (asking with a shared-lock probe, so it never disturbs the real one), where things live, and how many Tasks are ready, Broken, or flagged. `logs` prints the latest Run from disk, with `--follow` tailing it. Bare Task names resolve when unambiguous and list the candidates when not.

`run --dry-run` shows the whole plan: the login shell, the exact argv one line per argument, whether the prompt travels in argv or on stdin, the working directory, the timeout, the environment names, and the next fire with its jitter spelled out. It closes the carried item from ticket 05 — the login-shell invocation is now assertable at the process boundary.

Three bugs my own tests caught before review: the generated config put `default_agent` after the `[agents.*]` tables, so TOML read it as part of one and `init` produced a config that would not load; the timeout printed as `PT1800S`; and — the one that mattered — **the environment values were visible in the displayed argv** while a line underneath claimed they were hidden. A plan is exactly the sort of thing someone pastes into a bug report, and a Task's `env:` is exactly where an API token lives, so values are now replaced with `<hidden>` in the argv too.

Plain `run` without `--dry-run` explains that firing goes through the daemon's API, which ticket 11 adds.
