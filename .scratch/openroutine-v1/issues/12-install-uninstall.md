# 12 — `install` / `uninstall`: sudo-free boot service

**What to build:** One command makes the schedule survive reboots, with no sudo anywhere. On Linux, `openroutine install` writes a systemd user unit and enables lingering so the daemon starts at boot with no login session; on macOS it writes a per-user LaunchAgent that starts at login (docs note the auto-login pairing for headless machines — the honest asterisk). The service supervises only: start at boot/login, restart on death, never schedule. `uninstall` removes exactly what `install` created. Both commands are idempotent and report precisely what they did.

**Blocked by:** 01 — Walking skeleton.

**Status:** ready-for-agent

- [ ] `install` succeeds without sudo on both platforms and prints what was written and how to verify
- [ ] After `install`, the service manager starts the daemon (login/boot per platform) and restarts it if killed
- [ ] Repeated `install` converges (no duplicate units); `uninstall` removes the unit/agent and stops supervision, leaving config, state, and Tasks untouched
- [ ] On Linux, lingering is enabled so the daemon runs with no active session
- [ ] The installed daemon still launches Runs through the login shell with a real environment (the stripped-service-env failure mode this project exists to fix)
- [ ] `status` reflects supervised-vs-foreground appropriately
- [ ] Carried from the ticket-05 review: `SHELL` is usually absent under launchd, so a supervised daemon falls back to `/bin/sh` instead of the user's shell — which undercuts the real-environment guarantee this project exists for. The service definition should carry it
