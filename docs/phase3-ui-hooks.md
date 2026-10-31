# Phase 3 UI hooks

These are the backend hooks for the Phase 3 management features. The Rust side is in `src-tauri/src`, and the TS types are in `src/lib/types.ts`. The UI has not been changed. Use "Tunnel Bar" in every user-facing string.

All commands use Tauri's camelCase argument names. Errors reject the `invoke` promise with a string.

## Guarded actions and error prefixes

`delete_tunnel`, `add_port`, `remove_port`, `set_port_headers`, expiration and header changes in `update_tunnel`, `set_anonymous`, and `add_access_rule`/`remove_access_rule` refuse risky changes. The error message starts with a stable prefix, which `errorKind(e)` in `types.ts` maps to a kind. After the user confirms, retry with the matching flag. `stripErrorPrefix(e)` gives a message you can display.

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
| `update_tunnel` | `id`, `update: UpdateTunnelRequest`, `confirm?`, `allowHostedElsewhere?` | — | Edits description, labels, expiration and tunnel-level Host/Origin headers. See below. |
| `delete_preflight` | `id` | `DeletePreflight` | `{createdByApp, needsConfirm, hostedElsewhere, hostingHere, connectedHere}` |
| `delete_tunnel` | `id`, `confirm?`, `allowHostedElsewhere?` | — | Stops the local host and connect first. Also drops the tunnel's settings and pins. |

`create_tunnel`:
- Always adds the `tunnelbar` label.
- Adds `ports` (each with optional `hostHeader`/`originHeader`) and sets `anonymous` if requested.
- Tunnel-level `hostHeader`/`originHeader` go on `devtunnel create`; ports inherit them.
- `accessRules` are applied after the ports. A port-level rule must name a port in `ports`.
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
- On a tunnel Tunnel Bar didn't create:
  - Adding or removing the `tunnelbar` label is always refused, because that label is what marks a tunnel as Tunnel Bar's and gates every confirmation.
  - Changing `expiration` needs `confirm`, because the service deletes the tunnel when it expires.
- Other edits need no confirmation, except for header fields: those need `allowHostedElsewhere` when another machine hosts the tunnel, and restart our host if it's running.

`delete_tunnel`:
- Re-reads the tunnel with `devtunnel show` before the guards run, so the label and host count are current.
- "Hosted elsewhere" means the host count is greater than the one host Tunnel Bar runs, so a dev box hosting alongside this Mac still counts.
- The existing `delete_tunnel {id}` call still works for app-created tunnels.

### Ports

| Command | Args | Returns | Notes |
|---|---|---|---|
| `add_port` | `id`, `port`, `protocol?` (`"http"`, `"https"`, `"auto"`), `hostHeader?`, `originHeader?`, `allowHostedElsewhere?`, `remotePort?` | — | See below. |
| `set_port_map` | `id`, `remotePort`, `localPort` (number, or `null` to clear) | — | Hosting: forward tunnel port `remotePort` to `localhost:localPort`. See Port mapping below. |
| `set_port_headers` | `id`, `port`, `headers: HeaderOptions`, `allowHostedElsewhere?` | — | Changes one port's Host/Origin rewriting (`devtunnel port update`). Restarts our host. |
| `remove_port` | `id`, `port`, `confirm?`, `allowHostedElsewhere?` | — | See below. |

`add_port`:
- `port` is the local server port. `remotePort`, when set and different, is the tunnel port clients see (e.g. `port: 22, remotePort: 2222`); the mapping is saved in `settings.hostPortMap`.
- Restarts the local host, if Tunnel Bar is hosting the tunnel, so the new port is published.

`remove_port`:
- Needs `confirm` for tunnels Tunnel Bar didn't create.
- Restarts the local host.
- Removes any pin and port map on that port.

### Port mapping (hosting)

`devtunnel host` always forwards a tunnel port to `localhost:<same number>`. To advertise a different number (tunnel port 2222 → local SSH on 22), Tunnel Bar runs a loopback proxy on `127.0.0.1`/`::1:<tunnel port>` → `localhost:<local port>` whenever this Mac hosts the tunnel. It starts and stops with hosting (Host button, `autoHost`, reclaimed hosts on startup, port changes).

- Where: `expose_port` (`remotePort?`), `add_port` (`remotePort?`), `create_tunnel` (`ports[].remotePort`), `set_port_map` for existing ports.
- Refused when something already listens on the tunnel port locally (loopback, `0.0.0.0` or `[::]`), because `devtunnel host` would reach that server instead. `host` refuses for the same reason; otherwise `portMaps[].error` explains why the proxy can't listen and it retries every 3 seconds.
- Only affects hosting on this Mac. Clients still see the tunnel port number; another machine hosting the tunnel doesn't use the map.

### Host/Origin header rewriting

By default the service rewrites `Host` to `localhost` and `Origin` to `http(s)://localhost`. Each header can be set to `"unchanged"` (`HEADER_UNCHANGED`), which passes the browser's header through, or to a literal value. Values are trimmed and must not contain spaces; `""`/omitted leaves the setting alone.

