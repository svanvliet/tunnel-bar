<script lang="ts">
  import { tick } from "svelte";
  import { fade } from "svelte/transition";
  import Icon from "./Icon.svelte";
  import { hostOn, canHost, isSsh } from "./tunnels";
  import type { TunnelAction, TunnelRole } from "./tunnels";
  import { stateLabel, hostLabel, tone, isActive } from "./types";
  import type { TunnelView, Forward, HostedPort, TunnelSettings } from "./types";
  import { motionDuration } from "./motion";
  import { localPort } from "./management";
  import type { Guard, GuardFlags, ManagementResult } from "./management";

  let { t, role, ready, busy, reduced, copied, onAction, onLogs, onSave, onCopySsh, onCopyAddress, onCopyUrl, onOpenSsh, onOpenUrl, onDetails }: {
    t: TunnelView; role: TunnelRole; ready: boolean; busy: boolean; reduced: boolean; copied: string | null;
    onAction: (t: TunnelView, action: TunnelAction, allow?: boolean, flags?: GuardFlags) => Promise<ManagementResult>;
    onDetails: (t: TunnelView) => void;
    onLogs: (t: TunnelView) => Promise<void>;
    onSave: (t: TunnelView, patch: Partial<TunnelSettings>) => Promise<boolean>;
    onCopySsh: (t: TunnelView, f: Forward) => Promise<void>;
    onCopyAddress: (t: TunnelView, f: Forward) => Promise<void>;
    onCopyUrl: (t: TunnelView, p: HostedPort) => Promise<void>;
    onOpenSsh: (t: TunnelView, f: Forward) => Promise<void>;
    onOpenUrl: (url: string) => Promise<void>;
  } = $props();
  let details = $state(false);
  let editing = $state(false);
  let draft = $state("");
  let confirmation = $state<"public" | "delete" | null>(null);
  let guard = $state<Guard | null>(null);
  let guardFlags = $state<GuardFlags>({});
  let accessAllow = $state(true);
  let nickInput = $state<HTMLInputElement>();
  let nameButton = $state<HTMLButtonElement>();
  let optionsButton = $state<HTMLButtonElement>();
  let confirmationBox = $state<HTMLDivElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let savingName = false;
  const local = $derived(role === "local");
  const connectionState = $derived(local ? t.hostState : t.state);
  const stateTone = $derived(tone(connectionState));
  const managedHost = $derived(hostOn(t) && t.hostState.kind !== "external");
  const displayState = $derived(local && connectionState.kind === "disconnected" ? "Not hosting" : local ? hostLabel(connectionState) : stateLabel(connectionState));

  async function rename() {
    draft = t.settings.nickname ?? "";
    details = false;
    editing = true;
    await tick();
    nickInput?.focus();
    nickInput?.select();
  }
  async function finishRename() {
    if (!editing || savingName) return;
    savingName = true;
    const ok = await onSave(t, { nickname: draft.trim() || null });
    savingName = false;
    if (ok) { editing = false; await tick(); nameButton?.focus(); }
  }
  async function confirm() {
    const action = confirmation;
    if (!action) return;
    const flags = { ...guardFlags, ...(guard?.kind === "confirm" ? { confirm: true } : guard?.kind === "hostedElsewhere" ? { allowHostedElsewhere: true } : {}) };
    const result = await onAction(t, action === "delete" ? "delete_tunnel" : "set_anonymous", accessAllow, flags);
    if (!result.ok && result.guard) { guard = result.guard; guardFlags = flags; await tick(); cancelButton?.focus(); }
    if (result.ok) {
      confirmation = null;
      await tick();
      optionsButton?.focus();
    }
  }
  async function showConfirmation(action: "public" | "delete") {
    details = false;
    guard = null; guardFlags = {};
    accessAllow = true;
    confirmation = action;
    await tick();
    confirmationBox?.scrollIntoView({ block: "nearest", behavior: reduced ? "auto" : "smooth" });
    cancelButton?.focus({ preventScroll: true });
  }
  async function requireSignIn() {
    const result = await onAction(t, "set_anonymous", false);
    if (!result.ok && result.guard) {
      await showConfirmation("public");
      accessAllow = false;
      guard = result.guard;
    }
  }
  function escape(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (editing) { event.stopPropagation(); editing = false; tick().then(() => nameButton?.focus()); }
    else if (confirmation || details) { event.stopPropagation(); confirmation = null; details = false; optionsButton?.focus(); }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<article class="tunnel-card" class:inactive={connectionState.kind === "disconnected"} class:attention={connectionState.kind === "authRequired"} data-tunnel={t.id} data-role={role} onkeydown={escape}>
  <div class="card-heading">
    <span class="role-icon {stateTone}"><Icon name={local ? "laptop" : "server"} /></span>
    <div class="tunnel-heading">
      {#if editing}
        <label class="sr-only" for={"nickname-" + role + "-" + t.id}>Local nickname</label>
        <input id={"nickname-" + role + "-" + t.id} class="nickname" bind:this={nickInput} bind:value={draft}
          placeholder={t.summary.name || t.id} disabled={busy}
          onblur={finishRename} onkeydown={(event) => { if (event.key === "Enter") { event.preventDefault(); finishRename(); } }} />
      {:else}
        <button class="tunnel-name" bind:this={nameButton} title={t.displayName + " — double-click to rename"} ondblclick={rename}>{t.displayName}</button>
      {/if}
      <div class="tunnel-state {stateTone}" title={"message" in connectionState ? connectionState.message : displayState}>
        <span class="state-dot"></span><span>{displayState}</span>
        {#if connectionState.kind === "connected" || connectionState.kind === "disconnected"}
          <span class="host-availability">· {local && managedHost ? "This device" : t.summary.hostConnections > 0 ? "Host online" : "Host offline"}</span>
        {/if}
      </div>
    </div>
    {#if local && !hostOn(t)}
      <button class="primary" disabled={busy || !ready || !canHost(t)} title={t.hostedElsewhere ? "Hosted by another device" : t.orphaned ? "Tunnel is not on this account" : "Host this tunnel"} onclick={() => onAction(t, "host")}>Host</button>
    {/if}
    <button class="icon-button options" bind:this={optionsButton} title="Tunnel details and actions" aria-label={"Details for " + t.displayName} aria-expanded={details}
      onclick={() => { details = !details; confirmation = null; }}><Icon name="more" /></button>
  </div>

  {#if t.orphaned}<div class="inline-warning"><Icon name="warning" size={13} />Not on this account</div>{/if}
  {#if connectionState.kind === "backoff" || connectionState.kind === "authRequired"}
    <div class="inline-warning" class:error-text={connectionState.kind === "authRequired"}><Icon name="warning" size={13} /><span>{connectionState.message}</span></div>
  {/if}

  <div class="access-line" class:public-access={t.anonymous}>
    <Icon name={t.anonymous ? "globe" : "lock"} size={12} />
    <span>{t.anonymous ? "Public · anyone with URL" : t.accessRules.some((entry) => !entry.isDeny && entry.type !== "Anonymous") ? "Private · access rules" : "Private · account only"}</span>
    {#if local && managedHost}
      <button class="text-button access-action" disabled={busy || !ready} onclick={() => t.anonymous ? requireSignIn() : showConfirmation("public")}>
        {t.anonymous ? "Require sign-in" : "Make public…"}
      </button>
    {:else if local && !hostOn(t)}
      <span class="spacer"></span>
      <label class="auto-toggle" title="Host automatically when Tunnel Bar starts">
        <input type="checkbox" checked={t.settings.autoHost} disabled={busy} onchange={(event) => onSave(t, { autoHost: event.currentTarget.checked })} />Auto-host
      </label>
    {/if}
  </div>

  {#if local}
    {#each t.hostedPorts as p (p.port)}
      <div class="endpoint">
        <code>:{p.port}</code><Icon name="arrow" size={14} />
        {#if p.localPort !== p.port}<code>localhost:{p.localPort}</code>{/if}
        {#if p.uri}
          <button class="endpoint-url text-button" title={p.uri} onclick={() => onOpenUrl(p.uri)}>{p.uri.replace(/^https:\/\//, "").replace(/\/$/, "")}</button>
          <button class="icon-button endpoint-tool" title="Copy full URL" aria-label={"Copy URL for port " + p.port} disabled={busy} onclick={() => onCopyUrl(t, p)}>
            <Icon name={copied === "url:" + t.id + ":" + p.port ? "check" : "copy"} />
          </button>
          {#if p.inspectUri}
            <button class="icon-button endpoint-tool" title="Inspect traffic" aria-label={"Inspect traffic for port " + p.port} onclick={() => onOpenUrl(p.inspectUri)}><Icon name="inspect" /></button>
          {/if}
        {:else}<span class="hint">Public URL pending…</span>{/if}
      </div>
      {#if !p.listening && t.hostState.kind === "connected"}
        <div class="inline-warning"><Icon name="warning" size={13} /><span>Nothing is listening on localhost:{p.localPort}</span></div>
      {/if}
    {/each}
    {#each t.portMaps.filter((map) => map.error) as map}
      <div class="inline-warning error-text"><Icon name="warning" size={13} /><span>Host mapping :{map.remotePort} → localhost:{map.localPort}: {map.error}</span></div>
    {/each}
    {#if t.hostState.kind === "connected" && !t.hostedPorts.length}
      <div class="inline-warning"><Icon name="warning" size={13} />No ports on this tunnel — nothing to serve.</div>
    {/if}
  {:else}
    {#each t.forwards as f (f.localPort)}
      <div class="endpoint">
        <button class="text-button address" title="Copy local address" onclick={() => onCopyAddress(t, f)}>localhost:{localPort(t, f)}</button><Icon name="arrow" size={14} /><code>:{f.remotePort}</code>
        <span class="spacer"></span>
        {#if isSsh(f, t)}
          <button class="icon-button endpoint-tool" title="Copy SSH command" aria-label={"Copy SSH command for port " + f.remotePort} disabled={busy} onclick={() => onCopySsh(t, f)}>
            <Icon name={copied === "ssh:" + t.id + ":" + f.localPort ? "check" : "copy"} />
          </button>
          {#if t.state.kind === "external"}
            <button class="icon-button endpoint-tool" title="Open SSH" aria-label={"Open SSH for port " + f.remotePort} disabled={busy} onclick={() => onOpenSsh(t, f)}><Icon name="terminal" /></button>
          {/if}
        {/if}
      </div>
    {/each}
    {#each t.pins.filter((pin) => t.forwards.some((f) => f.remotePort === pin.remotePort)) as pin}
      <div class="port-summary" class:error-text={!!pin.error}>{pin.error || (pin.listening ? `Stable port :${pin.localPort} · proxy ready` : `Stable port :${pin.localPort} · waiting for connection`)}</div>
    {/each}
    {#if !t.forwards.length && t.ports.length && !isActive(t.state) && t.state.kind !== "external"}
      <div class="port-summary">Ports: {t.ports.map((p) => `${p.portNumber}${p.protocol && p.protocol !== "auto" ? "/" + p.protocol : ""}`).join(", ")}</div>
    {/if}
  {/if}

  {#if confirmation}
    <div class="confirmation" bind:this={confirmationBox} transition:fade={{ duration: motionDuration(reduced, 160) }}>
      <strong>{guard?.kind === "hostedElsewhere" ? "Another machine is hosting this tunnel" : guard?.kind === "confirm" ? "Tunnel Bar didn't create this tunnel" : confirmation === "delete" ? "Delete this tunnel?" : "Make this tunnel public?"}</strong>
      <p>{guard ? guard.message + (guard.kind === "hostedElsewhere" ? " This can interrupt the other device’s SSH connection. Confirm the override separately." : "") : confirmation === "delete" ? "This removes it from your account. Its URL stops working." : "Anyone with the URL can connect without signing in. This changes access for every port on this tunnel."}</p>
      <div class="button-row">
        <button bind:this={cancelButton} disabled={busy} onclick={() => { confirmation = null; optionsButton?.focus(); }}>Cancel</button>
        <button class:danger={confirmation === "delete"} class:primary={confirmation === "public"} disabled={busy || !ready} onclick={confirm}>
          {busy ? "Working…" : guard?.kind === "hostedElsewhere" ? "Override remote host" : guard ? "Confirm change" : confirmation === "delete" ? "Delete tunnel" : "Make public"}
        </button>
      </div>
    </div>
  {/if}

  {#if !local || hostOn(t)}
  <div class="card-actions">
    {#if local}
      {#if t.hostState.kind === "external"}
        <span class="hint">Hosted by another process</span>
        <button disabled={busy || !ready} title="Stop the external host and host it from Tunnel Bar" onclick={() => onAction(t, "adopt_external")}>Take over</button>
        <button class="quiet" disabled={busy} onclick={() => onAction(t, "stop_external")}>Stop</button>
      {:else if managedHost}
        <button disabled={busy} onclick={() => onAction(t, "stop_host")}>Stop hosting</button>
      {:else}
        <button class="primary" disabled={busy || !ready || !canHost(t)} title={t.hostedElsewhere ? "Hosted by another device" : t.orphaned ? "Tunnel is not on this account" : "Host this tunnel"}
          onclick={() => onAction(t, "host")}>Host</button>
      {/if}
      <span class="spacer"></span>
      <label class="auto-toggle" title="Host automatically when Tunnel Bar starts">
        <input type="checkbox" checked={t.settings.autoHost} disabled={busy} onchange={(event) => onSave(t, { autoHost: event.currentTarget.checked })} />Auto-host
      </label>
    {:else}
      {#if t.state.kind === "external"}
        <button disabled={busy || !ready} title="Stop the external process and manage its connection here" onclick={() => onAction(t, "adopt_external")}>Take over</button>
        <button class="quiet" disabled={busy} onclick={() => onAction(t, "stop_external")}>Stop</button>
      {:else if isActive(t.state)}
        {@const sshForwards = t.forwards.filter((f) => isSsh(f, t))}
        {#if sshForwards.length === 1}
          <button class="primary" disabled={busy || t.state.kind !== "connected"} onclick={() => onOpenSsh(t, sshForwards[0])}><Icon name="terminal" size={14} />Open SSH</button>
        {/if}
        <button class="quiet" disabled={busy} onclick={() => onAction(t, "disconnect")}>Disconnect</button>
      {:else if t.hostState.kind === "external"}
        <!-- Another process hosts this tunnel on this machine; connecting to our own host makes no sense. -->
        <button disabled={busy || !ready} title="Stop the external host and host it from Tunnel Bar" onclick={() => onAction(t, "adopt_external")}>Take over</button>
        <button class="quiet" disabled={busy} onclick={() => onAction(t, "stop_external")}>Stop</button>
      {:else}
        <button class="primary" disabled={busy || !ready} onclick={() => onAction(t, "connect")}>Connect</button>
      {/if}
      <span class="spacer"></span>
      <label class="auto-toggle" title="Connect automatically when Tunnel Bar starts">
        <input type="checkbox" checked={t.settings.autoConnect} disabled={busy} onchange={(event) => onSave(t, { autoConnect: event.currentTarget.checked })} />Auto
      </label>
    {/if}
  </div>
  {/if}

  {#if details}
    <div class="tunnel-details" transition:fade={{ duration: motionDuration(reduced, 140) }}>
      <div class="detail-id"><span class="hint">Tunnel ID</span><code>{t.id}</code></div>
      {#if local && t.ports.length}
        <div class="port-summary">Ports: {t.ports.map((p) => `${p.portNumber}${p.protocol && p.protocol !== "auto" ? "/" + p.protocol : ""}`).join(", ")}</div>
      {/if}
      {#if !local && t.forwards.filter((f) => isSsh(f, t)).length > 1}
        {#each t.forwards.filter((f) => isSsh(f, t)) as f (f.localPort)}
          <button class="quiet" disabled={busy || t.state.kind !== "connected"} onclick={() => onOpenSsh(t, f)}><Icon name="terminal" size={14} />Open SSH · localhost:{localPort(t, f)}</button>
        {/each}
      {/if}
      <div class="details-actions">
        <button class="quiet" disabled={busy} onclick={rename}>Rename</button>
        <button class="quiet" disabled={busy || !ready} onclick={() => { details = false; onDetails(t); }}>Manage…</button>
        <button class="quiet" onclick={() => { details = false; onLogs(t); }}><Icon name="terminal" size={14} />Logs</button>
        {#if !local && managedHost}
          <button class="quiet" disabled={busy} onclick={() => onAction(t, "stop_host")}>Stop hosting</button>
        {:else if !local && canHost(t)}
          <button class="quiet" disabled={busy || !ready} onclick={() => onAction(t, "host")}>Host here</button>
        {/if}
        {#if t.createdByApp}
          <button class="quiet danger-text" disabled={busy || !ready} onclick={() => showConfirmation("delete")}>Delete…</button>
        {/if}
      </div>
    </div>
  {/if}
</article>
