<script lang="ts">
  import { tick, untrack } from "svelte";
  import HeaderFields from "./HeaderFields.svelte";
  import AccessRuleFields from "./AccessRuleFields.svelte";
  import { expirationLabel } from "./types";
  import type { AccessEntry, AccessRuleRequest, TunnelView, UpdateTunnelRequest } from "./types";
  import type { Guard, Manage, ManagementCommand } from "./management";

  let { t, busy, onManage, onReadRules, onDeleted }: {
    t: TunnelView; busy: boolean; onManage: Manage;
    onReadRules: (id: string, port?: number) => Promise<AccessEntry[] | null>; onDeleted: () => void;
  } = $props();
  type Pending = { command: ManagementCommand; args: Record<string, unknown>; title: string; guard: Guard | { kind: "destructive"; message: string } };
  let pending = $state<Pending | null>(null);
  let confirmationBox = $state<HTMLDivElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let description = $state("");
  let labels = $state("");
  let labelError = $state("");
  let expiration = $state("");
  let initialized = "";
  let hostHeader = $state("");
  let originHeader = $state("");
  let addPort = $state<number | undefined>();
  let addTunnelPort = $state<number | undefined>();
  let protocol = $state("http");
  let addHost = $state("");
  let addOrigin = $state("");
  let ruleScope = $state("");
  let rule = $state<AccessRuleRequest>({ kind: "tenant" });
  let rules = $state<AccessEntry[] | null>(null);
  let loadingRules = $state(false);
  let rulesRevision = 0;
  const blocked = $derived(busy || !!pending);
  const tunnelId = $derived(t.id);

  $effect(() => {
    if (initialized === t.id) return;
    initialized = t.id;
    description = t.summary.description;
    labels = t.summary.labels.filter((label) => label.toLowerCase() !== "tunnelbar").join(", ");
  });
  $effect(() => {
    const id = tunnelId, port = ruleScope ? Number(ruleScope) : undefined;
    const revision = ++rulesRevision;
    rules = null; loadingRules = true;
    untrack(() => onReadRules(id, port)).then((value) => {
      if (revision === rulesRevision) { rules = value; loadingRules = false; }
    });
    return () => { rulesRevision++; };
  });
  async function focusConfirmation() {
    await tick();
    confirmationBox?.scrollIntoView({ block: "nearest" });
    cancelButton?.focus({ preventScroll: true });
  }
  async function run(command: ManagementCommand, args: Record<string, unknown>, title: string) {
    const result = await onManage(command, args, title);
    if (!result.ok) {
      if (result.guard) { pending = { command, args, title, guard: result.guard }; await focusConfirmation(); }
      return false;
    }
    pending = null;
    if (command === "delete_tunnel") onDeleted();
    if (command === "add_access_rule" || command === "remove_access_rule") {
      const revision = ++rulesRevision;
      loadingRules = true;
      const updated = await onReadRules(t.id, ruleScope ? Number(ruleScope) : undefined);
      if (revision === rulesRevision) { rules = updated; loadingRules = false; }
    }
    return true;
  }
  async function confirm() {
    if (!pending) return;
    const { command, args, title, guard } = pending;
    const flags = guard.kind === "confirm" ? { confirm: true } : guard.kind === "hostedElsewhere" ? { allowHostedElsewhere: true } : {};
    await run(command, { ...args, ...flags }, title);
  }
  function request(command: ManagementCommand, args: Record<string, unknown>, title: string, message: string) {
    pending = { command, args, title, guard: { kind: "destructive", message } };
    focusConfirmation();
  }
  async function saveAbout(event: SubmitEvent) {
    event.preventDefault();
    const desired = [...new Set(labels.split(",").map((label) => label.trim()).filter(Boolean))];
    labelError = desired.some((label) => label.toLowerCase() === "tunnelbar") ? "The tunnelbar ownership label is reserved; it cannot be edited here." : desired.some((label) => !/^[\w=-]{1,50}$/.test(label)) ? "Labels must be 1–50 letters, digits, underscores, hyphens or equals signs." : "";
    if (labelError) return;
    const current = t.summary.labels.filter((label) => label.toLowerCase() !== "tunnelbar");
    const update: UpdateTunnelRequest = {
      description, addLabels: desired.filter((label) => !current.includes(label)),
      removeLabels: current.filter((label) => !desired.includes(label)),
      ...(expiration ? { expiration } : {}),
    };
    if (await run("update_tunnel", { id: t.id, update }, "Save tunnel details")) expiration = "";
  }
  function submitRule(event: SubmitEvent) {
    event.preventDefault();
    const request: AccessRuleRequest = { kind: rule.kind, deny: !!rule.deny,
      ...(rule.kind === "org" || rule.kind === "repo" ? { value: rule.value?.trim() } : {}),
      ...(rule.expiration ? { expiration: rule.expiration } : {}), ...(ruleScope ? { port: Number(ruleScope) } : {}),
    };
    run("add_access_rule", { id: t.id, rule: request }, "Add access rule");
  }
  const ruleNames: Record<string, string> = { Anonymous: "Anyone", Organizations: "Entra tenant", GitHubOrganization: "GitHub organization", GitHubRepository: "Repository" };
  const ruleLabel = (entry: AccessEntry) => ruleNames[entry.type] ?? entry.type;
