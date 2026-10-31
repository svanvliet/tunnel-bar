# Phase 3 UI hooks

These are the backend hooks for the Phase 3 management features. The Rust side is in `src-tauri/src`, and the TS types are in `src/lib/types.ts`. The UI has not been changed. Use "Tunnel Bar" in every user-facing string.

All commands use Tauri's camelCase argument names. Errors reject the `invoke` promise with a string.

## Guarded actions and error prefixes

`delete_tunnel`, `add_port` and `remove_port` refuse risky changes. The error message starts with a stable prefix, which `errorKind(e)` in `types.ts` maps to a kind. After the user confirms, retry with the matching flag. `stripErrorPrefix(e)` gives a message you can display.

| Prefix | Meaning | Retry with |
|---|---|---|
| `CONFIRM_REQUIRED: ` | The tunnel isn't labelled `tunnelbar`, so Tunnel Bar didn't create it. | `confirm: true` |
| `HOSTED_ELSEWHERE: ` | Another machine (for example a dev box) is hosting the tunnel. | `allowHostedElsewhere: true` |

You can also call `delete_preflight` first and ask before attempting the action.

Treat `allowHostedElsewhere` as a deliberate, separate confirmation. Never pass it by default. The user's SSH tunnel to their dev box is protected by this rule.

## Commands

### Tunnels

| Command | Args | Returns | Notes |
|---|---|---|---|
| `create_tunnel` | `request: CreateTunnelRequest` | tunnel id (`name.cluster`) | Validates input, then creates the tunnel. See below. |
| `update_tunnel` | `id`, `update: UpdateTunnelRequest` | — | Edits description, labels and expiration. See below. |
| `delete_preflight` | `id` | `DeletePreflight` | `{createdByApp, needsConfirm, hostedElsewhere, hostingHere, connectedHere}` |
| `delete_tunnel` | `id`, `confirm?`, `allowHostedElsewhere?` | — | Stops the local host and connect first. Also drops the tunnel's settings and pins. |

`create_tunnel`:
- Always adds the `tunnelbar` label.
- Adds `ports` and sets `anonymous` if requested.
- Starts hosting when `host: true`.
- Rolls back the tunnel if a later step fails.
- Validation:
  - `tunnelId`: 3–49 lowercase letters, digits or `-`, not starting or ending with `-`.
  - Labels: `[\w-=]{1,50}`.
  - Expiration: 1h–30d.

`update_tunnel`:
- `description: ""` clears the description.
- `null` or an omitted field leaves that field unchanged.
- The cache updates optimistically and a delayed refresh confirms it.
- No confirmation is needed.

`delete_tunnel`:
- The existing `delete_tunnel {id}` call still works for app-created tunnels.

### Ports

| Command | Args | Returns | Notes |
|---|---|---|---|
| `add_port` | `id`, `port`, `protocol?` (`"http"`, `"https"`, `"auto"`), `allowHostedElsewhere?` | — | See below. |
| `remove_port` | `id`, `port`, `confirm?`, `allowHostedElsewhere?` | — | See below. |

`add_port`:
- Restarts the local host, if Tunnel Bar is hosting the tunnel, so the new port is published.

`remove_port`:
- Needs `confirm` for tunnels Tunnel Bar didn't create.
- Restarts the local host.
- Removes any pin on that port.

### Pins and label filter

| Command | Args | Returns | Notes |
|---|---|---|---|
| `set_pinned_port` | `id`, `remotePort`, `localPort` (number, or `null` to unpin) | — | Rejects a local port that is already pinned elsewhere. |
| `set_label_filter` | `labels: string[]` | — | `[]` shows all tunnels. Labels are matched case-insensitively, and any one match counts. |

### Sign-in and hotkey

| Command | Args | Returns | Notes |
|---|---|---|---|
| `start_login` | `provider?` (`"microsoft"` default, `"github"`), `deviceCode?` | — | Runs `devtunnel user login` in the background. Progress appears in `Snapshot.login`. |
| `cancel_login` | — | — | Kills the running login. |
| `logout` | — | — | Runs `devtunnel user logout` and clears the tunnel list. Running connections keep running. |
| `set_hotkey` | `hotkey` (accelerator string, or `null` to disable) | — | Validates and registers the shortcut, then saves it and clears `hotkeyError`. |

`set_hotkey` accepts strings like `"CmdOrCtrl+Alt+Shift+T"`. A modifier is required.

`open_login` (open Terminal) is unchanged. Keep it as a fallback.

## New fields

**`TunnelView`**
- `expirationHours: number | null`: the tunnel's lifetime from the service. The CLI only reports a duration such as "30 days", not an absolute date. Display it with `expirationLabel(hours)`.
- `matchesFilter: boolean`: dim or hide the tunnel when this is false. The tray menu stays unfiltered.
- `pins: PinView[]`: each entry is `{remotePort, localPort, listening, targetPort, error}`.
  - `targetPort` is the port that `devtunnel connect` actually chose. It is null while the tunnel is disconnected.
  - `error` is set when the pinned port is busy. The proxy retries every 3 seconds.

**`TunnelSettings`**
- `pinnedPorts: Record<string, number>`, mapping remote port to local port.
- `update_tunnel_settings` replaces the whole object, so keep spreading the existing settings, as `saveTunnel` already does. Otherwise the pins are lost.

**`Settings`**
- `hotkey`: defaults to `CmdOrCtrl+Alt+Shift+T`.
- `labelFilter: string[]`

**`Snapshot`**
- `allLabels`: sorted, de-duplicated labels across all tunnels, for the filter chips.
- `login: LoginState | null`, one of:
  - `{kind:"running", provider}`
  - `{kind:"deviceCode", provider, url, code}`
  - `{kind:"succeeded", message}`
  - `{kind:"failed", message}`
- `hotkeyError: string | null`: set when the startup registration failed, for example because another app owns the shortcut.

## UI notes

- **Sign-in**
  - Offer "Sign in with Microsoft / GitHub" in-app.
  - On `deviceCode`, show the code with a copy button, then open `url` with `open_url`.
  - The browser flow (`deviceCode: false`) opens the system browser itself.
  - After `succeeded`, the backend refreshes and auto-connects.
- **Pins**
  - When a forward has a pin, show and copy `localhost:<pin.localPort>`, not the forward's `localPort`. For example, Copy local URL should use the pinned port.
  - `ssh_command` and `open_ssh` already switch to the pinned port, so callers can keep passing the forward's ports.
  - The pin stays stable across reconnects. A `~/.ssh/config` entry with `Port <pin>` can therefore stay static.
- **Expiration**
  - Valid choices are 1h–30d (for example 1h, 8h, 1d, 7d, 30d).
- **Creating a tunnel**
  - The existing "Expose local port" flow is unchanged (`expose_port`). It now goes through `create_tunnel` internally.
  - A full "New tunnel" form can call `create_tunnel` directly.
