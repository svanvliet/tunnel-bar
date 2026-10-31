# TunnelBar

A macOS menu-bar app for managing [Microsoft Dev Tunnels](https://learn.microsoft.com/azure/developer/dev-tunnels/) client connections. It wraps the `devtunnel` CLI, so it uses your existing CLI sign-in.

## Features (Phase 1 MVP)

- Lists every tunnel on your account, with host online/offline status.
- Connect and disconnect, with auto-reconnect using exponential backoff. Stops retrying on auth errors.
- Per-tunnel **Auto** connect on launch; launch at login.
- Detects `devtunnel connect` processes started outside TunnelBar, with **Take over** and **Stop**.
- Shows forwarded ports (`localhost:L → :R`), with Copy SSH (uses your `~/.ssh/config` alias if one matches) and Open SSH (Terminal).
- Per-tunnel live logs, saved to `~/Library/Logs/TunnelBar/`.
- Notifications on unexpected drops and reconnects.
- Optional local nicknames (double-click a tunnel name).

## Requirements

- macOS 11+
- `devtunnel` CLI (`brew install --cask devtunnel`), signed in with `devtunnel user login`

## Build

```sh
npm ci
npm run tauri dev     # development
npm run tauri build   # release .app in src-tauri/target/release/bundle/macos
codesign --force --deep -s - src-tauri/target/release/bundle/macos/TunnelBar.app   # ad-hoc sign
```

## Test

```sh
cd src-tauri
cargo test                         # unit + fake-CLI supervisor tests
cargo clippy --all-targets -- -D warnings
TUNNELBAR_LIVE_TUNNEL=<tunnel-id> cargo test live_ -- --ignored --nocapture   # real CLI
cd .. && npm run check
```

## Debug environment variables

| Variable | Effect |
| --- | --- |
| `TUNNELBAR_TRACE=1` | (debug builds) print each state snapshot to stderr |
| `TUNNELBAR_PIN=1` | keep the popover open when it loses focus |
| `TUNNELBAR_LIVE_TUNNEL` | tunnel id for the ignored live integration test |

## Roadmap

1. **Phase 1 – connection management MVP** (this release)
2. Phase 2 – hosting (`devtunnel host`) from the menu bar
3. Phase 3 – tunnel management (create, delete, ports, access)
4. Phase 4 – Windows tray version
5. Phase 5 – signed/notarized DMG, auto-update, CI releases
