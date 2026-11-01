<script lang="ts">
  import { onMount, tick } from "svelte";
  import { fade } from "svelte/transition";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getVersion } from "@tauri-apps/api/app";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import Icon from "$lib/Icon.svelte";
  import TunnelCard from "$lib/TunnelCard.svelte";
  import Notification from "$lib/Notification.svelte";
  import TunnelManagement from "$lib/TunnelManagement.svelte";
  import HeaderFields from "$lib/HeaderFields.svelte";
  import AccessRuleFields from "$lib/AccessRuleFields.svelte";
  import LoginPanel from "$lib/LoginPanel.svelte";
  import type { Notice } from "$lib/Notification.svelte";
  import type { Snapshot, TunnelView, LogLine, Forward, HostedPort, TunnelSettings, AccessEntry, AccessRuleRequest, LoginProvider } from "$lib/types";
  import { DEFAULT_HOTKEY, errorKind, stripErrorPrefix } from "$lib/types";
  import { localPort } from "$lib/management";
  import type { GuardFlags, ManagementResult, ManagementCommand } from "$lib/management";
  import { remoteTunnels, localTunnels } from "$lib/tunnels";
  import type { TunnelAction } from "$lib/tunnels";
  import { motionDuration } from "$lib/motion";

  type View = "list" | "logs" | "settings" | "expose" | "details";
  type Result<T> = { ok: true; value: T } | { ok: false; error?: unknown };
  let snap = $state<Snapshot | null>(null);
  let view = $state<View>("list");
  let busy = $state<string[]>([]);
  let notice = $state<Notice | null>(null);
  let copied = $state<string | null>(null);
  let reduced = $state(false);
  let windows = $state(false);
  let exposePort = $state("");
  let exposeTunnelPort = $state("");
  let exposeProtocol = $state("http");
  let exposeAnon = $state(false);
  let exposeName = $state("");
  let exposeHost = $state("");
  let exposeOrigin = $state("");
  let exposeRules = $state<AccessRuleRequest[]>([]);
  let detailId = $state<string | null>(null);
  let hotkeyDraft = $state("");
  let portError = $state("");
  let logId = $state<string | null>(null);
  let logs = $state<LogLine[]>([]);
  let follow = $state(true);
  let cliDraft = $state("");
  let logBox = $state<HTMLDivElement>();
  let heading = $state<HTMLHeadingElement>();
  let exposeInput = $state<HTMLInputElement>();
  let exposeTunnelInput = $state<HTMLInputElement>();
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  let now = $state(Date.now());
  let appVersion = $state("");
  let nextNotice = 0;
  let mounted = false;
  let snapshotRevision = 0;

  const visibleTunnels = $derived((snap?.tunnels ?? []).filter((t) => t.matchesFilter !== false));
  const remote = $derived(remoteTunnels(visibleTunnels));
  const local = $derived(localTunnels(visibleTunnels));
  const detailTunnel = $derived(snap?.tunnels.find((t) => t.id === detailId));
  const ready = $derived(!!snap?.cliPath && !!snap?.auth?.loggedIn);
  const logTunnel = $derived(snap?.tunnels.find((t) => t.id === logId));
  const account = $derived(snap?.auth?.loggedIn
    ? snap.auth.message.replace(/^Logged in as /, "").replace(/ using .*$/, "").replace(/\.$/, "")
    : "Not signed in");

  function success(title: string, detail = "", copyKey?: string) {
    if (copyKey) {
      copied = copyKey;
      clearTimeout(copiedTimer);
      copiedTimer = setTimeout(() => (copied = null), 1600);
    }
    if (notice?.kind !== "error") notice = { id: ++nextNotice, kind: "success", title, detail };
  }
  function failure(title: string, error: unknown, retry?: () => Promise<void>) {
    notice = { id: ++nextNotice, kind: "error", title, detail: String(error), retry };
  }
  async function perform<T>(operation: () => Promise<T>, title: string, key: string, retry?: () => Promise<void>, guarded = false): Promise<Result<T>> {
    if (busy.includes(key)) return { ok: false };
    busy = [...busy, key];
    try {
      return { ok: true, value: await operation() };
    } catch (error) {
      if (!guarded || !errorKind(error)) failure(title, stripErrorPrefix(error), retry);
      return { ok: false, error };
    } finally {
      busy = busy.filter((item) => item !== key);
    }
  }
  function command<T = void>(cmd: string, args: Record<string, unknown> | undefined, title: string, key: string, retry?: () => Promise<void>) {
    return perform(() => invoke<T>(cmd, args), title, key, retry);
  }
  async function loadSnapshot() {
    const revision = snapshotRevision;
    const result = await command<Snapshot>("get_snapshot", undefined, "Couldn’t load tunnels", "snapshot", async () => { notice = null; await loadSnapshot(); });
    if (result.ok && mounted && revision === snapshotRevision) snap = result.value;
  }
  async function refresh() {
    await command("refresh", undefined, "Couldn’t refresh tunnels", "refresh", async () => { notice = null; await refresh(); });
  }
  async function navigate(next: View) {
    view = next;
    await tick();
    if (next === "expose") exposeInput?.focus();
    else heading?.focus();
  }
  function back() { navigate("list"); }
  function openSettings() { cliDraft = snap?.settings.cliPath ?? ""; hotkeyDraft = snap?.settings.hotkey ?? ""; navigate("settings"); }
  function openDetails(t: TunnelView) { detailId = t.id; navigate("details"); }
  function openExpose() {
    exposePort = ""; exposeProtocol = "http"; exposeAnon = false; exposeName = ""; portError = "";
    exposeTunnelPort = "";
    exposeHost = ""; exposeOrigin = ""; exposeRules = [];
    navigate("expose");
  }

  onMount(() => {
    mounted = true;
    windows = /Win/.test(navigator.platform);
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    reduced = media.matches;
    const motion = () => { reduced = media.matches; };
    media.addEventListener("change", motion);
    getVersion().then((version) => { if (mounted) appVersion = version; }).catch((error) => { if (mounted) failure("Couldn’t read app version", error); });
    const unlisteners: UnlistenFn[] = [];
    function register<T>(event: string, handler: (payload: T) => void) {
      return listen<T>(event, (e) => { if (mounted) handler(e.payload); })
        .then((unlisten) => { if (mounted) unlisteners.push(unlisten); else unlisten(); })
        .catch((error) => { if (mounted) failure("Couldn’t subscribe to " + event, error); });
    }
    const snapshotListener = register<Snapshot>("snapshot", (snapshot) => { snapshotRevision++; snap = snapshot; });
    const logListener = register<{ id: string; line: LogLine }>("log", async (e) => {
      if (view !== "logs" || e.id !== logId) return;
      logs.push(e.line);
      if (logs.length > 2000) logs.splice(0, logs.length - 2000);
      if (follow) { await tick(); logBox?.scrollTo(0, logBox.scrollHeight); }
    });
    Promise.all([snapshotListener, logListener]).then(() => { if (mounted) loadSnapshot(); });
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (notice) { notice = null; event.preventDefault(); }
      else if (view !== "list" && !busy.includes("expose")) back();
    };
    window.addEventListener("keydown", onKey);
    const clock = setInterval(() => (now = Date.now()), 5000);
    return () => {
      mounted = false;
      unlisteners.forEach((unlisten) => unlisten());
      media.removeEventListener("change", motion);
      window.removeEventListener("keydown", onKey);
      clearInterval(clock);
      clearTimeout(copiedTimer);
    };
  });

  async function manage(cmd: ManagementCommand, args: Record<string, unknown>, title: string): Promise<ManagementResult> {
    const result = await perform(() => invoke(cmd, args), "Couldn’t " + title.toLowerCase(), String(args.id), undefined, true);
    if (!result.ok) {
      const kind = errorKind(result.error);
      return kind ? { ok: false, guard: { kind, message: stripErrorPrefix(result.error) } } : { ok: false };
    }
    success(cmd === "delete_tunnel" ? "Tunnel deleted" : "Change saved", title);
    return { ok: true };
  }
  async function readRules(id: string, port?: number): Promise<AccessEntry[] | null> {
    const result = await command<AccessEntry[]>("list_access_rules", { id, ...(port != null ? { port } : {}) }, "Couldn’t load access rules", "rules:" + id);
    return result.ok ? result.value : null;
  }
  async function action(t: TunnelView, cmd: TunnelAction, allow?: boolean, flags?: GuardFlags): Promise<ManagementResult> {
    const retryable = cmd === "connect" || cmd === "host";
    const titles: Record<TunnelAction, string> = {
      connect: "Couldn’t connect", disconnect: "Couldn’t disconnect", host: "Couldn’t start hosting",
      stop_host: "Couldn’t stop hosting", adopt_external: "Couldn’t take over",
      stop_external: "Couldn’t stop external devtunnel", delete_tunnel: "Couldn’t delete tunnel",
      set_anonymous: "Couldn’t change access",
    };
    const result = await perform(() => invoke(cmd, cmd === "set_anonymous" ? { id: t.id, allow, ...flags } : { id: t.id, ...flags }), titles[cmd], t.id,
      retryable ? async () => { notice = null; await action(t, cmd, allow); } : undefined, cmd === "delete_tunnel" || cmd === "set_anonymous");
    if (!result.ok) {
      const kind = errorKind(result.error);
      return kind ? { ok: false, guard: { kind, message: stripErrorPrefix(result.error) } } : { ok: false };
    }
    if (cmd === "delete_tunnel") success("Tunnel deleted", t.displayName);
    else if (cmd === "set_anonymous") success(allow ? "Public access enabled" : "Sign-in required", t.displayName);
    return { ok: true };
  }
  async function saveTunnel(t: TunnelView, patch: Partial<TunnelSettings>): Promise<boolean> {
    const current = snap?.tunnels.find((item) => item.id === t.id) ?? t;
    const settings = { ...current.settings, ...patch };
    const result = await command("update_tunnel_settings", { id: t.id, settings }, "Couldn’t save tunnel settings", t.id);
    if (!result.ok) return false;
    if (snap) snap = { ...snap, tunnels: snap.tunnels.map((item) => item.id === t.id ? { ...item, settings } : item) };
    success(patch.nickname !== undefined ? "Nickname saved" : "Automation updated", t.displayName);
    return true;
  }
  async function saveApp(patch: { launchAtLogin?: boolean; notifications?: boolean; cliPath?: string | null }) {
    if (!snap) return;
    const s = snap.settings;
    const settings = {
      launchAtLogin: patch.launchAtLogin ?? s.launchAtLogin,
      notifications: patch.notifications ?? s.notifications,
      cliPath: patch.cliPath !== undefined ? patch.cliPath : (s.cliPath ?? null),
    };
    const result = await command("update_app_settings", settings, "Couldn’t save settings", "settings");
    if (result.ok) {
      if (snap) snap = { ...snap, settings: { ...snap.settings, ...settings } };
      success("Settings saved", patch.cliPath !== undefined ? "devtunnel CLI path updated" : "Startup and notification preferences");
    }
  }
  async function clipboard(text: string, title: string, detail: string, key: string) {
    const result = await perform(() => writeText(text), "Couldn’t copy " + title.replace(/ copied$/, ""), "clipboard",
      async () => { notice = null; await clipboard(text, title, detail, key); });
    if (result.ok) success(title, detail, key);
  }
  async function copySsh(t: TunnelView, f: Forward) {
    const result = await command<string>("ssh_command", { id: t.id, localPort: f.localPort, remotePort: f.remotePort }, "Couldn’t create SSH command", t.id);
    if (result.ok) await clipboard(result.value, "SSH command copied", t.displayName + " · localhost:" + localPort(t, f), "ssh:" + t.id + ":" + f.localPort);
  }
  async function copyAddress(t: TunnelView, f: Forward) { const port = localPort(t, f); await clipboard(`localhost:${port}`, "Address copied", `localhost:${port}`, "address:" + t.id + ":" + f.localPort); }
  async function copyUrl(t: TunnelView, p: HostedPort) { await clipboard(p.uri, "URL copied", t.displayName + " · full URL", "url:" + t.id + ":" + p.port); }
  async function openSsh(t: TunnelView, f: Forward) {
    await command("open_ssh", { id: t.id, localPort: f.localPort, remotePort: f.remotePort }, "Couldn’t open SSH", t.id);
  }
  async function openUrl(url: string) { await command("open_url", { url }, "Couldn’t open URL", "url"); }
  async function openLogin() { await command("open_login", undefined, "Couldn’t open sign-in", "login"); }
  async function startLogin(provider: LoginProvider, deviceCode: boolean) { await command("start_login", { provider, deviceCode }, "Couldn’t start sign-in", "login"); }
  async function cancelLogin() { await command("cancel_login", undefined, "Couldn’t cancel sign-in", "login"); }
  async function logout() { const result = await command("logout", undefined, "Couldn’t sign out", "login"); return result.ok; }
  async function saveHotkey() { const result = await command("set_hotkey", { hotkey: hotkeyDraft.trim() || null }, "Couldn’t save shortcut", "settings"); if (result.ok) success("Shortcut saved", hotkeyDraft.trim() || "Global shortcut disabled"); }
  async function setLabelFilter(labels: string[]) { const result = await command("set_label_filter", { labels }, "Couldn’t update label filter", "settings"); if (result.ok) success("Label filter updated", labels.length ? labels.join(", ") : "Showing all tunnels"); }
  async function toggleLabel(label: string) {
    const current = snap?.settings.labelFilter ?? [];
    await setLabelFilter(current.some((item) => item.toLowerCase() === label.toLowerCase()) ? current.filter((item) => item.toLowerCase() !== label.toLowerCase()) : [...current, label]);
  }
  async function openLogFolder() { await command("open_log_dir", undefined, "Couldn’t open logs folder", "folder"); }
  let updateLater = $state<string | null>(null);
  const update = $derived(snap?.update);
  const updateReady = $derived(update?.status === "ready" || update?.status === "installing");
  const activeCount = $derived(snap?.tunnels.filter((t) => t.state.kind !== "disconnected" || t.hostState.kind !== "disconnected").length ?? 0);
  const restartWarning = $derived(activeCount
    ? `Restarting stops ${activeCount === 1 ? "1 active connection or host" : activeCount + " active connections and hosts"}, so open SSH sessions will drop. They come back automatically after the update.`
    : "Tunnel Bar restarts in a few seconds.");
  function updateStatusText(u: NonNullable<typeof update>) {
    switch (u.status) {
      case "checking": return "Checking for updates…";
      case "downloading": return `Downloading ${u.version ?? "update"}… ${Math.round((u.progress ?? 0) * 100)}%`;
      case "ready": return `Version ${u.version} is ready to install`;
      case "installing": return `Installing ${u.version}…`;
      case "upToDate": return "Up to date" + (u.lastChecked ? " · checked " + ago(u.lastChecked) : "");
      case "error": return u.error ?? "Couldn’t check for updates";
      default: return u.enabled ? "Checks automatically every few hours" : "Updates are off in development builds";
    }
  }
  async function checkForUpdates() { await command("check_for_updates", undefined, "Couldn’t check for updates", "update"); }
  async function installUpdate() { await command("install_update", undefined, "Couldn’t install the update", "update"); }
  async function quit() { await command("quit", undefined, "Couldn’t quit Tunnel Bar", "quit"); }

  async function openLogs(t: TunnelView) {
    const result = await command<LogLine[]>("get_logs", { id: t.id }, "Couldn’t load logs", "logs");
    if (!result.ok) return;
    logId = t.id; logs = result.value; follow = true;
    await navigate("logs");
    logBox?.scrollTo(0, logBox.scrollHeight);
  }
  async function clearLogs() {
    if (!logId) return;
    const result = await command("clear_logs", { id: logId }, "Couldn’t clear logs", "logs");
    if (result.ok) { logs = []; success("Logs cleared", logTunnel?.displayName ?? logId); }
  }
  async function submitExpose(event: SubmitEvent) {
    event.preventDefault();
    const port = Number(exposePort);
    if (!/^\d+$/.test(exposePort.trim()) || !Number.isInteger(port) || port < 1 || port > 65535) {
      portError = "Enter a port between 1 and 65535."; exposeInput?.focus(); return;
    }
    const remotePort = exposeTunnelPort.trim() ? Number(exposeTunnelPort) : null;
    if (remotePort !== null && (!/^\d+$/.test(exposeTunnelPort.trim()) || !Number.isInteger(remotePort) || remotePort < 1 || remotePort > 65535)) {
      portError = "Enter a tunnel port between 1 and 65535."; exposeTunnelInput?.focus(); return;
    }
    portError = "";
    const accessRules = exposeRules.map((rule): AccessRuleRequest => ({ kind: rule.kind, deny: !!rule.deny,
      ...(rule.kind === "org" || rule.kind === "repo" ? { value: rule.value?.trim() } : {}), ...(rule.expiration ? { expiration: rule.expiration } : {}),
    }));
    const result = await command<string>("expose_port", { port, remotePort, protocol: exposeProtocol, anonymous: exposeAnon, name: exposeName.trim() || null,
      hostHeader: exposeHost.trim() || null, originHeader: exposeOrigin.trim() || null, accessRules }, "Couldn’t expose port", "expose");
    if (result.ok) { await navigate("list"); success("Tunnel created", `Starting host for localhost:${port}`); }
  }
  const ago = (iso?: string | null) => {
    if (!iso) return "never";
    const seconds = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
    return seconds < 60 ? `${seconds}s ago` : `${Math.round(seconds / 60)}m ago`;
  };
