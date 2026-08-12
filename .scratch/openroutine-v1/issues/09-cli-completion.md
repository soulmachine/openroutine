# 09 — v0.1 CLI completion: `init`, `status`, `logs`, `run --dry-run`

**What to build:** The rest of the day-one and day-two terminal experience, all disk-read (no daemon required). `openroutine init` scaffolds the first five minutes: config with the built-in agent templates, a registered Project, and a sample Task. `status` summarizes at a glance: daemon running or not, Project and Task counts, flagged/Broken counts, global-pause state. `logs <task>` prints the latest Run's output; `--follow` tails it live from disk. `run <task> --dry-run` prints the fully resolved execution plan — validation result, agent, exact argv after `{prompt}`/`{model}`/`{permission_mode}` substitution, delivery mode (argv vs stdin), cwd, timeout, env override keys, and the next fire times with deterministic jitter applied — executing nothing, recording nothing, working with the daemon stopped. (Plain `run` without `--dry-run` fires via the API and lands in ticket 11.)

**Blocked by:** 03 — Scheduling semantics core; 04 — Schedule forms; 05 — Execution environment.

**Status:** ready-for-agent

- [ ] `init` in an empty directory produces a working setup whose sample Task passes `run --dry-run` immediately
- [ ] `status` reports daemon liveness, counts, and global-pause state with the daemon stopped and running
- [ ] `logs <task>` prints the latest Run's output; `--follow` streams appended output of an active Run from disk
- [ ] `--dry-run` shows the exact argv (metacharacters inert), delivery mode, cwd, timeout, and env override keys a real Run would use, and spawns nothing
- [ ] `--dry-run` on cron Tasks predicts jittered next fires; on One-shots the absolute time; on Manual Tasks "no schedule"; on Broken Tasks the validation error
- [ ] Bare Task names work where unambiguous; ambiguous names list the `<project>/<name>` candidates
- [ ] Carried from the ticket-05 review: `--dry-run` prints the resolved argv, which is the first chance to assert the login-shell invocation itself (`$SHELL -l -c 'exec "$0" "$@"' …`) at the process boundary — cover it here
