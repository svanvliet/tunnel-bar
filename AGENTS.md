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
- Release flow:
  1. Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`.
  2. Write the release notes to a file and run `NOTES_FILE=<notes.md> scripts/package-macos.sh`. It builds universal, signs with `Developer ID Application: Scott Van Vliet (97379Y67S5)` (hardened runtime, no entitlements), notarizes and staples the `.app` and `.dmg` with the `spicewood-notary` notarytool keychain profile, and checks them with `spctl -a -vv` and `stapler validate`.
  3. Outputs in `dist/`: `.dmg`, `.app.zip`, the updater `Tunnel-Bar_X.Y.Z_universal.app.tar.gz` + `.sig`, `latest.json` (darwin-aarch64 and darwin-x86_64 both point at the universal tarball) and `SHA256SUMS`.
  4. Tag `vX.Y.Z` and create the GitHub release with `--notes-file`, uploading the dmg, zip, tar.gz, `.sig`, `latest.json` and `SHA256SUMS`. The app reads `https://github.com/svanvliet/tunnel-bar/releases/latest/download/latest.json`, so the newest non-draft, non-prerelease release must carry `latest.json`.
- Updater signing key: private key at `~/.tauri/tunnelbar-updater.key` (never in the repo); its password is in the login keychain (`security find-generic-password -s tunnelbar-updater-key-password -w`), and the script reads both. Only the public key is in `tauri.conf.json`. `requireSignedVersion` is on, so the signature must be made with `tauri signer sign --app-version X.Y.Z` (the script does this). Losing the key means installed apps can't update. Never print or commit credentials.
- Windows updater artifacts (not published yet; after PC testing): on the PC, set `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` from the same key, then either build with `bundle.createUpdaterArtifacts: true` (produces the NSIS `-setup.exe` + `.sig`) or sign the setup `.exe` with `npx tauri signer sign --app-version X.Y.Z`. Add `"windows-x86_64": {"signature": <.sig contents>, "url": <release URL of the setup .exe>}` to `latest.json` before uploading. Updates install with NSIS `passive` mode.
- Testing updates without publishing: build two versions with `ALLOW_TEST_BUILD=1 TUNNELBAR_TEST_PROFILE=updatetest TUNNELBAR_UPDATE_ENDPOINT=http://127.0.0.1:8765/latest.json DOWNLOAD_BASE=http://127.0.0.1:8765 EXTRA_CONFIG='{"version":"0.3.90","identifier":"me.svv.tunnelbar.updatetest","plugins":{"updater":{"dangerousInsecureTransportProtocol":true}}}' scripts/package-macos.sh`. Serve the higher version's `latest.json` and tarball with `python3 -m http.server 8765 --bind 127.0.0.1`, unzip the lower version outside `/Applications`, and `open --env TUNNELBAR_TEST_AUTO_INSTALL=1` it. The test profile keeps its own settings, logs and single-instance socket, so the installed app and its tunnels aren't touched. Delete `~/Library/{Application Support,Logs,WebKit,Caches}/*updatetest*` afterwards.

## Validation

```sh
cd src-tauri && cargo test -q && cargo clippy -q --all-targets -- -D warnings
npm run -s check
npx tauri build --bundles app   # -> src-tauri/target/release/bundle/macos/Tunnel Bar.app
```

- Live tests run against the real devtunnel service and are `#[ignore]`: `cargo test live_manage -- --ignored --nocapture`. They may only create, modify or delete tunnels named `tunnelbar-test-*`. Never touch the user's other tunnels.
- **Validate locally; don't spend GitHub Actions minutes.** CI (`.github/workflows/ci.yml`) is manual-only (`workflow_dispatch`). Don't add push or PR triggers, and don't run it unless the user asks.
- Windows code is checked on the Mac by cross-target clippy (`rustup target add x86_64-pc-windows-msvc`, then `cargo clippy --target x86_64-pc-windows-msvc`). The real Windows build and testing happen on the user's PC.
  - `tauri-winres` needs `llvm-rc`. If it isn't installed, put a stub on `PATH` that writes an empty file to the path after `/fo` (clippy doesn't link): `PATH=/tmp/rcstub:$PATH cargo clippy -q --all-targets --target x86_64-pc-windows-msvc -- -D warnings`.
  - The updater uses `native-tls` rather than rustls, because `ring` doesn't cross-compile to MSVC from macOS.

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

- **Keep docs current in the same change.** Any change to features, commands, settings, install/build/release steps, platform behaviour or agent workflow updates the affected docs in the same commit: `README.md`, `AGENTS.md`, `docs/*.md` and screenshots. Lanes include the doc updates on their branch; the conductor checks for them in review and doesn't merge code whose docs are stale.
- `README.md` documents features with screenshots in `docs/screenshots/`. Screenshots use **synthetic data only**: no real tunnel IDs, accounts or hostnames.
- **The repo is public.** Tests, fixtures, docs, screenshots and commit messages use synthetic names only, such as `devbox-ssh` or `abc-2222.usw3.devtunnels.ms`. Never commit real tunnel IDs, tunnel URLs, machine names, accounts or tokens.

## Environment quirks (macOS host)

- Use `perl` or `python3` for in-place edits; BSD `sed -i` differs from GNU sed. PIL is not available.
- Ask the user before any computer-use, UI automation or headed browser work. Headless checks are fine.
