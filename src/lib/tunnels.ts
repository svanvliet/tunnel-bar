import type { TunnelView, Forward } from "./types";

export type TunnelRole = "remote" | "local";
export type TunnelAction = "connect" | "disconnect" | "host" | "stop_host" | "adopt_external" | "stop_external" | "delete_tunnel" | "set_anonymous";
export const hostOn = (t: TunnelView) => t.hostState.kind !== "disconnected";
export const canHost = (t: TunnelView) => !t.hostedElsewhere && !hostOn(t) && !t.orphaned;
export const remoteTunnels = (tunnels: TunnelView[]) => tunnels.filter((t) => !t.createdByApp || t.state.kind !== "disconnected");
export const localTunnels = (tunnels: TunnelView[]) => tunnels.filter((t) => t.createdByApp || hostOn(t));
export const isSsh = (f: Forward, t: TunnelView) =>
  f.remotePort === 22 || f.remotePort === 2222 || !!t.settings.sshHost ||
  t.ports.some((p) => p.portNumber === f.remotePort && p.protocol === "ssh");
