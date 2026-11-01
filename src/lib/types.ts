export type ConnState =
  | { kind: "disconnected" }
  | { kind: "connecting" }
  | { kind: "connected" }
  | { kind: "reconnecting" }
  | { kind: "backoff"; attempt: number; retryInSecs: number; message: string }
  | { kind: "authRequired"; message: string }
  | { kind: "external"; pid: number };

export interface Forward { localPort: number; remotePort: number }
export interface TunnelPort {
  portNumber: number; protocol: string; portUri: string;
  /** Effective Host/Origin rewriting ("unchanged" or a value; inherited from the tunnel). null = service default (localhost). */
  hostHeader?: string | null; originHeader?: string | null;
  /** Local server port when this tunnel port is mapped for hosting (`settings.hostPortMap`); null = same number. */
  localPort?: number | null;
}
/** Host/Origin header rewriting for web clients. "unchanged" keeps the browser's header (Vite/Next dev servers need this for Host). Omit = leave as is. */
export interface HeaderOptions { hostHeader?: string | null; originHeader?: string | null }
export const HEADER_UNCHANGED = "unchanged";
/** One `devtunnel access` entry. `index` (own entries only) is what `remove_access_rule` takes; inherited (tunnel-level, seen on a port) entries have none. */
export interface AccessEntry {
  type: string; provider: string; subjects: string[]; scopes: string[];
  isDeny: boolean; isInherited: boolean; expiration?: string | null; index?: number | null;
}
export type AccessRuleKind = "anonymous" | "tenant" | "org" | "repo";
export interface AccessRuleRequest {
  kind: AccessRuleKind;
  /** org: GitHub org; repo: owner/repo, ADO org/project/repo or URL; none for anonymous/tenant. */
  value?: string | null;
  deny?: boolean;
  /** "2h", "7d", … (1h–30d); permanent when omitted. */
  expiration?: string | null;
  /** Port-level rule; tunnel-level when omitted. */
  port?: number | null;
}
export interface TunnelSettings {
  nickname?: string | null; autoConnect: boolean; autoHost: boolean; sshHost?: string | null;
  /** Stable local ports: remote port -> local port (keys are stringified numbers). Prefer `set_pinned_port`. */
  pinnedPorts?: Record<string, number>;
  /** Hosting: tunnel port -> local server port (keys are stringified numbers). Prefer `set_port_map`. */
  hostPortMap?: Record<string, number>;
}
/** A hosting port map: while this Mac hosts, 127.0.0.1:`remotePort` forwards to localhost:`localPort`. */
export interface PortMapView { remotePort: number; localPort: number; listening: boolean; error?: string | null }
/** A stable-port proxy: `localPort` always forwards to the port `devtunnel connect` picked for `remotePort`. */
export interface PinView { remotePort: number; localPort: number; listening: boolean; targetPort?: number | null; error?: string | null }
/** `port` is the tunnel port; `localPort` the server it reaches (differs when mapped); `listening` is about `localPort`. */
export interface HostedPort { port: number; protocol: string; uri: string; inspectUri: string; localPort: number; listening: boolean }
export interface TunnelSummary {
  tunnelId: string; name: string; hostConnections: number; labels: string[];
  portCount: number; tunnelExpiration: string; description: string;
}
export interface TunnelView {
  id: string; displayName: string; summary: TunnelSummary; ports: TunnelPort[];
  state: ConnState; forwards: Forward[]; settings: TunnelSettings; orphaned: boolean;
  hostState: ConnState; hostedPorts: HostedPort[]; anonymous: boolean; createdByApp: boolean; hostedElsewhere: boolean;
  /** "live": this device hosts it now (Tunnel Bar or an external host); "last": it hosted it last. */
  hostedHere?: "no" | "live" | "last";
  /** Tunnel lifetime in hours (e.g. 720 for "30 days"); null if unknown. */
  expirationHours?: number | null;
  /** False when `settings.labelFilter` is set and this tunnel has none of those labels. */
  matchesFilter: boolean;
  pins: PinView[];
  /** Tunnel-level access rules (from the last refresh). */
  accessRules: AccessEntry[];
  /** Hosting port maps (tunnel port != local port) and their proxy status. */
  portMaps: PortMapView[];
}
export interface Settings {
  cliPath?: string | null; launchAtLogin: boolean; notifications: boolean;
  firstRunDone: boolean; tunnels: Record<string, TunnelSettings>;
  /** Global shortcut (Tauri accelerator) that toggles the popover; null = disabled. Change via `set_hotkey`. */
  hotkey?: string | null;
  /** Show only tunnels with any of these labels (empty = all). Change via `set_label_filter`. */
  labelFilter: string[];
}
export interface Snapshot {
  cliPath?: string | null; cliVersion?: string | null;
  auth?: { loggedIn: boolean; message: string } | null;
  tunnels: TunnelView[]; settings: Settings; lastRefresh?: string | null;
  refreshError?: string | null; refreshing: boolean; logDir: string;
  allLabels: string[];
  login?: LoginState | null;
  hotkeyError?: string | null;
  adminDevtunnel?: boolean;
  /** Self-update state. Change via `check_for_updates` / `install_update`. */
  update?: UpdateState;
}