- Offer a one-click "Dev server (Vite/Next)" preset that sets both headers to `unchanged`. Apps that build absolute URLs or redirects, or compare `Origin` with `Host` (for example Next.js Server Actions and OAuth callbacks), need the real public hostname. With `Host: unchanged`, Vite also needs the tunnel host in `server.allowedHosts`, for example `.devtunnels.ms`. Mention that next to the preset.
- Where: `create_tunnel` (tunnel and per port), `expose_port` (`hostHeader?`, `originHeader?`, `remotePort?`), `add_port`, `set_port_headers`, `update_tunnel`.
- Display: `TunnelPort.hostHeader`/`originHeader` show the effective value per port (parsed from `show`, inherited from the tunnel); `null` means the service default. The tunnel-level setting itself isn't reported by `devtunnel show`.

### Access rules

| Command | Args | Returns | Notes |
|---|---|---|---|
| `list_access_rules` | `id`, `port?` | `AccessEntry[]` | Live `devtunnel access list`. With `port`, tunnel-level rules appear with `isInherited: true` and no `index`. |
| `add_access_rule` | `id`, `rule: AccessRuleRequest`, `confirm?` | — | `devtunnel access create`. Needs `confirm` on tunnels Tunnel Bar didn't create. |
| `remove_access_rule` | `id`, `port?`, `index`, `confirm?` | — | Deletes the own rule at `index` (from `AccessEntry.index`). Same `confirm` rule. |

`AccessRuleRequest` is `{kind, value?, deny?, expiration?, port?}`:
- `kind: "anonymous"`: anyone (no value). The existing `set_anonymous {id, allow, confirm?}` toggle still works and only touches anonymous entries. Like the other access commands, it now needs `confirm` on tunnels Tunnel Bar didn't create. The current `+page.svelte` call doesn't pass it, so on such tunnels it gets a `CONFIRM_REQUIRED` error until the UI retries with `confirm: true`.
- `kind: "tenant"`: everyone in the signed-in user's Entra tenant (no value).
- `kind: "org"`, `value: "<github-org>"`: needs a **GitHub** sign-in. With a Microsoft sign-in the CLI fails with "must use the same identity provider", which the backend rewrites to "Organization rules need a GitHub sign-in…".
- `kind: "repo"`, `value: "owner/repo"`, ADO `org/project/repo`, or a URL. CLI 1.0.2115 rejected every form under a Microsoft sign-in ("Specify access control parameters"); the backend rewrites that error. Treat repo rules as best-effort and surface the error.
- `deny: true` makes a deny rule. `expiration` is 1h–30d. Note: the service caps tenant-rule expiry by its own policy; `AccessEntry.expiration` shows what the service actually stored, which may differ from the requested `-e`.
- `port` makes a port-level rule; omit for tunnel-level.

`TunnelView.accessRules` holds the tunnel-level entries from the last refresh, and is reloaded right after add/remove. Display `type` (`Anonymous`, `Organizations` = tenant, `GitHubOrganization`/`GitHubRepository`, …), `isDeny`, and `expiration`. `subjects` holds tenant ids / org ids, not friendly names.

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
- `accessRules: AccessEntry[]`: tunnel-level access rules. See Access rules above.
- `portMaps: PortMapView[]`: `{remotePort, localPort, listening, error}` per hosting port map. `listening` is the proxy; it is only true while this Mac hosts.
- `hostedPorts[].localPort`: the server a hosted port reaches. `listening` now refers to that port, so "nothing listening" warnings name `localhost:<localPort>`.
- `pins: PinView[]`: each entry is `{remotePort, localPort, listening, targetPort, error}`.
  - `targetPort` is the port that `devtunnel connect` actually chose. It is null while the tunnel is disconnected.
  - `error` is set when the pinned port is busy. The proxy retries every 3 seconds.
  - A server listening on `0.0.0.0:<port>` or `[::]:<port>` (for example a dev server) counts as busy. macOS would otherwise let the pin take over that server's `localhost` traffic.

**`TunnelPort`**
- `hostHeader`, `originHeader`: effective header rewriting (`"unchanged"`, a value, or `null` = default).
- `localPort: number | null`: the mapped local server port; show "2222 → localhost:22". `null` means the same number.

**`TunnelSettings`**
- `pinnedPorts: Record<string, number>`, mapping remote port to local port.
- `hostPortMap: Record<string, number>`, mapping tunnel port to local server port for hosting.
- `update_tunnel_settings` replaces the whole object, so keep spreading the existing settings, as `saveTunnel` already does. Otherwise the pins and port maps are lost.

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
  - The existing "Expose local port" flow is unchanged (`expose_port`). It now goes through `create_tunnel` internally, and accepts optional `name`, `hostHeader`, `originHeader`, `accessRules` and `remotePort` (advertise a different tunnel port) (e.g. `[{kind:"tenant"}]` for "my org only").
  - A full "New tunnel" form can call `create_tunnel` directly.