</script>

<section class="settings-content management-content" aria-label="Tunnel management">
  <div class="detail-id"><span class="hint">Tunnel ID</span><code>{t.id}</code></div>
  {#if pending}
    <div class="confirmation guard-confirmation" bind:this={confirmationBox} role="alert">
      <strong>{pending.guard.kind === "hostedElsewhere" ? "Another machine is hosting this tunnel" : pending.guard.kind === "confirm" ? "Tunnel Bar didn't create this tunnel" : pending.title + "?"}</strong>
      <p>{pending.guard.message}</p>
      {#if pending.guard.kind === "hostedElsewhere"}<p>This can interrupt the other device's host or SSH connection. Approve this remote-host override separately.</p>{/if}
      <div class="button-row"><button bind:this={cancelButton} disabled={busy} onclick={() => (pending = null)}>Cancel</button>
        <button class="danger" disabled={busy} onclick={confirm}>{busy ? "Working…" : pending.guard.kind === "hostedElsewhere" ? "Override remote host" : pending.guard.kind === "confirm" ? "Confirm change" : "Confirm"}</button></div>
    </div>
  {/if}
  <details class="management-section" open>
    <summary>About this tunnel <span>{expirationLabel(t.expirationHours) ?? "Lifetime unknown"}</span></summary>
    <form data-form="about" onsubmit={saveAbout}>
      <label class="field"><span>Description</span><textarea bind:value={description} rows="2" disabled={blocked} placeholder="What this tunnel is for"></textarea></label>
      <label class="field"><span>Labels</span><input bind:value={labels} aria-invalid={!!labelError} disabled={blocked} placeholder="development, preview" oninput={() => (labelError = "")} /></label>
      {#if labelError}<p class="field-hint error-text" role="alert">{labelError}</p>{/if}
      <p class="field-hint">Comma-separated labels, 1–50 letters, digits, underscores, hyphens or equals signs. The reserved tunnelbar ownership label cannot be edited.</p>
      <label class="field"><span>Tunnel expiration</span><select bind:value={expiration} disabled={blocked}><option value="">Keep current lifetime</option><option value="1h">1 hour</option><option value="8h">8 hours</option><option value="1d">1 day</option><option value="7d">7 days</option><option value="30d">30 days</option></select></label>
      <button class="primary" disabled={blocked}>Save details</button>
    </form>
  </details>
  <details class="management-section" data-section="ports">
    <summary>Ports and stable addresses <span>{t.ports.length}</span></summary>
    {#each t.ports as port (port.portNumber)}
      <details class="port-editor" data-port={port.portNumber}>
        <summary><code>:{port.portNumber}</code> · {port.protocol || "auto"}{#if t.settings.pinnedPorts?.[String(port.portNumber)]}<span>Pinned :{t.settings.pinnedPorts[String(port.portNumber)]}</span>{/if}</summary>
        <form data-form="host-map" onsubmit={(event) => { event.preventDefault(); const data = new FormData(event.currentTarget); const value = String(data.get("localServerPort") ?? ""); run("set_port_map", { id: t.id, remotePort: port.portNumber, localPort: value ? Number(value) : null }, "Save host mapping"); }}>
          <label class="field"><span>Local server port when hosting</span><input name="localServerPort" type="number" min="1" max="65535" step="1" value={port.localPort ?? t.settings.hostPortMap?.[String(port.portNumber)] ?? ""} disabled={blocked} placeholder="Same as tunnel port" /></label>
          <p class="field-hint">Tunnel :{port.portNumber} → localhost:{port.localPort ?? port.portNumber}. Only affects hosting on this device; leave empty to clear the map.</p>
          {#each t.portMaps.filter((map) => map.remotePort === port.portNumber) as map}
            <p class="field-hint" class:error-text={!!map.error}>{map.error || (map.listening ? "Host mapping proxy ready." : "The mapping starts when this device hosts.")}</p>
          {/each}
          <button disabled={blocked}>Save host mapping</button>
        </form>
        <form data-form="pin" onsubmit={(event) => { event.preventDefault(); const data = new FormData(event.currentTarget); const value = String(data.get("localPort") ?? ""); run("set_pinned_port", { id: t.id, remotePort: port.portNumber, localPort: value ? Number(value) : null }, "Save stable port"); }}>
          <label class="field"><span>Stable localhost port</span><input name="localPort" type="number" min="1" max="65535" step="1" value={t.settings.pinnedPorts?.[String(port.portNumber)] ?? ""} disabled={blocked} placeholder="Unpinned" /></label>
          <p class="field-hint">A loopback proxy keeps this address stable across reconnects. Leave empty to unpin. Copy / Open SSH use the pin.</p>
          {#each t.pins.filter((pin) => pin.remotePort === port.portNumber) as pin}
            <p class="field-hint" class:error-text={!!pin.error}>{pin.error || (pin.listening ? `localhost:${pin.localPort} → ${pin.targetPort ? "localhost:" + pin.targetPort : "waiting for connection"}` : "Waiting for the connection. The proxy retries automatically.")}</p>
          {/each}
          <button disabled={blocked}>Save stable port</button>
        </form>
        <form data-form="port-headers" onsubmit={(event) => { event.preventDefault(); const data = new FormData(event.currentTarget); run("set_port_headers", { id: t.id, port: port.portNumber, headers: { hostHeader: String(data.get("hostHeader") ?? "").trim(), originHeader: String(data.get("originHeader") ?? "").trim() } }, "Save port headers"); }}>
          <HeaderFields hostHeader={port.hostHeader ?? ""} originHeader={port.originHeader ?? ""} disabled={blocked} />
          <p class="field-hint">Current effective values are shown. Saving restarts this device's host.</p>
          <button disabled={blocked}>Save port headers</button>
        </form>
        <button class="quiet danger-text" disabled={blocked} onclick={() => request("remove_port", { id: t.id, port: port.portNumber }, "Remove port", `Remove port ${port.portNumber}? Its URL stops working and its stable-port pin is removed. This device's host restarts.`)}>Remove port…</button>
      </details>
    {/each}
    <form data-form="add-port" onsubmit={async (event) => { event.preventDefault(); if (await run("add_port", { id: t.id, port: addPort, remotePort: addTunnelPort ?? null, protocol, hostHeader: addHost.trim(), originHeader: addOrigin.trim() }, "Add port")) { addPort = undefined; addTunnelPort = undefined; } }}>
      <div class="two-fields">
        <label class="field"><span>Local server port</span><input type="number" min="1" max="65535" step="1" required bind:value={addPort} disabled={blocked} placeholder="3000" /></label>
        <label class="field"><span>Tunnel port (optional)</span><input type="number" min="1" max="65535" step="1" bind:value={addTunnelPort} disabled={blocked} placeholder="Same as local" /></label>
      </div>
      <label class="field"><span>Protocol</span><select bind:value={protocol} disabled={blocked}><option value="http">HTTP</option><option value="https">HTTPS</option><option value="auto">Auto-detect</option></select></label>
      <details class="advanced-options"><summary>Headers for this port</summary><HeaderFields bind:hostHeader={addHost} bind:originHeader={addOrigin} disabled={blocked} /></details>
      <p class="field-hint">Adding or removing ports restarts this device's host.</p><button class="primary" disabled={blocked}>Add port</button>
    </form>
  </details>
  <details class="management-section" data-section="access">
    <summary>Access rules <span>Tunnel or port</span></summary>
    <label class="field"><span>Rule scope</span><select bind:value={ruleScope} disabled={blocked || loadingRules}><option value="">Entire tunnel</option>{#each t.ports as port}<option value={String(port.portNumber)}>Port {port.portNumber}</option>{/each}</select></label>
    {#if loadingRules}<p class="field-hint" role="status">Loading access rules…</p>
    {:else if rules === null}<p class="field-hint error-text">Rules couldn't be loaded.</p><button disabled={blocked} onclick={async () => { loadingRules = true; rules = await onReadRules(t.id, ruleScope ? Number(ruleScope) : undefined); loadingRules = false; }}>Retry loading rules</button>
    {:else}
      {#each rules as entry}
        <div class="access-entry">
          <div><strong>{entry.isDeny ? "Deny" : "Allow"} · {ruleLabel(entry)}</strong><small class="selectable">{entry.subjects.join(", ") || entry.provider || "All visitors"}</small><small>{entry.expiration ? "Expires " + entry.expiration : "No expiration"}{entry.isInherited ? " · Inherited from tunnel" : ""}</small></div>
          {#if !entry.isInherited && entry.index != null}
            <button class="quiet danger-text" disabled={blocked} onclick={() => request("remove_access_rule", { id: t.id, ...(ruleScope ? { port: Number(ruleScope) } : {}), index: entry.index }, "Remove access rule", "Removing an allow rule blocks those visitors; removing a deny rule can let them connect.")}>Remove…</button>
          {/if}
        </div>
      {:else}<p class="field-hint">No rules at this scope. Account-owner access still applies.</p>{/each}
    {/if}
    <form data-form="add-rule" onsubmit={submitRule}>
      <AccessRuleFields bind:rule disabled={blocked || loadingRules || rules === null} />
      <p class="field-hint">The service may shorten tenant-rule expiration. Stored expiration is shown above.</p>
      <button class="primary" disabled={blocked || loadingRules || rules === null}>Add rule</button>
    </form>
  </details>
  <details class="management-section" data-section="headers">
    <summary>Tunnel header defaults</summary>
    <form data-form="tunnel-headers" onsubmit={(event) => { event.preventDefault(); run("update_tunnel", { id: t.id, update: { hostHeader: hostHeader.trim(), originHeader: originHeader.trim() } }, "Save tunnel headers"); }}>
      <HeaderFields bind:hostHeader bind:originHeader disabled={blocked} />
      <p class="field-hint">Tunnel-level values are not reported by the service. Effective values are shown per port. Changes restart this device's host.</p>
      <button class="primary" disabled={blocked || (!hostHeader.trim() && !originHeader.trim())}>Save tunnel headers</button>
    </form>
  </details>
  <div class="danger-zone"><strong>Delete tunnel</strong><p class="field-hint">Permanently remove this tunnel and its ports. Other devices and non-app tunnels require additional confirmation.</p>
    <button class="danger" disabled={blocked} onclick={() => request("delete_tunnel", { id: t.id }, "Delete tunnel", "This removes the tunnel from your account. Its URL stops working, and local hosting and connections stop.")}>Delete tunnel…</button>
  </div>
</section>
