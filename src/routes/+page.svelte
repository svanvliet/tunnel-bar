<script lang="ts">
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import type { Snapshot, TunnelView, LogLine, Forward } from "$lib/types";
  import { stateLabel, tone, isActive } from "$lib/types";

  let snap = $state<Snapshot | null>(null);
  let view = $state<"list" | "logs" | "settings">("list");
  let logId = $state<string | null>(null);
  let logs = $state<LogLine[]>([]);
  let follow = $state(true);
  let error = $state<string | null>(null);
  let toast = $state<string | null>(null);
  let editing = $state<string | null>(null);
  let nickDraft = $state("");
  let cliDraft = $state("");
  let logBox = $state<HTMLDivElement | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let now = $state(Date.now());

  function flash(msg: string) {
    toast = msg;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), 1800);
  }

  async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
    error = null;
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    invoke<Snapshot>("get_snapshot").then((s) => (snap = s));
    const un1 = listen<Snapshot>("snapshot", (e) => (snap = e.payload));
    const un2 = listen<{ id: string; line: LogLine }>("log", async (e) => {
      if (view === "logs" && e.payload.id === logId) {
        logs.push(e.payload.line);
        if (logs.length > 2000) logs.splice(0, logs.length - 2000);
        if (follow) { await tick(); logBox?.scrollTo(0, logBox.scrollHeight); }
      }
    });
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && view !== "list") view = "list"; };
    window.addEventListener("keydown", onKey);
    const clock = setInterval(() => (now = Date.now()), 5000);
    return () => { un1.then((f) => f()); un2.then((f) => f()); window.removeEventListener("keydown", onKey); clearInterval(clock); };
  });

  async function openLogs(t: TunnelView) {
    logId = t.id;
    logs = (await call<LogLine[]>("get_logs", { id: t.id })) ?? [];
    view = "logs";
    follow = true;
    await tick();
    logBox?.scrollTo(0, logBox.scrollHeight);
  }

  function openSettings() {
    cliDraft = snap?.settings.cliPath ?? "";
    view = "settings";
  }

  async function saveTunnel(t: TunnelView, patch: Partial<TunnelView["settings"]>) {
    await call("update_tunnel_settings", { id: t.id, settings: { ...t.settings, ...patch } });
  }

  async function saveApp(patch: { launchAtLogin?: boolean; notifications?: boolean; cliPath?: string | null }) {
    if (!snap) return;
    const s = snap.settings;
    await call("update_app_settings", {
      launchAtLogin: patch.launchAtLogin ?? s.launchAtLogin,
      notifications: patch.notifications ?? s.notifications,
      cliPath: patch.cliPath !== undefined ? patch.cliPath : (s.cliPath ?? null),
    });
  }

  async function copySsh(t: TunnelView, f: Forward) {
    const cmd = await call<string>("ssh_command", { id: t.id, localPort: f.localPort, remotePort: f.remotePort });
    if (cmd) { await writeText(cmd); flash(`Copied: ${cmd}`); }
  }

  async function copyAddr(f: Forward) {
    await writeText(`localhost:${f.localPort}`);
    flash(`Copied localhost:${f.localPort}`);
  }

  function startEdit(t: TunnelView) {
    editing = t.id;
    nickDraft = t.settings.nickname ?? "";
  }

  async function commitEdit(t: TunnelView) {
    editing = null;
    const v = nickDraft.trim();
    await saveTunnel(t, { nickname: v || null });
  }

  const ago = (iso?: string | null) => {
    if (!iso) return "never";
    const s = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
    return s < 60 ? `${s}s ago` : `${Math.round(s / 60)}m ago`;
  };
  const logTunnel = $derived(snap?.tunnels.find((t) => t.id === logId));
  const isSsh = (f: Forward, t: TunnelView) =>
    f.remotePort === 22 || f.remotePort === 2222 || !!t.settings.sshHost ||
    t.ports.some((p) => p.portNumber === f.remotePort && p.protocol === "ssh");
</script>

