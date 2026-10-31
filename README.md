# Tunnel Bar

A macOS menu-bar app for managing [Microsoft Dev Tunnels](https://learn.microsoft.com/azure/developer/dev-tunnels/): connect to tunnels and host your own. It wraps the `devtunnel` CLI, so it uses your existing CLI sign-in.

## Features

### Connections

- Lists every tunnel on your account, with host online/offline status.
- Connect and disconnect, with auto-reconnect using exponential backoff. Stops retrying on auth errors.
- Per-tunnel **Auto** connect on launch; launch at login.
- Detects `devtunnel connect` processes started outside Tunnel Bar, with **Take over** and **Stop**.
- Shows forwarded ports (`localhost:L → :R`), with Copy SSH (uses your `~/.ssh/config` alias if one matches) and Open SSH (Terminal).
- Per-tunnel live logs, saved to `~/Library/Logs/TunnelBar/`.
- Notifications on unexpected drops and reconnects.
- Optional local nicknames (double-click a tunnel name).
- Never orphans connections: stops children on Quit/SIGTERM, and reclaims any left by a crash on next launch.

### Hosting

- **＋ Expose a local port**: creates a tunnel labelled `tunnelbar`, adds the port (http/https/auto), and hosts it from this Mac.
- **Host here** / **Stop hosting** for your own tunnels, with optional **Auto-host** on launch and auto-restart.
- Shows each hosted port's public URL (open/copy) and Inspect link, and warns when nothing is listening locally.
- Access control: private (your account) by default; **Make public…** adds anonymous access after confirmation; **Require sign-in** removes it.
- Delete tunnels that Tunnel Bar created. Tunnels hosted by another machine (e.g. a dev box) are never hosted or deleted from here.

### Management (Phase 3: backend ready, UI in progress)

- Create tunnels with an id, description, labels, an expiration (1h–30d), ports, public access and hosting. Tunnel Bar always adds the `tunnelbar` label.
- Edit labels, description and expiration. Shows each tunnel's lifetime.
- Add and remove ports. Tunnel Bar restarts its host so the change takes effect.
- Access rules beyond public: your Entra tenant, a GitHub organization (needs a GitHub sign-in) or a repository, as allow or deny rules with an optional expiration, on the tunnel or a single port. Changing rules on a tunnel Tunnel Bar didn't create needs confirmation.
- Host/Origin header rewriting per tunnel or port, including `unchanged` for dev servers like Vite and Next.js.
- Delete tunnels.
  - Deleting a tunnel Tunnel Bar didn't create needs explicit confirmation.
  - A tunnel hosted by another machine also needs a separate override.
- In-app sign-in (Microsoft or GitHub, with a browser or device-code flow) and sign-out.
- **Stable local ports**: pin a remote port to a fixed `localhost` port. Tunnel Bar runs a small loopback proxy to whichever port `devtunnel connect` picked. A static `~/.ssh/config` `Port` then keeps working across reconnects, and Copy/Open SSH use the pin.
- A global hotkey toggles the popover. The default is ⌘⌥⇧T, and it can be configured or disabled.
- Filter tunnels by label.

See [docs/phase3-ui-hooks.md](docs/phase3-ui-hooks.md) for the commands and fields.

## Requirements

- macOS 11+
- `devtunnel` CLI (`brew install --cask devtunnel`), signed in with `devtunnel user login`

## Install

Download the DMG from [Releases](https://github.com/svanvliet/tunnel-bar/releases) and drag Tunnel Bar to Applications. Builds are ad-hoc signed (not notarized yet), so clear the quarantine flag before first launch:

```sh
xattr -dr com.apple.quarantine "/Applications/Tunnel Bar.app"
```

## Build

```sh
npm ci
npm run tauri dev     # development
npm run tauri build   # release .app in src-tauri/target/release/bundle/macos
codesign --force --deep -s - src-tauri/target/release/"bundle/macos/Tunnel Bar.app"   # ad-hoc sign
./scripts/package-macos.sh   # universal DMG + zip in dist/
```

## Test

```sh
cd src-tauri
cargo test                         # unit + fake-CLI supervisor tests
cargo clippy --all-targets -- -D warnings
TUNNELBAR_LIVE_TUNNEL=<tunnel-id> cargo test live_connect -- --ignored --nocapture   # real CLI
TUNNELBAR_LIVE_HOST=<tunnel-id> cargo test live_host -- --ignored --nocapture   # tunnel with an http port served locally
cargo test live_manage -- --ignored --nocapture   # creates/edits/hosts/pins/deletes a temporary tunnelbar-test-* tunnel (1h expiry)
cd .. && npm run check
```

## Debug environment variables

| Variable | Effect |
| --- | --- |
| `TUNNELBAR_TRACE=1` | (debug builds) print each state snapshot to stderr |
| `TUNNELBAR_PIN=1` | keep the popover open when it loses focus |
| `TUNNELBAR_LIVE_TUNNEL` | tunnel id for the ignored live integration test |
| `TUNNELBAR_LIVE_HOST` | tunnel id for the ignored live hosting test |

## Roadmap

1. ~~Phase 1 – connection management MVP~~ (v0.1.0)
2. ~~Phase 2 – hosting (`devtunnel host`) from the menu bar~~ (v0.2.0)
3. Phase 3 – tunnel management (create, delete, ports, labels, expiration, sign-in, stable ports, hotkey, label filter): backend done, UI in progress
4. Phase 4 – Windows tray version
5. Phase 5 – signed/notarized DMG, auto-update, CI releases
