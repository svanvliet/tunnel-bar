<p align="center">
  <img src="design/icon-128.png" width="128" height="128" alt="Tunnel Bar: a laptop connected to a server by a copper route">
</p>

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

### Icon artwork

The editable app icon is `design/icon.svg`; tray state sources are in
`design/tray/`. To regenerate the README image, favicon, monochrome macOS
templates, outlined Windows tray images, and Tauri platform icons, install
`rsvg-convert` (`brew install librsvg`), then run:

```sh
node design/render-icons.mjs
```

macOS uses black-and-alpha templates at 36px (18pt at 2x), including the error
badge. Windows uses a black symbol with a white edge so it remains readable on
light and dark taskbars. A hollow server means disconnected, a dashed route
means connecting/retrying, a filled server means connected, and an exclamation
badge means attention is needed.

## Test

```sh
cd src-tauri
cargo test                         # unit + fake-CLI supervisor tests
cargo clippy --all-targets -- -D warnings
TUNNELBAR_LIVE_TUNNEL=<tunnel-id> cargo test live_connect -- --ignored --nocapture   # real CLI
TUNNELBAR_LIVE_HOST=<tunnel-id> cargo test live_host -- --ignored --nocapture   # tunnel with an http port served locally
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
3. Phase 3 – tunnel management (create, delete, ports, access)
4. Phase 4 – Windows tray version
5. Phase 5 – signed/notarized DMG, auto-update, CI releases
