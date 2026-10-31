import type { Forward, TunnelView } from "./types";

export interface GuardFlags { confirm?: boolean; allowHostedElsewhere?: boolean }
export interface Guard { kind: "confirm" | "hostedElsewhere"; message: string }
export type ManagementResult = { ok: true } | { ok: false; guard?: Guard };
export type ManagementCommand = "update_tunnel" | "add_port" | "remove_port" | "set_port_headers" |
  "add_access_rule" | "remove_access_rule" | "set_pinned_port" | "set_port_map" | "delete_tunnel";
export type Manage = (command: ManagementCommand, args: Record<string, unknown>, title: string) => Promise<ManagementResult>;
export const localPort = (t: TunnelView, f: Forward) =>
  t.pins.find((pin) => pin.remotePort === f.remotePort)?.localPort ??
  t.settings.pinnedPorts?.[String(f.remotePort)] ?? f.localPort;
