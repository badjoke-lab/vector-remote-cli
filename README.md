# Vector Remote CLI

Private, owner-only remote shell via encrypted Vector DM. Rust + [Vector SDK](https://github.com/VectorPrivacy/Vector/tree/master/crates/vector-sdk).

**Status:** experimental. DM round trip, persistent cwd, basic session close/reconnect, and Linux/macOS CI checks have been verified; long-running unattended operation is not yet field-verified. Run only on a device you own. Never publish your Bot's nsec, owner configuration, or logs.

## Existing Mac installation (one-time move to GitHub updates)

Stop the previous Bot with Ctrl+C, then paste **once** into Terminal:

```bash
git clone https://github.com/badjoke-lab/vector-remote-cli.git "$HOME/.local/share/vector-remote-cli/source" && bash "$HOME/.local/share/vector-remote-cli/source/scripts/setup-mac.sh"
```

After that, double-click **Vector Remote CLI.command** on your Desktop. The launcher fetches updates from GitHub, builds, and starts the Bot, leaving the existing owner npub and Bot identity untouched.

If the folder `$HOME/.local/share/vector-remote-cli/source` already exists, do not delete it or repeat `git clone`; use `bash "$HOME/.local/share/vector-remote-cli/source/scripts/update-and-start.sh"` instead.

The installer does NOT copy or upload your keys. It uses:
- `~/.config/vector-remote-cli/owner.npub`
- `~/.local/share/vector-remote-cli/data/identity.nsec`
- `~/.local/bin/vector-remote-cli`

## Optional macOS background service

The foreground Bot must be stopped first (Control+C). Install the user-level
launchd service using the existing GitHub checkout:

```bash
cd "$HOME/.local/share/vector-remote-cli/source"
git pull --ff-only
bash scripts/manage-mac.sh enable
```

This **opt-in** service starts on user login and restarts the Bot after a crash.
No sudo, administrator privileges, network-exposed ports or cloud secrets are
required. It runs the already-built binary and does not update code by itself.

To manage the background Bot on your Mac:

```bash
cd "$HOME/.local/share/vector-remote-cli/source"
bash scripts/manage-mac.sh status   # loaded/running state
bash scripts/manage-mac.sh update   # stop, git pull/build, restart (old binary retained on failure)
bash scripts/manage-mac.sh disable  # stop and remove LaunchAgent, keep keys/data
```

The desktop `Vector Remote CLI.command` still launches the **foreground**
updater. It will refuse to launch another Bot if the background LaunchAgent is
loaded. Use `manage-mac.sh update` when the service is enabled.

Private macOS service logs are saved in
`~/.local/share/vector-remote-cli/logs/` (user-only permissions). Logs and
Bot identity stay local, and are not uploaded to GitHub. Logs are not
automatically rotated yet; review storage periodically.

**Limitations:** The Mac must stay on and awake for remote access; launchd
does not bypass sleep, logout, macOS security permissions, or network outages.
The SDK may reconnect to relays, but crash restart and reconnect after sleep
still require field tests. The shell uses pipes rather than a true interactive
terminal, so full-screen apps (vim, top etc.) are unsupported. Because the Bot
can execute commands with your account privileges, secure your Vector account
and do not publish `identity.nsec`.

## Testing

Wait for `[READY]` in the Mac terminal. The Bot then attempts an outbound confirmation DM to the configured owner. In Vector, send `connect` (no slash), then `pwd`. Look for `[RX]`, `[RX SKIP]`, and `[TX ERROR]` if there is no reply.

Commands are executed as the logged-in Mac user. Do not use this experimental remote shell on sensitive devices or as root. Message timestamps and message IDs are checked for replay, but the project has not been independently security-audited.

## SDK

The dependency is pinned to [Vector commit `20efc172`](https://github.com/VectorPrivacy/Vector/commit/20efc172693c3556a7d9b61715effc306536f46a) while the interface is tested. This repository contains no compiled binaries or private keys.
