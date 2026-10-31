# AGENTS.md

Guidance for AI agents working on **Tunnel Bar**, a menu-bar/tray app (Tauri 2: Rust core in `src-tauri/`, Svelte 5 + TypeScript UI in `src/`) that wraps Microsoft's `devtunnel` CLI.

## Working model: conductor + background lanes

- The main session is the **conductor**. It plans, splits work into independent **lanes**, reviews, integrates, and releases. It does not do lane work itself.
- Each lane is a **nested child session** with its own worktree and branch (`svanvliet-<lane-name>`). Give it a standalone kickoff with the user's verbatim request, scope, the API contracts it depends on, and validation expectations.
- Keep lanes busy. On every active turn, fill any idle lane from the ready backlog without waiting to be asked.
- Pick models per lane. Use strong models for design, UX and complex work, and cheaper models for routine work.
- Lanes run in **autopilot**. They decide implementation details themselves. **Design and UX choices go to the user.** Lanes ask the user, and the conductor relays the question. The conductor never answers a design question on the user's behalf.
- Trust but verify. The conductor reviews each lane's diff, runs the validation itself, and inspects artifacts: screenshots, logs, the running app. Use a code-review agent for non-trivial changes before merging.
- Cross-lane contract changes (commands, snapshot fields, error prefixes) go in `docs/phase3-ui-hooks.md`, and the conductor tells the affected lanes.
- Progress reports cover parallelism, per-phase progress, wall-clock and agent-hours spent, and estimates to completion.

## Git and releases

- **Only the conductor merges to `main` and publishes releases.** Lanes commit on their own branches, rebase onto `main` when asked, and never push or merge.
- The conductor pushes `main` and publishes GitHub releases **only when the user asks or at agreed milestones**.
- Identity: GitHub account `svanvliet`, email `782486+svanvliet@users.noreply.github.com`. Never use a work identity.
- Commits end with `Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>`.
- Release flow: bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`. Then run `scripts/package-macos.sh` (universal `.dmg` + `.app.zip`), tag `vX.Y.Z`, and create the GitHub release with `--notes-file`.

## Validation

```sh
cd src-tauri && cargo test -q && cargo clippy -q --all-targets -- -D warnings
npm run -s check
npx tauri build --bundles app   # -> src-tauri/target/release/bundle/macos/Tunnel Bar.app
```

- Live tests run against the real devtunnel service and are `#[ignore]`: `cargo test live_manage -- --ignored --nocapture`. They may only create, modify or delete tunnels named `tunnelbar-test-*`. Never touch the user's other tunnels.
- **Validate locally; don't spend GitHub Actions minutes.** CI (`.github/workflows/ci.yml`) is manual-only (`workflow_dispatch`). Don't add push or PR triggers, and don't run it unless the user asks.
- Windows code is checked on the Mac by cross-target clippy (`rustup target add x86_64-pc-windows-msvc`, then `cargo clippy --target x86_64-pc-windows-msvc`). The real Windows build and testing happen on the user's PC.

## Installing a local build (macOS)

The user relies on live connections such as SSH on `localhost:2222`. Before replacing the installed app:

1. Check for active sessions with `lsof -iTCP:2222 -sTCP:ESTABLISHED`. If any are live, ask the user before restarting.
2. Get the PID with `pgrep -f "Tunnel Bar.app/Contents/MacOS"`, then `kill <pid>`. Never use `pkill` or `killall`.
3. Replace and re-sign:
   `rm -rf "/Applications/Tunnel Bar.app"`, `cp -R` the new bundle, `codesign --force --deep -s - "/Applications/Tunnel Bar.app"`, then `open` it.

## Product invariants

- The display name is "Tunnel Bar". These stay stable so settings and state persist: bundle id `me.svv.tunnelbar`, `~/Library/Logs/TunnelBar`, the settings folder, and the `tunnelbar` tunnel label.
- The app is menu-bar only (`LSUIElement`, no Dock icon).
- Tunnels the app didn't create, meaning they lack the `tunnelbar` label, are protected:
  - Destructive or access-changing operations require `confirm`.
  - Adding or removing the label on them is refused.
  - Guard errors start with `CONFIRM_REQUIRED` or `HOSTED_ELSEWHERE`, and the UI handles them inline.
- `devtunnel host` always forwards to localhost on the same port number. Mapping a remote port to a different local port, and stable client ports (pins), use in-app loopback proxies (`src-tauri/src/pin.rs`). Check `pin::port_in_use` before binding.
- The devtunnel service is eventually consistent. Update the cache optimistically and schedule a delayed refresh.

## Icons

- Sources live in `design/icon.svg` (app icon) and `design/tray/*.svg` (the four tray states).
- Regenerate everything with `node design/render-icons.mjs`. This needs `rsvg-convert` and also reruns `tauri icon`. Revert incidental `icon.icns` byte churn.
- On macOS the tray PNGs are 44×36 templates (black ink plus alpha). On Windows they are 32×32 solid glyphs in two sets, `windows/dark` (white ink) and `windows/light` (black ink), picked from the taskbar theme.
- Tests in `src-tauri/src/icon.rs` assert these dimensions.

## Docs

- `README.md` documents features with screenshots in `docs/screenshots/`. Screenshots use **synthetic data only**: no real tunnel IDs, accounts or hostnames.

## Environment quirks (macOS host)

- Use `perl` or `python3` for in-place edits; BSD `sed -i` differs from GNU sed. PIL is not available.
- Ask the user before any computer-use, UI automation or headed browser work. Headless checks are fine.