export type UpdateStatus = "idle" | "checking" | "upToDate" | "downloading" | "ready" | "installing" | "error";
export interface UpdateState {
  status: UpdateStatus; currentVersion: string;
  /** False in development builds, which never update. */
  enabled: boolean;
  /** Version being downloaded or ready to install. */
  version?: string | null; notes?: string | null;
  /** Download progress 0–1 while `downloading`. */
  progress?: number | null;
  lastChecked?: string | null; error?: string | null;
}

/** `port` is the local server port; `remotePort` (optional) the tunnel port to advertise instead. */
export interface PortSpec extends HeaderOptions { port: number; remotePort?: number | null; protocol?: "http" | "https" | "auto" | "" }
export interface CreateTunnelRequest extends HeaderOptions {
  tunnelId?: string | null; description?: string; labels?: string[];
  /** "8h", "30d", … (1h–30d); service default (30 days) when omitted. */
  expiration?: string | null;
  ports?: PortSpec[]; anonymous?: boolean; host?: boolean; nickname?: string | null;
  /** Applied after ports/anonymous; port-level rules must name a port in `ports`. */
  accessRules?: AccessRuleRequest[];
}
/** Header fields change tunnel-level rewriting (needs `allowHostedElsewhere` if hosted elsewhere; restarts our host). */
export interface UpdateTunnelRequest extends HeaderOptions {
  /** null/omitted = unchanged; "" clears it. */
  description?: string | null;
  addLabels?: string[]; removeLabels?: string[];
  expiration?: string | null;
}
export interface DeletePreflight {
  createdByApp: boolean; needsConfirm: boolean; hostedElsewhere: boolean; hostingHere: boolean; connectedHere: boolean;
}
export type LoginProvider = "microsoft" | "github";
export type LoginState =
  | { kind: "running"; provider: LoginProvider }
  | { kind: "deviceCode"; provider: LoginProvider; url: string; code: string }
  | { kind: "succeeded"; message: string }
  | { kind: "failed"; message: string };

/** Error-message prefixes from delete_tunnel / add_port / remove_port: retry with the matching flag after asking. */
export const CONFIRM_REQUIRED = "CONFIRM_REQUIRED: ";
export const HOSTED_ELSEWHERE = "HOSTED_ELSEWHERE: ";
export const DEFAULT_HOTKEY = "CmdOrCtrl+Alt+Shift+T";

export function errorKind(e: unknown): "confirm" | "hostedElsewhere" | null {
  const m = String(e);
  if (m.includes(CONFIRM_REQUIRED)) return "confirm";
  if (m.includes(HOSTED_ELSEWHERE)) return "hostedElsewhere";
  return null;
}
export const stripErrorPrefix = (e: unknown) => String(e).replace(CONFIRM_REQUIRED, "").replace(HOSTED_ELSEWHERE, "");

export function expirationLabel(hours?: number | null): string | null {
  if (hours == null) return null;
  if (hours % 24 === 0) return `${hours / 24} day${hours === 24 ? "" : "s"}`;
  return `${hours} hour${hours === 1 ? "" : "s"}`;
}
export interface LogLine { ts: string; stream: string; text: string }

export function stateLabel(s: ConnState): string {
  switch (s.kind) {
    case "disconnected": return "Disconnected";
    case "connecting": return "Connecting…";
    case "connected": return "Connected";
    case "reconnecting": return "Reconnecting…";
    case "backoff": return `Retrying in ${s.retryInSecs}s (attempt ${s.attempt})`;
    case "authRequired": return "Sign-in required";
    case "external": return `External connection (pid ${s.pid})`;
  }
}

export function tone(s: ConnState): "ok" | "busy" | "bad" | "off" {
  switch (s.kind) {
    case "connected": case "external": return "ok";
    case "connecting": case "reconnecting": return "busy";
    case "backoff": case "authRequired": return "bad";
    default: return "off";
  }
}

export const isActive = (s: ConnState) => s.kind !== "disconnected" && s.kind !== "authRequired" && s.kind !== "external";

export function hostLabel(s: ConnState): string {
  switch (s.kind) {
    case "connecting": return "Starting host…";
    case "connected": return "Hosting";
    case "reconnecting": return "Host reconnecting…";
    case "external": return `External host (pid ${s.pid})`;
    default: return stateLabel(s);
  }
}
