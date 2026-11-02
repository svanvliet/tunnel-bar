<script lang="ts" module>
  export function providerName(p: LoginProvider | null | undefined) { return p === "github" ? "GitHub" : "Microsoft"; }
  /** "14:32" for a deadline, clamped at 0:00. */
  export function countdown(until: string, now: number) {
    const s = Math.max(0, Math.ceil((Date.parse(until) - now) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }
  export function secondsLeft(until: string, now: number) { return Math.max(0, Math.ceil((Date.parse(until) - now) / 1000)); }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";
  import type { AuthState, LoginProvider, LoginState } from "./types";

  let { authState, login, lastProvider, refreshing, busy, windows, onStart, onCancel, onFallback, onCopy, onOpen, onRefresh, onSettings, onLogs, onCopyCommand }: {
    authState: AuthState; login?: LoginState | null; lastProvider?: LoginProvider | null; refreshing: boolean; busy: boolean; windows: boolean;
    onStart: (provider: LoginProvider, deviceCode: boolean) => Promise<void>; onCancel: () => Promise<void>; onFallback: () => Promise<void>;
    onCopy: (code: string) => Promise<void>; onOpen: (url: string) => Promise<void>; onRefresh: () => Promise<void>;
    onSettings: () => void; onLogs: () => Promise<void>; onCopyCommand: (command: string) => Promise<void>;
  } = $props();

  let deviceMode = $state(false);
  /** Failure the user moved past ("Use a different sign-in method"), by content. */
  let dismissed = $state<string | null>(null);
  let now = $state(Date.now());
  onMount(() => { const t = setInterval(() => (now = Date.now()), 1000); return () => clearInterval(t); });

  const installCommand = $derived(windows ? "winget install Microsoft.devtunnel" : "brew install --cask devtunnel");
  const running = $derived(login?.kind === "running" ? login : null);
  const device = $derived(login?.kind === "deviceCode" ? login : null);
  const failedKey = $derived(login?.kind === "failed" ? JSON.stringify(login) : null);
  const failed = $derived(login?.kind === "failed" && failedKey !== dismissed ? login : null);
  const codeExpired = $derived(failed?.reason === "timedOut" && failed.deviceCode ? failed : null);
  const primary = $derived<LoginProvider>(authState.kind === "expired" && lastProvider ? lastProvider : "microsoft");
  const order = $derived<LoginProvider[]>(primary === "github" ? ["github", "microsoft"] : ["microsoft", "github"]);
  const offlineLeft = $derived(authState.kind === "offline" ? secondsLeft(authState.retryAt, now) : 0);
  const failureText = $derived.by(() => {
    if (!failed) return null;
    const who = providerName(failed.provider);
    switch (failed.reason) {
      case "cancelled": return null;
      case "timedOut": return { title: "Sign-in timed out", body: `${who} didn’t confirm the sign-in in time. Try again.` };
      case "denied": return { title: "Sign-in didn’t complete", body: "This account can’t use Dev Tunnels. Try another account, or ask your administrator." };
      case "offline": return { title: "Couldn’t reach the sign-in service", body: "Check your internet connection, then try again." };
      default: return { title: "Sign-in didn’t complete", body: `${who} sign-in stopped before it finished. Try again or choose another account.` };
    }
  });
</script>

{#snippet header(icon: "lock" | "globe" | "terminal" | "warning", tone: "" | "warn" | "bad", orbit: boolean, title: string, body?: string)}
  <div class="auth-badge {tone}" class:orbiting={orbit} data-testid="auth-badge">
    <Icon name={icon} size={26} />
    {#if orbit}
      <svg class="auth-orbit" width="72" height="72" viewBox="0 0 72 72" aria-hidden="true" data-testid="auth-orbit">
        <circle class="track" cx="36" cy="36" r="35" />
        <circle class="arc" cx="36" cy="36" r="35" pathLength="100" />
      </svg>
    {/if}
  </div>
  <h2>{title}</h2>
  {#if body}<p class="auth-body">{body}</p>{/if}
{/snippet}

{#snippet spinner()}<span class="auth-spin" aria-hidden="true"></span>{/snippet}

{#snippet links(disabled: boolean)}
  <div class="auth-links">
    <button class="auth-link" {disabled} onclick={() => (deviceMode = !deviceMode)}>{deviceMode ? "Use the browser instead" : "Use a device code"}</button>
    <button class="auth-link" {disabled} onclick={onFallback}>Use Terminal instead</button>
  </div>
{/snippet}

<div class="auth-screen" data-auth={device ? "deviceCode" : running ? "signingIn" : codeExpired ? "codeExpired" : authState.kind}>
  {#if device}
    {@render header("lock", "", true, "Enter this code in your browser")}
    <div class="auth-column">
      <div class="auth-card code-card">
        <span class="code-label">Your one-time {providerName(device.provider)} code</span>
        <div class="code-box"><code class="selectable">{device.code}</code><button class="icon-button" title="Copy code" aria-label="Copy code" onclick={() => onCopy(device.code)}><Icon name="copy" size={15} /></button></div>
        <button class="primary" onclick={() => onOpen(device.url)}>Open sign-in page<Icon name="arrow" size={14} /></button>
        <span class="auth-wait"><span role="status">{@render spinner()}Waiting for you</span> · expires in <span class="countdown" aria-hidden="true">{countdown(device.expiresAt, now)}</span><span class="sr-only">at {new Date(device.expiresAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}</span></span>
      </div>
      <div class="auth-stack"><button class="quiet-outline" disabled={busy} onclick={onCancel}>Cancel sign-in</button></div>
    </div>
  {:else if running}
    {@render header("lock", "", true, "Sign in to Dev Tunnels", `Use your ${providerName(running.provider)} account in the browser window that just opened.`)}
    <div class="auth-column">
      <div class="auth-notice" role="status"><Icon name="globe" size={16} /><span>Finish signing in in your browser. The other options are unavailable until you finish or cancel.</span></div>
      <div class="auth-stack">
        {#each order as p (p)}
          {#if p === running.provider}
            <button class:primary={p === primary} class="waiting" disabled>{@render spinner()}Waiting for {providerName(p)}…</button>
          {:else}
            <button class:primary={p === primary} disabled>Continue with {providerName(p)}</button>
          {/if}
        {/each}
        <button class="quiet-outline" disabled={busy} onclick={onCancel}>Cancel sign-in</button>
      </div>
      {@render links(true)}
    </div>
  {:else if authState.kind === "cliMissing"}
    {@render header("terminal", "bad", false, "devtunnel CLI not found", "Tunnel Bar needs Microsoft’s devtunnel CLI. Install it, then check again.")}
    <div class="auth-column">
      <div class="auth-card command-box"><code class="selectable">{installCommand}</code><button class="icon-button" title="Copy command" aria-label="Copy install command" onclick={() => onCopyCommand(installCommand)}><Icon name="copy" size={14} /></button></div>
      <div class="auth-pair">
        <button class="primary" disabled={refreshing} onclick={onRefresh}>{#if refreshing}{@render spinner()}Checking…{:else}<Icon name="refresh" size={14} />Check again{/if}</button>
        <button onclick={onSettings}>Set path in Settings</button>
      </div>
    </div>
  {:else if authState.kind === "offline"}
    {@render header("globe", "warn", false, "Can’t reach Dev Tunnels", "Tunnel Bar couldn’t check your sign-in. Check your internet connection. Running connections keep running.")}
    <div class="auth-column">
      <div class="auth-stack">
        <button class="primary" disabled={refreshing} onclick={onRefresh}>{#if refreshing}{@render spinner()}Retrying…{:else}<Icon name="refresh" size={14} />Retry now{/if}</button>
      </div>
      <span class="auth-note">{#if refreshing}<span role="status">Contacting Dev Tunnels…</span>{:else if offlineLeft > 0}Retrying automatically in <span aria-hidden="true">{offlineLeft}s</span><span class="sr-only">soon</span>{:else}Retrying…{/if}</span>
    </div>
  {:else if authState.kind === "error"}
    {@render header("warning", "bad", false, "Couldn’t check your sign-in", "devtunnel stopped with an unexpected error.")}
    <div class="auth-column">
      <details class="auth-card auth-details"><summary>Details</summary><pre class="selectable">{authState.message}</pre></details>
      <div class="auth-pair">
        <button class="primary" disabled={refreshing} onclick={onRefresh}>{#if refreshing}{@render spinner()}Checking…{:else}<Icon name="refresh" size={14} />Try again{/if}</button>
        <button onclick={onLogs}>View logs</button>
      </div>
    </div>
  {:else if codeExpired}
    {@render header("lock", "warn", false, "That code expired", "Codes are valid for 15 minutes. Get a new one to keep going.")}
    <div class="auth-column">
      <div class="auth-card code-card">
        <span class="code-label">Expired {providerName(codeExpired.provider)} code</span>
        {#if codeExpired.code}<div class="code-box expired"><code>{codeExpired.code}</code></div>{/if}
        <button class="primary" disabled={busy} onclick={() => onStart(codeExpired.provider, true)}><Icon name="refresh" size={14} />Get a new code</button>
      </div>
      <div class="auth-links"><button class="auth-link" onclick={() => (dismissed = failedKey)}>Use a different sign-in method</button></div>
    </div>
  {:else}
    {@const expired = authState.kind === "expired"}
    {@render header("lock", expired ? "warn" : "", false, expired ? "Your sign-in expired" : "Sign in to Dev Tunnels",
      expired ? "Sign in again to manage your tunnels. Running connections keep running." : "Use your Microsoft or GitHub account to view and manage your tunnels.")}
    <div class="auth-column">
      {#if failed && failureText}
        <div class="auth-notice bad" role="alert"><Icon name="warning" size={16} /><span><strong>{failureText.title}</strong>{failureText.body}
          {#if failed.message && failed.reason !== "timedOut"}<details><summary>Details</summary><pre class="selectable">{failed.message}</pre></details>{/if}</span></div>
      {:else if failed?.reason === "cancelled"}
        <div class="auth-notice neutral" role="status"><Icon name="close" size={14} /><span>Sign-in cancelled. Choose an account to try again.</span></div>
      {:else if deviceMode}
        <div class="auth-notice" role="status"><Icon name="laptop" size={16} /><span>You’ll get a one-time code to enter in a browser on any device.</span></div>
      {/if}
      <div class="auth-stack">
        {#each order as p (p)}
          <button class:primary={p === primary} disabled={busy} onclick={() => onStart(p, deviceMode)}>Continue with {providerName(p)}</button>
        {/each}
      </div>
      {#if expired && lastProvider}<span class="auth-note">You last signed in with {providerName(lastProvider)}.</span>{/if}
      {@render links(busy)}
    </div>
  {/if}
</div>

<style>
  /* Shared (global) layout for every auth and empty state: one centered column: badge, title, body, then content/actions/links at one width. */
  :global(.auth-screen) { --auth-width: 272px; display: flex; flex-direction: column; align-items: center; text-align: center; padding: 30px 0 16px; }
  :global(.auth-badge) { width: 60px; height: 60px; border-radius: 50%; display: grid; place-items: center; position: relative; flex: none; background: var(--wash); color: var(--accent); }
  :global(.auth-badge.warn) { background: var(--warn-bg); color: var(--warn); }
  :global(.auth-badge.bad) { background: var(--bad-bg); color: var(--bad); }
  /* The ring is an SVG laid exactly over the badge's centre, so it stays concentric while it spins. */
  .auth-orbit { position: absolute; left: 50%; top: 50%; width: 72px; height: 72px; margin: -36px 0 0 -36px; overflow: visible; animation: auth-rotate 1.1s cubic-bezier(.5, .15, .5, .85) infinite; }
  .auth-orbit circle { fill: none; stroke-width: 2; }
  .auth-orbit .track { stroke: color-mix(in srgb, var(--accent) 14%, transparent); }
  .auth-orbit .arc { stroke: var(--accent); stroke-linecap: round; stroke-dasharray: 26 74; }
  :global(.auth-screen h2) { font-size: 18px; letter-spacing: -.35px; line-height: 1.3; margin: 16px 0 0; max-width: var(--auth-width); }
  :global(.auth-body) { color: var(--muted); line-height: 1.55; margin: 6px 0 0; max-width: var(--auth-width); }
  :global(.auth-column) { width: 100%; max-width: var(--auth-width); display: flex; flex-direction: column; align-items: stretch; gap: 10px; margin-top: 18px; }
  :global(.auth-column > *) { width: 100%; margin: 0; }
  :global(.auth-stack) { display: flex; flex-direction: column; gap: 8px; }
  :global(.auth-pair) { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  :global(.auth-stack button), :global(.auth-pair button) { width: 100%; height: 36px; font-size: 13px; padding: 0 8px; }
  :global(.auth-pair button) { font-size: 12px; }
  .auth-stack button.waiting { opacity: 1; cursor: default; }
  .auth-stack button.quiet-outline { background: transparent; color: var(--muted); }
  .auth-stack button.quiet-outline:hover:not(:disabled) { color: var(--fg); background: var(--soft); }
  .auth-notice { display: flex; align-items: flex-start; gap: 8px; text-align: left; font-size: 11.5px; line-height: 1.45; color: var(--muted); background: var(--soft); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px; overflow-wrap: anywhere; }
  .auth-notice :global(svg) { margin-top: 1px; }
  .auth-notice.bad { color: var(--bad); background: var(--bad-bg); border-color: transparent; }
  .auth-notice.bad strong { display: block; font-size: 12px; margin-bottom: 2px; }
  .auth-notice.neutral { background: transparent; border-style: dashed; align-items: center; }
  .auth-notice > span { flex: 1; min-width: 0; }
  .auth-notice details, .auth-details { margin-top: 6px; font-size: 11px; }
  details summary { cursor: pointer; color: inherit; opacity: .85; }
  details pre { margin: 6px 0 0; padding: 8px; background: var(--soft); border-radius: 6px; font: 11px/1.45 ui-monospace, Menlo, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; color: var(--fg); text-align: left; }
  .auth-card { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; }
  .auth-details { text-align: left; padding: 8px 10px; margin: 0; }
  .auth-details summary { color: var(--muted); }
  .code-card { padding: 12px; display: flex; flex-direction: column; align-items: stretch; gap: 10px; }
  .code-label { font-size: 11px; color: var(--muted); text-align: left; }
  .code-box { display: flex; align-items: center; justify-content: space-between; gap: 8px; min-height: 44px; background: var(--soft); border-radius: 8px; padding: 4px 6px 4px 12px; }
  .code-box code { font: 600 20px/1.2 ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace; letter-spacing: 2px; }
  .code-box.expired code { color: var(--muted); text-decoration: line-through; text-decoration-thickness: 1.5px; }
  .code-card button.primary { height: 36px; font-size: 13px; }
  .command-box { display: flex; align-items: center; gap: 6px; background: var(--soft); padding: 4px 4px 4px 10px; min-height: 36px; }
  .command-box code { flex: 1; min-width: 0; text-align: left; font: 11.5px ui-monospace, Menlo, Consolas, monospace; overflow-wrap: anywhere; }
  .command-box button, .code-box button { width: 28px; height: 28px; min-height: 28px; }
  .auth-wait, :global(.auth-note) { display: flex; align-items: center; justify-content: center; gap: 7px; font-size: 11px; color: var(--muted); }
  .countdown { font-variant-numeric: tabular-nums; }
  :global(.auth-links) { display: flex; justify-content: center; flex-wrap: wrap; gap: 4px 18px; margin-top: 2px; }
  :global(.auth-links button.auth-link) { min-height: 22px; height: 22px; padding: 0 2px; border: none; border-radius: 3px; background: transparent; color: var(--accent); font-size: 11.5px; }
  :global(.auth-links button.auth-link:hover:not(:disabled)) { background: transparent; text-decoration: underline; }
  :global(.auth-links button.auth-link:disabled) { color: var(--muted); }
  .auth-spin { width: 12px; height: 12px; border-radius: 50%; border: 1.6px solid currentColor; border-right-color: transparent; display: inline-block; flex: none; animation: auth-rotate .8s linear infinite; }
  @keyframes auth-rotate { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) {
    .auth-orbit { animation: none; }
    .auth-orbit .arc { stroke-dasharray: none; stroke: color-mix(in srgb, var(--accent) 45%, transparent); }
  }
</style>
