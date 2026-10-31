# Tunnel Bar design refresh

## Approved direction

Implement refined direction C: **Purpose groups**, with A's explicit access
labels, the approved toast dock, and restrained motion. Retain the 400 × 580
popover, all existing backend commands/events, and the two-word app name.
App icons and menu-bar/tray icons belong to a separate lane and are unchanged.

## Structure and behavior

- **Remote connections:** account tunnels not created by Tunnel Bar, plus any
  locally created tunnel with a connection state. Keep exact forwarded-port
  mappings, SSH tools, connection state, host availability, and Auto-connect.
- **Local tunnels:** locally created tunnels, and tunnels currently hosted on
  this device. Keep inactive local tunnels here with **Host**, not **Connect**.
  A dual-role tunnel can appear in both groups; both reference the same snapshot.
- Use human-readable names first. Move tunnel IDs, rename, logs, and secondary
  hosting actions into a keyboard-accessible contextual details disclosure.
  Retain double-click rename as a shortcut.
- Show private account-only or public URL access explicitly. Retain public-access
  and deletion confirmations inline on the affected local tunnel. Management
  also supports deleting account tunnels, with explicit ownership and remote-host
  confirmations enforced by the backend.
- Keep port tools beside their address. Long URLs truncate, remain selectable,
  and expose the complete value via their title and copy action. SVG action icons
  have accessible labels; do not redesign the app or tray icon.
- Keep external-process Take over / Stop behavior and host-external ownership
  restrictions. Preserve orphaned-tunnel warnings and existing host eligibility.

## Supporting screens

Keep Expose, Logs, and Settings as compact full-height views with a consistent
back action and footer. Expose retains the optional tunnel-name field from
`d82b657`, remains private by default, validates port range,
and explains anonymous access. Logs retain follow, clear, copy, error colors,
and folder opening. Settings retain launch-at-login, notifications, CLI path /
auto-detection, in-app Microsoft/GitHub sign-in, device-code progress, sign-out,
and the diagnostic folder. Terminal sign-in remains a fallback.

Expose has adjacent **Local port** and optional **Tunnel port** fields. An empty
tunnel port means the same number; a distinct number creates the local host map,
for example `:2222 → localhost:22`. Validate both ports independently before IPC.

## Feedback and motion

- One toast dock in normal document flow above the footer, never over buttons.
  Successful copy/save feedback lasts about 2.6 seconds; a new success replaces
  an old one. Hover, keyboard focus, or a hidden document pauses dismissal.
- Operation failures remain until dismissed or retried. Show the actual error,
  not a success-shaped fallback. Restrict retry to non-destructive operations;
  destructive retries must pass through their inline confirmation again.
- Connection/hosting progress and authentication failures remain on the tunnel.
  Do not add in-app reconnect notification spam or change native notifications.
- Use short CSS / Svelte transitions: control feedback around 100 ms, menus and
  status around 140–160 ms, toast entrance 180 ms / exit 120 ms. No idle
  connection pulses, celebratory animation, or blanket list staggering.
- Respect `prefers-reduced-motion`, preserve keyboard focus on navigation,
  announce success politely and errors assertively, and make dismiss/retry
  keyboard reachable. Use the existing Svelte runtime, not new motion packages.

## Implementation boundaries and Phase 3 integration

The page owns snapshot subscriptions, view navigation, IPC, and error handling.
A shared tunnel-card component owns role-specific presentation and local
disclosure/confirmation state. Shared UI tokens use system type, light/dark
semantics, and a small Windows shell adaptation; no native material API is added.

The branch incorporates `8e7bc02` and its ancestors: the approved app/tray icons,
Phase 3 backend, review guards, and host-port mapping. This lane changes no Rust,
app-icon, tray-icon, or native material code.

- **Expose → Advanced:** editable tenant / GitHub organization / repository
  rules, allow/deny and rule lifetime, plus Host/Origin rewriting. The dev-server
  preset sets both headers to `unchanged` and explains Vite's allowed-host setting.
  The optional name from `d82b657` remains; creation uses `expose_port`.
