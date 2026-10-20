# Vector Remote CLI

Private, owner-only remote shell via encrypted Vector DM. Rust + [Vector SDK](https://github.com/VectorPrivacy/Vector/tree/master/crates/vector-sdk).

**Status:** experimental. Build and DM round-trip are not yet verified. Run only on a device you own. Never publish your Bot's nsec, owner configuration, or logs.

## Existing Mac installation (one-time move to GitHub updates)

Stop the previous Bot with Ctrl+C, then run **once** in Terminal:

```bash
git clone https://github.com/badjoke-lab/vector-remote-cli.git "$HOME/.local/share/vector-remote-cli/source"
bash "$HOME/.local/share/vector-remote-cli/source/scripts/update-and-start.sh"
```

Thereafter double-click `更新して起動.command` **in that source folder**. Each update fetches from GitHub, builds, and starts the Bot, leaving the existing owner npub and Bot identity untouched.

The installer does NOT copy or upload your keys. It uses:
- `~/.config/vector-remote-cli/owner.npub`
- `~/.local/share/vector-remote-cli/data/identity.nsec`
- `~/.local/bin/vector-remote-cli`

## Testing

Wait for `[READY]` in the Mac terminal. The Bot then attempts an outbound confirmation DM to the configured owner. In Vector, send `connect` (no slash), then `pwd`. Look for `[RX]`, `[RX SKIP]`, and `[TX ERROR]` if there is no reply.

Commands are executed as the logged-in Mac user. Do not use this experimental remote shell on sensitive devices or as root. Message timestamps and message IDs are checked for replay, but the project has not been independently security-audited.

## SDK

The dependency is pinned to [Vector commit `20efc172`](https://github.com/VectorPrivacy/Vector/commit/20efc172693c3556a7d9b61715effc306536f46a) while the interface is tested. This repository contains no compiled binaries or private keys.