<main>
  {#if view === "list"}
    <header>
      <div class="title">
        <strong>TunnelBar</strong>
        {#if snap?.auth}
          <span class="sub" class:bad={!snap.auth.loggedIn} title={snap.auth.message}>
            {snap.auth.loggedIn ? snap.auth.message.replace(/^Logged in as /, "").replace(/ using .*$/, "").replace(/\.$/, "") : "Not signed in"}
          </span>
        {/if}
      </div>
      <button class="icon" title="Refresh" disabled={snap?.refreshing} onclick={() => call("refresh")}>
        <span class:spin={snap?.refreshing}>⟳</span>
      </button>
      <button class="icon" title="Settings" onclick={openSettings}>⚙</button>
    </header>

    {#if !snap}
      <p class="empty">Loading…</p>
    {:else if !snap.cliPath}
      <div class="banner bad">
        devtunnel CLI not found. Install it (<code>brew install --cask devtunnel</code>) or set its path in Settings.
      </div>
    {:else if snap.auth && !snap.auth.loggedIn}
      <div class="banner bad">
        You're not signed in to Dev Tunnels.
        <button onclick={() => call("open_login")}>Sign in…</button>
      </div>
    {/if}
    {#if snap?.refreshError}
      <div class="banner warn">{snap.refreshError}</div>
    {/if}

    <section class="list">
      {#each snap?.tunnels ?? [] as t (t.id)}
        {@const tn = tone(t.state)}
        <article class="card {tn}">
          <div class="row">
            <span class="dot {tn}"></span>
            {#if editing === t.id}
              <!-- svelte-ignore a11y_autofocus -->
              <input class="nick" bind:value={nickDraft} autofocus placeholder={t.summary.name || t.id}
                onblur={() => commitEdit(t)}
                onkeydown={(e) => { if (e.key === "Enter") commitEdit(t); if (e.key === "Escape") editing = null; }} />
            {:else}
              <button class="name" title="Rename (local nickname)" ondblclick={() => startEdit(t)}>{t.displayName}</button>
            {/if}
            <span class="host" class:online={t.summary.hostConnections > 0}
              title={t.summary.hostConnections > 0 ? "Host is online" : "Host is offline"}>
              {t.summary.hostConnections > 0 ? "host online" : "host offline"}
            </span>
          </div>
          <div class="meta">
            <span class="id" title={t.id}>{t.id}</span>
            {#if t.orphaned}<span class="tag warn">not on account</span>{/if}
          </div>
          <div class="state {tn}" title={"message" in t.state ? t.state.message : ""}>
            {stateLabel(t.state)}
            {#if t.state.kind === "backoff" || t.state.kind === "authRequired"}
              <div class="msg">{t.state.message}</div>
            {/if}
          </div>

          {#if t.forwards.length}
            <ul class="fwd">
              {#each t.forwards as f (f.localPort)}
                <li>
                  <button class="link" title="Copy address" onclick={() => copyAddr(f)}>localhost:{f.localPort}</button>
                  <span class="arrow">→</span> <span>:{f.remotePort}</span>
                  {#if isSsh(f, t)}
                    <span class="spacer"></span>
                    <button class="small" onclick={() => copySsh(t, f)}>Copy SSH</button>
                    <button class="small" onclick={() => call("open_ssh", { id: t.id, localPort: f.localPort, remotePort: f.remotePort })}>Open SSH</button>
                  {/if}
                </li>
              {/each}
            </ul>
          {:else if t.ports.length && !isActive(t.state) && t.state.kind !== "external"}
            <div class="ports">Ports: {t.ports.map((p) => `${p.portNumber}${p.protocol && p.protocol !== "auto" ? "/" + p.protocol : ""}`).join(", ")}</div>
          {/if}

          <div class="actions">
            {#if t.state.kind === "external"}
              <button onclick={() => call("adopt_external", { id: t.id })} title="Stop the external process and manage the connection here">Take over</button>
              <button class="ghost" onclick={() => call("stop_external", { id: t.id })}>Stop</button>
            {:else if isActive(t.state)}
              <button class="danger" onclick={() => call("disconnect", { id: t.id })}>Disconnect</button>
            {:else}
              <button class="primary" disabled={!snap?.cliPath} onclick={() => call("connect", { id: t.id })}>Connect</button>
            {/if}
            <span class="spacer"></span>
            <label class="toggle" title="Connect automatically when TunnelBar starts">
              <input type="checkbox" checked={t.settings.autoConnect}
                onchange={(e) => saveTunnel(t, { autoConnect: e.currentTarget.checked })} />
              Auto
            </label>
            <button class="ghost" onclick={() => openLogs(t)}>Logs</button>
          </div>
        </article>
      {:else}
        {#if snap?.cliPath && snap?.auth?.loggedIn}
          <p class="empty">No tunnels on this account yet.</p>
        {/if}
      {/each}
    </section>

    <footer>
      <span>Updated {ago(snap?.lastRefresh)}</span>
      <span class="spacer"></span>
      <button class="ghost" onclick={() => call("quit")}>Quit</button>
    </footer>
  {:else if view === "logs"}
    <header>
      <button class="icon" title="Back" onclick={() => (view = "list")}>‹</button>
      <div class="title">
        <strong>Logs</strong>
        <span class="sub">{logTunnel?.displayName ?? logId}</span>
      </div>
      <label class="toggle"><input type="checkbox" bind:checked={follow} /> Follow</label>
    </header>
    <div class="logs" bind:this={logBox}
      onscroll={() => { if (logBox) follow = logBox.scrollTop + logBox.clientHeight >= logBox.scrollHeight - 4; }}>
      {#each logs as l, i (i)}
        <div class="line {l.stream}"><span class="ts">{l.ts.slice(11, 19)}</span> {l.text}</div>
      {:else}
        <p class="empty">No output yet. Connect to see devtunnel logs here.</p>
      {/each}
    </div>
    <footer>
      <button class="ghost" onclick={async () => { await call("clear_logs", { id: logId }); logs = []; }}>Clear</button>
      <button class="ghost" onclick={async () => { await writeText(logs.map((l) => `${l.ts} ${l.text}`).join("\n")); flash("Copied logs"); }}>Copy</button>
      <span class="spacer"></span>
      <button class="ghost" onclick={() => call("open_log_dir")}>Open folder</button>
    </footer>
  {:else}
    <header>
      <button class="icon" title="Back" onclick={() => (view = "list")}>‹</button>
      <div class="title"><strong>Settings</strong></div>
    </header>
    {#if snap}
      <section class="settings">
        <label class="toggle"><input type="checkbox" checked={snap.settings.launchAtLogin}
          onchange={(e) => saveApp({ launchAtLogin: e.currentTarget.checked })} /> Launch at login</label>
        <label class="toggle"><input type="checkbox" checked={snap.settings.notifications}
          onchange={(e) => saveApp({ notifications: e.currentTarget.checked })} /> Notify on drops and reconnects</label>

        <div class="field">
          <span>devtunnel CLI</span>
          <input bind:value={cliDraft} placeholder={snap.cliPath ?? "Auto-detect"}
            onkeydown={(e) => { if (e.key === "Enter") saveApp({ cliPath: cliDraft.trim() || null }); }} />
          <div class="hint">
            {#if snap.cliPath}Using {snap.cliPath}{snap.cliVersion ? ` (v${snap.cliVersion})` : ""}{:else}Not found{/if}
          </div>
          <div class="actions">
            <button onclick={() => saveApp({ cliPath: cliDraft.trim() || null })}>Save path</button>
            <button class="ghost" onclick={() => { cliDraft = ""; saveApp({ cliPath: null }); }}>Auto-detect</button>
          </div>
        </div>

        <div class="field">
          <span>Account</span>
          <div class="hint">{snap.auth?.message ?? "Unknown"}</div>
          <div class="actions">
            <button onclick={() => call("open_login")}>{snap.auth?.loggedIn ? "Switch account…" : "Sign in…"}</button>
          </div>
        </div>

        <div class="field">
          <span>Logs</span>
          <div class="hint">{snap.logDir}</div>
          <div class="actions"><button class="ghost" onclick={() => call("open_log_dir")}>Open folder</button></div>
        </div>
      </section>
      <footer>
        <span>TunnelBar 0.1.0</span>
        <span class="spacer"></span>
        <button class="danger" onclick={() => call("quit")}>Quit TunnelBar</button>
      </footer>
    {/if}
  {/if}

  {#if error}
    <button class="toast bad" onclick={() => (error = null)}>{error}</button>
  {:else if toast}
    <div class="toast">{toast}</div>
  {/if}
</main>

<style>
  :global(html), :global(body) {
    margin: 0; height: 100%; overflow: hidden;
    font: 13px/1.35 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    color: var(--fg); background: var(--bg);
    -webkit-user-select: none; user-select: none; cursor: default;
  }
  :global(:root) {
    --bg: #f6f6f7; --card: #fff; --fg: #1d1d1f; --mute: #6e6e73; --line: #e2e2e6;
    --ok: #1f9d55; --busy: #d08a00; --bad: #d93025; --accent: #0a66d8;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) { --bg: #1e1e20; --card: #2a2a2d; --fg: #f2f2f4; --mute: #9a9aa1; --line: #3a3a3e; --accent: #4b9bff; }
  }
  main { display: flex; flex-direction: column; height: 100vh; border-radius: 10px; overflow: hidden; }
  header, footer { display: flex; align-items: center; gap: 6px; padding: 10px 12px; }
  header { border-bottom: 1px solid var(--line); }
  footer { border-top: 1px solid var(--line); color: var(--mute); font-size: 12px; }
  .title { flex: 1; display: flex; flex-direction: column; min-width: 0; }
  .sub { color: var(--mute); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sub.bad { color: var(--bad); }
  .spacer { flex: 1; }
  .list, .settings { flex: 1; overflow-y: auto; padding: 10px; display: flex; flex-direction: column; gap: 8px; }
  .card { background: var(--card); border: 1px solid var(--line); border-left: 3px solid var(--line); border-radius: 8px; padding: 10px; }
  .card.ok { border-left-color: var(--ok); } .card.busy { border-left-color: var(--busy); } .card.bad { border-left-color: var(--bad); }
  .row { display: flex; align-items: center; gap: 8px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--mute); flex: none; }
  .dot.ok { background: var(--ok); } .dot.busy { background: var(--busy); animation: pulse 1s infinite; } .dot.bad { background: var(--bad); }
  .name { all: unset; font-weight: 600; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nick { flex: 1; font: inherit; font-weight: 600; padding: 2px 4px; }
  .host { font-size: 11px; color: var(--mute); }
  .host.online { color: var(--ok); }
  .meta { display: flex; gap: 6px; margin: 2px 0 0 16px; font-size: 11px; color: var(--mute); }
  .id { font-family: ui-monospace, Menlo, monospace; -webkit-user-select: text; user-select: text; }
  .tag.warn { color: var(--busy); }
  .state { margin: 6px 0 0 16px; font-size: 12px; color: var(--mute); }
  .state.ok { color: var(--ok); } .state.busy { color: var(--busy); } .state.bad { color: var(--bad); }
  .msg { color: var(--mute); font-size: 11px; margin-top: 2px; overflow: hidden; text-overflow: ellipsis; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; }
  .fwd { list-style: none; margin: 8px 0 0 16px; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .fwd li { display: flex; align-items: center; gap: 6px; font-family: ui-monospace, Menlo, monospace; font-size: 12px; }
  .arrow { color: var(--mute); }
  .ports { margin: 6px 0 0 16px; font-size: 11px; color: var(--mute); }
  .actions { display: flex; align-items: center; gap: 6px; margin-top: 8px; }
  button { font: inherit; font-size: 12px; padding: 4px 10px; border-radius: 6px; border: 1px solid var(--line); background: var(--card); color: var(--fg); cursor: pointer; }
  button:hover:not(:disabled) { filter: brightness(0.96); }
  button:disabled { opacity: 0.5; cursor: default; }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button.danger { color: var(--bad); }
  button.ghost { background: transparent; border-color: transparent; color: var(--mute); }
  button.ghost:hover { color: var(--fg); }
  button.small { padding: 1px 6px; font-size: 11px; font-family: -apple-system, system-ui, sans-serif; }
  button.icon { background: transparent; border: none; font-size: 16px; padding: 2px 6px; color: var(--mute); }
  button.link { all: unset; color: var(--accent); cursor: pointer; }
  button.link:hover { text-decoration: underline; }
  .toggle { display: flex; align-items: center; gap: 4px; font-size: 12px; color: var(--mute); }
  .settings .toggle { font-size: 13px; color: var(--fg); }
  .banner { margin: 10px 10px 0; padding: 8px 10px; border-radius: 6px; font-size: 12px; }
  .banner.bad { background: color-mix(in srgb, var(--bad) 12%, transparent); }
  .banner.warn { background: color-mix(in srgb, var(--busy) 14%, transparent); }
  .banner button { margin-top: 6px; display: block; }
  .empty { color: var(--mute); text-align: center; padding: 30px 10px; }
  .field { display: flex; flex-direction: column; gap: 4px; padding-top: 8px; border-top: 1px solid var(--line); }
  .field > span { font-weight: 600; }
  .field input { font: inherit; padding: 4px 6px; border-radius: 5px; border: 1px solid var(--line); background: var(--card); color: var(--fg); }
  .hint { font-size: 11px; color: var(--mute); word-break: break-all; -webkit-user-select: text; user-select: text; }
  .logs { flex: 1; overflow-y: auto; padding: 8px 10px; font: 11px/1.45 ui-monospace, Menlo, monospace; -webkit-user-select: text; user-select: text; background: var(--card); }
  .line { white-space: pre-wrap; word-break: break-word; }
  .line.stderr { color: var(--bad); }
  .line.app { color: var(--accent); }
  .ts { color: var(--mute); }
  .toast { all: unset; position: fixed; left: 10px; right: 10px; bottom: 48px; padding: 8px 10px; border-radius: 6px; background: #000c; color: #fff; font-size: 12px; text-align: center; cursor: pointer; }
  .toast.bad { background: var(--bad); }
  .spin { display: inline-block; animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes pulse { 50% { opacity: 0.35; } }
</style>