</script>

<!-- Approved C: purpose groups, explicit access, one feedback dock; app/tray icons are unchanged. -->
<main class:windows>
  <header class="app-header">
    {#if view !== "list"}
      <button class="icon-button back-button" title="Back to tunnels" aria-label="Back to tunnels" disabled={busy.includes("expose")} onclick={back}><Icon name="back" /></button>
    {/if}
    <div class="header-title">
      <h1 tabindex="-1" bind:this={heading}>{view === "list" ? "Tunnel Bar" : view === "expose" ? "Expose a local port" : view === "logs" ? "Logs" : view === "details" ? "Tunnel details" : "Settings"}</h1>
      <span class="header-subtitle" title={view === "logs" ? logTunnel?.displayName ?? logId ?? "" : snap?.auth?.message ?? ""}>
        {view === "list" ? account : view === "logs" ? logTunnel?.displayName ?? logId : view === "details" ? detailTunnel?.displayName ?? detailId : view === "expose" ? "New local tunnel" : "Tunnel Bar"}
      </span>
    </div>
    {#if view === "list"}
      <div class="header-tools">
        <button class="primary expose-button" disabled={!ready} title="Expose a local port" onclick={openExpose}><Icon name="plus" size={14} />Expose</button>
        <button class="icon-button" title="Refresh tunnels" aria-label="Refresh tunnels" disabled={snap?.refreshing || busy.includes("refresh")} onclick={refresh}>
          <span class:refreshing={snap?.refreshing}><Icon name="refresh" /></span>
        </button>
        <button class="icon-button" title="Settings" aria-label="Settings" onclick={openSettings}><Icon name="settings" /></button>
      </div>
    {:else if view === "logs"}
      <label class="auto-toggle"><input type="checkbox" bind:checked={follow} onchange={async () => { if (follow) { await tick(); logBox?.scrollTo(0, logBox.scrollHeight); } }} />Follow</label>
    {/if}
  </header>

  {#if view === "list"}
    <section class="list-content" aria-label="Tunnels">
      {#if !snap}
        <div class="loading-state" aria-label="Loading tunnels" aria-busy="true"><div></div><div></div><div></div><span class="hint">Loading tunnels…</span></div>
      {:else if !snap.cliPath}
        <div class="empty-state"><Icon name="terminal" size={32} /><h2>devtunnel CLI not found</h2><p>Install the devtunnel CLI or set its path in Settings.</p><code class="selectable">{windows ? "winget install Microsoft.devtunnel" : "brew install --cask devtunnel"}</code><button onclick={openSettings}>Open Settings</button></div>
      {:else if !snap.auth?.loggedIn}
        <div class="empty-state"><Icon name="lock" size={32} /><h2>Sign in to Dev Tunnels</h2><p>Use your Microsoft or GitHub account to view and manage your tunnels.</p>
          <LoginPanel login={snap.login} loggedIn={false} {account} busy={busy.includes("login")} onStart={startLogin} onCancel={cancelLogin} onLogout={logout} onFallback={openLogin} onCopy={(code) => clipboard(code, "Code copied", "Sign-in device code", "login-code")} onOpen={openUrl} />
        </div>
      {/if}
      {#if updateReady && update && updateLater !== update.version}
        <div class="banner update-banner" role="status"><Icon name="download" /><div><strong>Tunnel Bar {update.version} is ready</strong><p>{restartWarning}</p>{#if update.error}<p class="error-text selectable">{update.error}</p>{/if}
          <div class="button-row"><button class="primary" disabled={update.status === "installing" || busy.includes("update")} onclick={installUpdate}>{update.status === "installing" ? "Restarting…" : "Restart to update"}</button><button class="quiet" disabled={update.status === "installing"} onclick={() => (updateLater = update.version ?? null)}>Later</button></div></div></div>
      {/if}
      {#if snap?.adminDevtunnel}<div class="banner warning"><Icon name="warning" /><div><strong>devtunnel is running as administrator</strong><p>Tunnel Bar can’t tell which tunnel it serves. Stop it in that terminal to manage it here.</p></div></div>{/if}
      {#if snap?.refreshError}<div class="banner warning"><Icon name="warning" /><div><strong>Couldn’t refresh tunnels</strong><p class="selectable">{snap.refreshError}</p><button class="quiet" disabled={snap.refreshing} onclick={refresh}>Retry refresh</button></div></div>{/if}
      {#if remote.length}
        <div class="section-heading"><h2>Remote connections</h2><span>{remote.length}</span></div>
        {#each remote as t (t.id)}
          <TunnelCard {t} role="remote" {ready} busy={busy.includes(t.id)} {reduced} {copied}
            onAction={action} onLogs={openLogs} onSave={saveTunnel} onCopySsh={copySsh} onCopyAddress={copyAddress}
            onCopyUrl={copyUrl} onOpenSsh={openSsh} onOpenUrl={openUrl} onDetails={openDetails} />
        {/each}
      {/if}
      {#if local.length}
        <div class="section-heading" class:subsequent={remote.length > 0}><h2>Local tunnels</h2><span>{local.length}</span></div>
        {#each local as t (t.id)}
          <TunnelCard {t} role="local" {ready} busy={busy.includes(t.id)} {reduced} {copied}
            onAction={action} onLogs={openLogs} onSave={saveTunnel} onCopySsh={copySsh} onCopyAddress={copyAddress}
            onCopyUrl={copyUrl} onOpenSsh={openSsh} onOpenUrl={openUrl} onDetails={openDetails} />
        {/each}
      {/if}
      {#if snap && ready && !snap.tunnels.length}
        <div class="empty-state"><Icon name="laptop" size={32} /><h2>No tunnels yet</h2><p>Share a local port through a new dev tunnel. Your account’s tunnels will appear here.</p><button class="primary" onclick={openExpose}><Icon name="plus" />Expose a local port</button><span class="hint">Existing tunnel not showing? Refresh the list.</span></div>
      {/if}
      {#if ready && snap?.tunnels.length && !visibleTunnels.length}
        <div class="empty-state"><h2>No matching tunnels</h2><p>Your label filter hides these tunnels. Connections keep running.</p><button disabled={busy.includes("settings")} onclick={() => setLabelFilter([])}>Clear label filter</button></div>
      {/if}
    </section>
  {:else if view === "expose"}
    <form id="expose-form" class="settings-content" onsubmit={submitExpose}>
      <p class="view-description">Create a dev tunnel and host this port from your device.</p>
      <div class="two-fields">
        <label class="field"><span>Local port</span><input bind:this={exposeInput} bind:value={exposePort} inputmode="numeric" placeholder="3000" aria-invalid={!!portError} aria-describedby="port-hint" disabled={busy.includes("expose")} oninput={() => (portError = "")} /></label>
        <label class="field"><span>Tunnel port <span class="optional">(optional)</span></span><input bind:this={exposeTunnelInput} bind:value={exposeTunnelPort} inputmode="numeric" placeholder="Same as local" aria-invalid={!!portError} aria-describedby="port-hint" disabled={busy.includes("expose")} oninput={() => (portError = "")} /></label>
      </div>
      <p id="port-hint" class="field-hint" class:error-text={!!portError} role={portError ? "alert" : undefined}>{portError || (exposeTunnelPort ? `Tunnel :${exposeTunnelPort} → localhost:${exposePort || "PORT"}. Both ports must be between 1 and 65535.` : "A port between 1 and 65535. Leave Tunnel port empty to use the same number.")}</p>
      <label class="field"><span>Tunnel name <span class="optional">(optional)</span></span><input bind:value={exposeName} placeholder="Random if empty" autocapitalize="off" spellcheck="false" pattern={"[a-z0-9-]{3,49}"} maxlength="49" disabled={busy.includes("expose")} oninput={() => (exposeName = exposeName.toLowerCase())} aria-describedby="name-hint" /></label>
      <p id="name-hint" class="field-hint">3–49 lowercase letters, digits or hyphens. Becomes the tunnel ID.</p>
      <label class="field"><span>Protocol</span><select bind:value={exposeProtocol} disabled={busy.includes("expose")}><option value="http">HTTP</option><option value="https">HTTPS (local TLS)</option><option value="auto">Auto-detect</option></select></label>
      <label class="setting-toggle"><span><strong>Allow anonymous access</strong><small>Off by default</small></span><input class="switch" type="checkbox" bind:checked={exposeAnon} disabled={busy.includes("expose")} /></label>
      <div class="access-explanation" class:warning={exposeAnon}><Icon name={exposeAnon ? "warning" : "lock"} size={14} /><p>{exposeAnon ? "Anyone with the URL can reach localhost:" + (exposePort || "PORT") + " without signing in." : "Visitors must sign in with the account that owns the tunnel."}</p></div>
      <details class="advanced-options"><summary>Advanced</summary>
        <HeaderFields bind:hostHeader={exposeHost} bind:originHeader={exposeOrigin} disabled={busy.includes("expose")} />
        <div class="button-row"><strong class="field-title">Additional access rules</strong><span class="spacer"></span><button type="button" class="quiet" disabled={busy.includes("expose")} onclick={() => exposeRules.push({ kind: "tenant", deny: false })}>Add rule</button></div>
        {#each exposeRules as rule, index}
          <div class="draft-rule"><AccessRuleFields bind:rule={exposeRules[index]} anonymous={false} disabled={busy.includes("expose")} /><button type="button" class="quiet danger-text" disabled={busy.includes("expose")} onclick={() => exposeRules.splice(index, 1)}>Remove rule</button></div>
        {/each}
        <p class="field-hint">These rules apply to the entire tunnel, in addition to account-owner access and the anonymous toggle above.</p>
      </details>
    </form>
  {:else if view === "logs"}
    <div class="logs-content" bind:this={logBox} onscroll={() => { if (logBox) follow = logBox.scrollTop + logBox.clientHeight >= logBox.scrollHeight - 4; }}>
      {#each logs as line, index (index)}
        <div class="log-line" class:log-error={["stderr", "err", "host-err"].includes(line.stream)} class:log-app={line.stream === "app"}>
          <time>{line.ts.slice(11, 19)}</time><span>{line.text}</span>
        </div>
      {:else}<div class="empty-state"><h2>No output yet</h2><p>Connect or host this tunnel to see devtunnel logs here.</p></div>{/each}
    </div>
  {:else if view === "details"}
    {#if detailTunnel}
      {#key detailTunnel.id}<TunnelManagement t={detailTunnel} busy={busy.includes(detailTunnel.id) || !ready} onManage={manage} onReadRules={readRules} onDeleted={back} />{/key}
    {:else}<section class="settings-content"><div class="empty-state"><h2>Tunnel no longer available</h2><button onclick={back}>Back to tunnels</button></div></section>{/if}
  {:else}
    <section class="settings-content" aria-label="Settings">
      {#if snap}
        <div class="settings-group">
          <label class="setting-toggle"><span><strong>Launch at login</strong></span><input class="switch" type="checkbox" checked={snap.settings.launchAtLogin} disabled={busy.includes("settings")} onchange={(event) => saveApp({ launchAtLogin: event.currentTarget.checked })} /></label>
          <label class="setting-toggle"><span><strong>Connection notifications</strong><small>Notify on drops and reconnects</small></span><input class="switch" type="checkbox" checked={snap.settings.notifications} disabled={busy.includes("settings")} onchange={(event) => saveApp({ notifications: event.currentTarget.checked })} /></label>
        </div>
        <div class="settings-group">
          <label class="field"><span>Global shortcut</span><input bind:value={hotkeyDraft} placeholder={DEFAULT_HOTKEY} disabled={busy.includes("settings")} /></label>
          <p class="field-hint">Use an accelerator such as {DEFAULT_HOTKEY}. A modifier is required. Leave empty to disable.</p>
          {#if snap.hotkeyError}<p class="inline-warning error-text" role="alert">{snap.hotkeyError}</p>{/if}
          <button disabled={busy.includes("settings")} onclick={saveHotkey}>Save shortcut</button>
        </div>
        <div class="settings-group">
          <strong class="field-title">Filter by label</strong><p class="field-hint">Match any selected label. Filtering does not stop connections or change the tray menu.</p>
          <div class="button-row label-chips">
            <button class:chosen={!snap.settings.labelFilter.length} aria-pressed={!snap.settings.labelFilter.length} disabled={busy.includes("settings")} onclick={() => setLabelFilter([])}>All tunnels</button>
            {#each [...new Set([...snap.allLabels, ...snap.settings.labelFilter])] as label}
              <button class:chosen={snap.settings.labelFilter.some((item) => item.toLowerCase() === label.toLowerCase())} aria-pressed={snap.settings.labelFilter.some((item) => item.toLowerCase() === label.toLowerCase())} disabled={busy.includes("settings")} onclick={() => toggleLabel(label)}>{label}</button>
            {/each}
          </div>
        </div>
        <div class="settings-group">
          <label class="field"><span>devtunnel CLI</span><input bind:value={cliDraft} placeholder={snap.cliPath ?? "Auto-detect"} disabled={busy.includes("settings")} onkeydown={(event) => { if (event.key === "Enter") saveApp({ cliPath: cliDraft.trim() || null }); }} /></label>
          <p class="field-hint selectable">{snap.cliPath ? "Using " + snap.cliPath + (snap.cliVersion ? " (v" + snap.cliVersion + ")" : "") : "Not found"}</p>
          <div class="button-row"><button disabled={busy.includes("settings")} onclick={() => saveApp({ cliPath: cliDraft.trim() || null })}>Save path</button><button class="quiet" disabled={busy.includes("settings")} onclick={() => { cliDraft = ""; saveApp({ cliPath: null }); }}>Auto-detect</button></div>
        </div>
        <div class="settings-group">
          <strong class="field-title">Account</strong>
          <LoginPanel login={snap.login} loggedIn={!!snap.auth?.loggedIn} {account} busy={!snap.cliPath || busy.includes("login")} onStart={startLogin} onCancel={cancelLogin} onLogout={logout} onFallback={openLogin} onCopy={(code) => clipboard(code, "Code copied", "Sign-in device code", "login-code")} onOpen={openUrl} />
        </div>
        {#if update}
          <div class="settings-group update-settings">
            <div class="settings-row"><div><strong>Updates</strong><p class="field-hint" class:error-text={update.status === "error"} role={update.status === "error" ? "alert" : undefined}>Version {update.currentVersion} · {updateStatusText(update)}</p></div>
              {#if updateReady}<button class="primary" disabled={update.status === "installing" || busy.includes("update")} onclick={installUpdate}>{update.status === "installing" ? "Restarting…" : "Restart to update"}</button>
              {:else}<button class="quiet" disabled={!update.enabled || ["checking", "downloading"].includes(update.status) || busy.includes("update")} onclick={checkForUpdates}>Check for updates</button>{/if}
            </div>
            {#if updateReady}<p class="field-hint">{restartWarning}</p>{/if}
            {#if update.status === "downloading"}<progress class="update-progress" max="1" value={update.progress ?? 0} aria-label="Update download progress"></progress>{/if}
          </div>
        {/if}
        <div class="settings-group settings-row"><div><strong>Logs</strong><p class="field-hint selectable">{snap.logDir}</p></div><button class="quiet" disabled={busy.includes("folder")} onclick={openLogFolder}>Open folder</button></div>
      {:else}<p class="hint">Settings are unavailable until the initial snapshot loads.</p>{/if}
    </section>
  {/if}

  {#if notice}
    <div class="notification-dock">
      <Notification {notice} {reduced} onDismiss={() => (notice = null)} />
    </div>
  {/if}
  <footer class="app-footer">
    {#if view === "list"}<span>Updated {ago(snap?.lastRefresh)}</span><span class="spacer"></span>{#if updateReady && update && updateLater === update.version}<button class="quiet update-pill" onclick={() => (updateLater = null)}><Icon name="download" size={12} />Update ready</button>{/if}<button class="quiet" onclick={quit}>Quit</button>
    {:else if view === "expose"}<span class="spacer"></span><button class="quiet" disabled={busy.includes("expose")} onclick={back}>Cancel</button><button class="primary" form="expose-form" disabled={!ready || busy.includes("expose") || !exposePort}>{busy.includes("expose") ? "Creating…" : "Expose"}</button>
    {:else if view === "logs"}<button class="quiet" disabled={busy.includes("logs")} onclick={clearLogs}>Clear</button><button class="quiet" onclick={() => clipboard(logs.map((line) => `${line.ts} ${line.text}`).join("\n"), "Logs copied", logTunnel?.displayName ?? logId ?? "", "logs")}>Copy</button><span class="spacer"></span><button class="quiet" onclick={openLogFolder}>Open folder</button>
    {:else}<span>Tunnel Bar {appVersion}</span><span class="spacer"></span><button class="quiet" onclick={quit}>Quit Tunnel Bar</button>{/if}
  </footer>
</main>

<style>
  :global(:root) {
    color-scheme: light;
    --bg: #f3f5f8; --surface: #fff; --fg: #222630; --muted: #616976; --line: #dce1e8;
    --accent: #0066d9; --button-accent: #0867d5; --soft: #edf0f5; --wash: #e9f1fd;
    --ok: #187043; --ok-bg: #e7f4ec; --warn: #825800; --warn-bg: #fff3d4; --bad: #b22c32; --bad-bg: #ffedf0;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) { color-scheme: dark; --bg: #24272e; --surface: #30343c; --fg: #f0f2f6; --muted: #b1b7c3; --line: #454b55; --accent: #75b4ff; --button-accent: #347ed9; --soft: #2a2e35; --wash: #2c405b; --ok: #85d5a2; --ok-bg: #293f34; --warn: #f2cd77; --warn-bg: #463c26; --bad: #ffafb4; --bad-bg: #493039; }
  }
  :global(*) { box-sizing: border-box; }
  :global(html), :global(body) { margin: 0; height: 100%; overflow: hidden; font: 13px/1.35 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; color: var(--fg); background: var(--bg); -webkit-user-select: none; user-select: none; }
  main { display: flex; flex-direction: column; height: 100vh; height: 100dvh; border-radius: 18px; overflow: hidden; }
  main.windows { border-radius: 10px; font-family: "Segoe UI Variable", "Segoe UI", system-ui, sans-serif; --bg: #f0f3fa; background: var(--bg); }
  @media (prefers-color-scheme: dark) { main.windows { --bg: #232832; } }
  :global(button), :global(input), :global(select), :global(textarea) { font: inherit; color: inherit; }
  :global(button) { display: inline-flex; align-items: center; justify-content: center; gap: 5px; min-height: 28px; padding: 0 10px; font-size: 12px; border: 1px solid var(--line); border-radius: 7px; background: var(--surface); white-space: nowrap; cursor: pointer; transition: background-color 100ms ease, color 100ms ease, transform 100ms ease; }
  :global(button:hover:not(:disabled)) { background: var(--soft); }
  :global(button:active:not(:disabled)) { transform: translateY(1px); }
  :global(button:disabled), :global(input:disabled), :global(select:disabled), :global(textarea:disabled) { opacity: .5; cursor: default; }
  :global(button.primary) { background: var(--button-accent); border-color: var(--button-accent); color: #fff; font-weight: 550; }
  :global(button.primary:hover:not(:disabled)) { filter: brightness(.94); }
  :global(button.quiet) { background: transparent; border-color: transparent; color: var(--muted); }
  :global(button.quiet:hover:not(:disabled)) { color: var(--fg); background: var(--soft); }
  :global(button.icon-button) { width: 28px; height: 28px; padding: 0; background: transparent; border-color: transparent; color: var(--muted); flex: none; }
  :global(button.text-button) { min-height: 24px; height: auto; padding: 0; border: none; border-radius: 3px; background: transparent; color: var(--accent); justify-content: flex-start; }
  :global(button.text-button:hover:not(:disabled)) { background: transparent; text-decoration: underline; }
  :global(button.danger) { background: var(--bad-bg); color: var(--bad); border-color: var(--bad-bg); }
  :global(.danger-text), :global(.error-text) { color: var(--bad) !important; }
  :global(button:focus-visible), :global(input:focus-visible), :global(select:focus-visible), :global(textarea:focus-visible), :global(summary:focus-visible), :global([tabindex]:focus-visible) { outline: 2px solid var(--accent); outline-offset: 2px; }
  :global(svg) { flex: none; vertical-align: middle; }
  :global(.spacer) { flex: 1; min-width: 0; }
  :global(.hint), :global(.field-hint) { font-size: 11px; color: var(--muted); }
  :global(.selectable), :global(code), :global(.address), :global(.endpoint-url) { -webkit-user-select: text; user-select: text; }
  :global(.sr-only) { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0,0,0,0); white-space: nowrap; border: 0; }
  .app-header { min-height: 68px; display: flex; align-items: center; gap: 8px; padding: 13px 16px; border-bottom: 1px solid var(--line); flex: none; }
  .header-title { flex: 1; min-width: 0; }
  h1 { margin: 0; font-size: 16px; font-weight: 650; letter-spacing: -.25px; line-height: 1.3; outline: none; }
  h1:focus-visible { outline: none; }
  .header-subtitle { display: block; margin-top: 3px; font-size: 11px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .header-tools { display: flex; align-items: center; gap: 2px; }
  .expose-button { height: 29px; }
  .list-content, :global(.settings-content), .logs-content { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .list-content { padding: 8px 16px; }
  .section-heading { display: flex; align-items: center; justify-content: space-between; margin: 0 0 6px; color: var(--muted); font-size: 12px; }
  .section-heading h2 { font-size: 12px; font-weight: 600; margin: 0; }
  .section-heading.subsequent { margin-top: 10px; }
  :global(.tunnel-card) { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 8px; margin-bottom: 8px; }
  :global(.tunnel-card.attention) { border-color: var(--bad); }
  :global(.card-heading) { display: flex; align-items: center; gap: 8px; }
  :global(.role-icon) { width: 30px; height: 30px; border-radius: 8px; background: var(--soft); color: var(--muted); display: grid; place-items: center; flex: none; }
  :global(.role-icon.ok) { background: var(--ok-bg); color: var(--ok); }
  :global(.role-icon.busy) { background: var(--warn-bg); color: var(--warn); }
  :global(.role-icon.bad) { background: var(--bad-bg); color: var(--bad); }
  :global(.tunnel-heading) { flex: 1; min-width: 0; }
  :global(button.tunnel-name) { display: block; padding: 0; min-height: 17px; max-width: 100%; font-size: 13px; font-weight: 620; letter-spacing: -.15px; overflow: hidden; text-overflow: ellipsis; background: transparent; border: none; border-radius: 3px; text-align: left; }
  :global(button.tunnel-name:hover) { background: transparent; }
  :global(.tunnel-state) { display: flex; align-items: center; gap: 5px; margin-top: 3px; font-size: 11px; color: var(--muted); flex-wrap: wrap; transition: color 160ms ease; }
  :global(.tunnel-state.ok) { color: var(--ok); }
  :global(.tunnel-state.busy) { color: var(--warn); }
  :global(.tunnel-state.bad) { color: var(--bad); }
  :global(.host-availability) { color: var(--muted); }
  :global(.state-dot) { width: 5px; height: 5px; border-radius: 50%; background: currentColor; flex: none; }
  :global(.access-line) { display: flex; align-items: center; gap: 5px; color: var(--muted); font-size: 11px; margin-top: 7px; min-width: 0; flex-wrap: wrap; }
  :global(.access-line.public-access) { color: var(--warn); }
  :global(button.access-action) { font-size: 11px; margin-left: 3px; min-height: 20px; }
  :global(.endpoint) { display: flex; align-items: center; gap: 6px; padding: 5px 0; margin-top: 2px; border-bottom: 1px solid var(--line); }
  :global(.endpoint code), :global(.endpoint .address) { font: 12px/1.35 ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace; white-space: nowrap; color: var(--fg); }
  :global(.endpoint > svg) { color: var(--muted); }
  :global(button.endpoint-url) { flex: 1; min-width: 0; display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font: 11px/1.4 ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace; text-align: left; }
  :global(button.endpoint-tool) { width: 24px; height: 24px; min-height: 24px; }
  :global(.inline-warning) { display: flex; align-items: flex-start; gap: 6px; padding: 6px; font-size: 11px; color: var(--warn); background: var(--warn-bg); border-radius: 6px; margin-top: 6px; overflow-wrap: anywhere; }
  :global(.port-summary) { margin-top: 7px; font-size: 11px; color: var(--muted); overflow-wrap: anywhere; }
  :global(.card-actions) { display: flex; align-items: center; gap: 5px; margin-top: 6px; }
  :global(.card-actions button) { padding: 0 9px; }
  :global(.auto-toggle) { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; color: var(--muted); white-space: nowrap; }
  :global(input[type="checkbox"]) { accent-color: var(--button-accent); margin: 0; width: 12px; height: 12px; flex: none; }
  :global(.confirmation) { padding-top: 10px; margin-top: 8px; border-top: 1px solid var(--line); font-size: 12px; }
  :global(.confirmation p) { margin: 5px 0 10px; color: var(--muted); }
  :global(.button-row), :global(.details-actions) { display: flex; align-items: center; gap: 5px; flex-wrap: wrap; }
  :global(.tunnel-details) { margin-top: 10px; padding-top: 10px; border-top: 1px solid var(--line); }
  :global(.detail-id) { display: flex; flex-direction: column; gap: 3px; margin-bottom: 8px; }
  :global(.detail-id code) { font: 11px/1.4 ui-monospace, Menlo, Consolas, monospace; overflow-wrap: anywhere; }
  :global(.nickname) { width: 100%; min-width: 0; padding: 2px 4px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); font-size: 13px; }
  .app-footer { display: flex; align-items: center; gap: 5px; height: 34px; flex: none; border-top: 1px solid var(--line); padding: 0 16px; font-size: 11px; color: var(--muted); }
  .app-footer button { min-height: 25px; height: 25px; font-size: 11px; }
  .notification-dock { flex: none; padding: 6px 12px; }
  :global(.notification) { display: flex; align-items: center; gap: 9px; padding: 9px 10px; min-height: 56px; background: var(--surface); border: 1px solid var(--line); border-radius: 10px; box-shadow: 0 4px 12px #00000014; }
  :global(.notification-mark) { width: 25px; height: 25px; border-radius: 7px; display: grid; place-items: center; flex: none; color: var(--ok); background: var(--ok-bg); }
  :global(.notification-error .notification-mark) { color: var(--bad); background: var(--bad-bg); }
  :global(.notification-copy) { display: flex; flex-direction: column; flex: 1; min-width: 0; }
  :global(.notification-copy strong) { font-size: 12px; font-weight: 600; }
  :global(.notification-copy > span) { font-size: 11px; color: var(--muted); margin-top: 3px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  :global(.notification-error .notification-copy > span) { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 95px; overflow-y: auto; }
  :global(.settings-content) { padding: 16px; }
  .view-description { font-size: 12px; color: var(--muted); margin: 3px 0 25px; }
  :global(.field) { display: flex; flex-direction: column; gap: 7px; margin-top: 18px; }
  :global(.field > span), :global(.field-title) { font-size: 12px; font-weight: 600; }
  :global(.field input), :global(.field select), :global(.field textarea) { width: 100%; height: 36px; min-width: 0; background: var(--surface); border: 1px solid var(--line); border-radius: 7px; padding: 0 10px; }
  :global(.field textarea) { height: auto; padding: 8px 10px; resize: vertical; }
  :global(.field input::placeholder), :global(.field textarea::placeholder) { color: var(--muted); }
  :global(.field-hint) { margin: 7px 0 14px; line-height: 1.5; overflow-wrap: anywhere; }
  :global(.setting-toggle) { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 15px 0; }
  :global(.setting-toggle strong), .settings-row strong { font-weight: 550; font-size: 13px; }
  :global(.setting-toggle small) { display: block; font-size: 11px; color: var(--muted); margin-top: 4px; }
  :global(input.switch) { appearance: none; -webkit-appearance: none; width: 29px; height: 17px; border-radius: 12px; background: #818b9a; position: relative; cursor: pointer; transition: background-color 140ms ease; }
  :global(input.switch::after) { content: ""; width: 13px; height: 13px; border-radius: 50%; background: #fff; position: absolute; top: 2px; left: 2px; transition: transform 140ms ease; }
  :global(input.switch:checked) { background: var(--button-accent); }
  :global(input.switch:checked::after) { transform: translateX(12px); }
  .access-explanation { display: flex; align-items: flex-start; gap: 7px; padding: 10px; font-size: 11px; color: var(--muted); border-radius: 6px; }
  .access-explanation p { margin: 0; }
  .warning { color: var(--warn); background: var(--warn-bg); }
  .settings-group { padding: 0 0 16px; margin-bottom: 16px; border-bottom: 1px solid var(--line); }
  .settings-group .field { margin-top: 0; }
  .settings-group .setting-toggle + .setting-toggle { border-top: 1px solid var(--line); }
  .settings-group:first-child { padding-bottom: 0; }
  .settings-row { display: flex; align-items: center; gap: 10px; }
  .settings-row > div { flex: 1; min-width: 0; }
  .settings-row .field-hint { margin-bottom: 0; }
  .logs-content { padding: 15px; background: var(--surface); -webkit-user-select: text; user-select: text; }
  .log-line { font: 11px/1.6 ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace; margin-bottom: 9px; }
  .log-line time { display: block; color: var(--muted); font-size: 10px; }
  .log-line > span { white-space: pre-wrap; overflow-wrap: anywhere; }
  .log-error { color: var(--bad); }
  .log-app { color: var(--accent); }
  .empty-state { display: flex; flex-direction: column; align-items: center; gap: 18px; text-align: center; padding: 42px 14px; }
  .empty-state :global(> svg) { color: var(--accent); }
  .empty-state h2 { font-size: 20px; letter-spacing: -.4px; margin: 0; }
  .empty-state p { color: var(--muted); line-height: 1.6; margin: 0; }
  .empty-state code { font-size: 11px; overflow-wrap: anywhere; }
  .loading-state { display: flex; flex-direction: column; gap: 12px; padding: 12px 0; }
  .loading-state > div { height: 92px; border: 1px solid var(--line); background: var(--soft); border-radius: 10px; }
  .banner { display: flex; align-items: flex-start; gap: 8px; font-size: 12px; padding: 10px; border-radius: 7px; margin-bottom: 10px; }
  .banner p { margin: 6px 0; overflow-wrap: anywhere; }
  .update-banner { background: var(--wash); border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent); }
  .update-banner > :global(svg) { color: var(--accent); flex: none; }
  .update-banner > div { flex: 1; min-width: 0; }
  .update-banner .button-row { margin-top: 8px; }
  .update-pill { color: var(--accent); gap: 4px; }
  .update-progress { width: 100%; height: 4px; margin-top: 8px; accent-color: var(--accent); }
  .refreshing { animation: rotate 1s linear infinite; }
  :global(.copy-check path) { stroke-dasharray: 24; animation: confirm-copy 160ms ease-out; }
  @keyframes confirm-copy { from { stroke-dashoffset: 24; } to { stroke-dashoffset: 0; } }
  @keyframes rotate { to { transform: rotate(360deg); } }
  .optional { font-weight: 400; color: var(--muted); }
  :global(.advanced-options) { border-top: 1px solid var(--line); margin-top: 12px; padding-top: 12px; }
  :global(.advanced-options summary) { cursor: pointer; color: var(--muted); font-size: 12px; }
  :global(.two-fields) { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  :global(.management-section) { border: 1px solid var(--line); border-radius: 9px; background: var(--surface); margin: 12px 0; padding: 12px; }
  :global(.management-section > summary), :global(.port-editor > summary) { cursor: pointer; font-size: 12px; font-weight: 600; }
  :global(.management-section > summary > span), :global(.port-editor > summary > span) { float: right; color: var(--muted); font-size: 11px; font-weight: 400; }
  :global(.management-section form + form), :global(.port-editor), :global(.danger-zone), :global(.draft-rule) { margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--line); }
  :global(.management-section form > button) { margin: 12px 0 0; }
  :global(.header-fields), :global(.login-panel) { margin-top: 12px; }
  :global(.header-fields code) { overflow-wrap: anywhere; }
  :global(.access-entry) { display: flex; align-items: flex-start; gap: 8px; padding: 10px 0; border-bottom: 1px solid var(--line); }
  :global(.access-entry > div) { flex: 1; min-width: 0; }
  :global(.access-entry strong), :global(.access-entry small) { display: block; overflow-wrap: anywhere; font-size: 11px; }
  :global(.access-entry small) { margin-top: 4px; color: var(--muted); }
  :global(.guard-confirmation) { padding: 12px; border-radius: 8px; border: 1px solid var(--warn); background: var(--warn-bg); margin-bottom: 16px; }
  :global(.account-label), :global(.inline-progress) { margin: 0 0 12px; font-size: 12px; overflow-wrap: anywhere; }
  :global(.login-mode) { margin: 12px 0; }
  :global(.login-panel > .text-button) { display: block; font-size: 11px; }
  :global(.device-code) { display: flex; align-items: flex-start; flex-direction: column; gap: 12px; margin-bottom: 12px; }
  :global(.device-code code) { font-size: 18px; letter-spacing: 2px; }
  :global(.label-chips button) { max-width: 100%; white-space: normal; overflow-wrap: anywhere; padding: 4px 8px; }
  :global(button.chosen) { color: var(--accent); background: var(--wash); border-color: var(--accent); }
  @media (prefers-reduced-motion: reduce) {
    :global(*), :global(*::before), :global(*::after) { animation: none !important; transition: none !important; scroll-behavior: auto !important; }
    :global(button:active:not(:disabled)) { transform: none; }
  }
</style>
