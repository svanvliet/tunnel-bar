export type ConnState =
  | { kind: "disconnected" }
  | { kind: "connecting" }
  | { kind: "connected" }
  | { kind: "reconnecting" }
  | { kind: "backoff"; attempt: number; retryInSecs: number; message: string }
  | { kind: "authRequired"; message: string }
  | { kind: "external"; pid: number };

export interface Forward { localPort: number; remotePort: number }
export interface TunnelPort { portNumber: number; protocol: string; portUri: string }
export interface TunnelSettings { nickname?: string | null; autoConnect: boolean; autoHost: boolean; sshHost?: string | null }
export interface HostedPort { port: number; protocol: string; uri: string; inspectUri: string; listening: boolean }
export interface TunnelSummary {
  tunnelId: string; name: string; hostConnections: number; labels: string[];
  portCount: number; tunnelExpiration: string; description: string;
}
export interface TunnelView {
  id: string; displayName: string; summary: TunnelSummary; ports: TunnelPort[];
  state: ConnState; forwards: Forward[]; settings: TunnelSettings; orphaned: boolean;
  hostState: ConnState; hostedPorts: HostedPort[]; anonymous: boolean; createdByApp: boolean; hostedElsewhere: boolean;
}
export interface Settings {
  cliPath?: string | null; launchAtLogin: boolean; notifications: boolean;
  firstRunDone: boolean; tunnels: Record<string, TunnelSettings>;
}
export interface Snapshot {
  cliPath?: string | null; cliVersion?: string | null;
  auth?: { loggedIn: boolean; message: string } | null;
  tunnels: TunnelView[]; settings: Settings; lastRefresh?: string | null;
  refreshError?: string | null; refreshing: boolean; logDir: string;
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
    case "external": return `Connected outside TunnelBar (pid ${s.pid})`;
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
    case "external": return `Hosted outside TunnelBar (pid ${s.pid})`;
    default: return stateLabel(s);
  }
}