- **Tunnel details → Manage:** a scrollable management view with disclosure
  sections for description, labels, lifetime, ports, local hosting maps, stable
  client-port pins, access rules, and tunnel/per-port header rewrites.
  Keep the `tunnelbar` ownership label out of the editable labels. Do not offer
  inherited access entries for removal; use backend-owned indices and scope.
- **Settings:** accelerator entry (empty disables), label-filter chips, browser
  or device-code sign-in for Microsoft/GitHub, cancel, and inline sign-out
  confirmation. Filtering does not stop connections or alter the native tray.
- **Guarded changes:** map `CONFIRM_REQUIRED` and `HOSTED_ELSEWHERE` into inline
  confirmation, not toasts. Never pre-set either flag. Approving one guard only
  sets its matching flag, and a second guard requires a separate approval.
  Preserve the pending operation after a failed change; do not offer automatic
  destructive retries.
- **Local addresses:** forwarded addresses and copy feedback use client pins;
  SSH IPC keeps its existing arguments because the backend applies the pin.
  Hosted rows show distinct tunnel/local mappings, and server warnings use the
  actual local server port. Surface pin/map errors inline.

Small shared Svelte components provide header fields, access-rule fields, login
progress, and tunnel management. The page owns command invocation and feedback;
forms own scoped drafts and confirmations. Preserve settings by spreading the
latest complete object so nickname/automation changes cannot drop pins or maps.

## Validation and delivery

1. Rebase this branch onto local `main`, including the rename, LSUIElement,
   approved icons, Phase 3 review fixes and host mapping.
2. Run `npm run check` and `npx tauri build --bundles app`.
3. Render the actual frontend headlessly at 400 × 580 with synthetic IPC data.
   Compare macOS/Windows light/dark, supporting views, and toast states against
   the approved boards. Check long names/URLs, multiple ports, dual-role tunnels,
   external ownership, loading/auth/CLI failures, confirmations, clipboard
   failures, focus, reduced motion, and the exact forwarded-port mapping.
4. Never operate the user's live `devbox-ssh` connection for validation.
5. Commit only this branch, with no push or merge. Report the commit, validation,
   and screenshots. Native Windows execution and material fidelity remain
   separate from the macOS build and headless platform-style checks.

## Reproducing synthetic screenshots

Use Node with native WebSocket support (Node 22+) and an already-installed
Chromium/headless-shell executable. No browser packages or test dependencies are
added, and the renderer never launches a headed browser or calls a real Tauri
backend.

```sh
npm run build
BROWSER_EXECUTABLE=/absolute/path/to/chromium node scripts/render-ui.mjs
```

The renderer serves the actual built frontend, emulates a 400 × 580 WebView at
2× scale, and injects synthetic IPC snapshots. It renders light/dark main,
Expose (name, advanced rules/headers and port mapping), Logs, Settings
(shortcut/filter/account), public/delete/sign-out and guard confirmations,
toasts, ports/pins, hosting maps, access rules, and sign-in progress. PNGs go to
`docs/screenshots/`; `UI_SCREENSHOT_DIR` overrides that location. The accompanying
`validation.json` lists assertions and marks the fixtures as synthetic.

No real account data, hostnames, tunnel IDs, installed-app actions, or live
connections are used. Browser processes and temporary profiles are cleaned up.

## Completed validation

- `npm run check`: zero errors and warnings.
- Local Rust tests: 58 passed, four live tests ignored.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `npx tauri build --bundles app`: macOS app bundle built successfully.
- Headless regression run: 120 assertions, including sequential guards,
  stable-port copies, host mapping, access scopes, startup snapshot ordering,
  error recovery, and hover/focus-paused notifications.
- Actual renders retain the approved grouping, access hierarchy, compact inactive
  Host control, and unwrapped port tools. The three canonical cards fit above
  the footer at 400 × 580. Windows light/dark checks emulate platform styles in
  Chromium; native Windows execution/material rendering was not tested.
- No GitHub Actions, push, merge, app installation, or live-tunnel operations.
