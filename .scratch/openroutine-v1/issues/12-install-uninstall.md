# 12 — `install` / `uninstall`: sudo-free boot service

**What to build:** One command makes the schedule survive reboots, with no sudo anywhere. On Linux, `openroutine install` writes a systemd user unit and enables lingering so the daemon starts at boot with no login session; on macOS it writes a per-user LaunchAgent that starts at login (docs note the auto-login pairing for headless machines — the honest asterisk). The service supervises only: start at boot/login, restart on death, never schedule. `uninstall` removes exactly what `install` created. Both commands are idempotent and report precisely what they did.

**Blocked by:** 01 — Walking skeleton.

**Status:** resolved

- [x] `install` succeeds without sudo on both platforms and prints what was written and how to verify
- [x] After `install`, the service manager starts the daemon (login/boot per platform) and restarts it if killed
- [x] Repeated `install` converges (no duplicate units); `uninstall` removes the unit/agent and stops supervision, leaving config, state, and Tasks untouched
- [x] On Linux, lingering is enabled so the daemon runs with no active session
- [x] The installed daemon still launches Runs through the login shell with a real environment (the stripped-service-env failure mode this project exists to fix)
- [x] `status` reflects supervised-vs-foreground appropriately
- [x] Carried from the ticket-05 review: `SHELL` is usually absent under launchd, so a supervised daemon falls back to `/bin/sh` instead of the user's shell — which undercuts the real-environment guarantee this project exists for. The service definition should carry it

## Comments

**Delivered.** 151 tests; clippy and rustfmt clean.

`install` writes a per-user LaunchAgent on macOS or a systemd user unit on Linux, then asks the service manager to pick it up — no sudo on either. On Linux it also enables lingering, which is what makes "starts at boot, with no login session" true; on macOS the daemon starts at login, and `install` says so rather than implying otherwise. Reinstalling takes the old registration out first, so it converges instead of stacking. `uninstall` reverses exactly that and leaves config, state, and Tasks alone.

The definition carries `SHELL`, closing the item carried from the ticket-05 review: a service manager starts processes without one, and the login-shell environment every Run depends on would otherwise fall back to `/bin/sh` — undercutting the guarantee this project exists for.

`install --print` shows the file, its destination, and the commands that would run, writing nothing. That is useful on its own — you can read a service definition before letting anything register it — and it is also the only honest way to test this ticket: actually bootstrapping a LaunchAgent would modify the developer's machine, so the tests assert the definition, and registration with `launchctl`/`systemctl` is verified by hand.
