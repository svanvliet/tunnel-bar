<script lang="ts">
  import type { LoginProvider, LoginState } from "./types";
  let { login, loggedIn, account, busy, onStart, onCancel, onLogout, onFallback, onCopy, onOpen }: {
    login?: LoginState | null; loggedIn: boolean; account: string; busy: boolean;
    onStart: (provider: LoginProvider, deviceCode: boolean) => Promise<void>;
    onCancel: () => Promise<void>; onLogout: () => Promise<boolean>; onFallback: () => Promise<void>;
    onCopy: (code: string) => Promise<void>; onOpen: (url: string) => Promise<void>;
  } = $props();
  let deviceCode = $state(false);
  let confirmLogout = $state(false);
  const running = $derived(login?.kind === "running" || login?.kind === "deviceCode");
</script>

<div class="login-panel">
  <p class="account-label selectable">{loggedIn ? account : "Choose an account to manage your tunnels."}</p>
  {#if login?.kind === "running"}
    <div class="inline-progress" role="status">Waiting for {login.provider === "github" ? "GitHub" : "Microsoft"} sign-in…</div>
  {:else if login?.kind === "deviceCode"}
    <div class="device-code" role="status">
      <strong>Enter this code in your browser</strong>
      <div class="button-row"><code class="selectable">{login.code}</code><button disabled={busy} onclick={() => login?.kind === "deviceCode" && onCopy(login.code)}>Copy code</button></div>
      <button class="primary" disabled={busy} onclick={() => login?.kind === "deviceCode" && onOpen(login.url)}>Open sign-in page</button>
    </div>
  {:else if login?.kind === "failed"}
    <p class="inline-warning error-text" role="alert">{login.message}</p>
  {:else if login?.kind === "succeeded"}
    <p class="inline-progress" role="status">{login.message}</p>
  {/if}
  {#if running}
    <button disabled={busy} onclick={onCancel}>Cancel sign-in</button>
  {:else}
    <div class="button-row">
      <button class="primary" disabled={busy || confirmLogout} onclick={() => onStart("microsoft", deviceCode)}>Microsoft</button>
      <button disabled={busy || confirmLogout} onclick={() => onStart("github", deviceCode)}>GitHub</button>
      {#if loggedIn}<button class="quiet" disabled={busy} onclick={() => (confirmLogout = true)}>Sign out…</button>{/if}
    </div>
    <label class="auto-toggle login-mode"><input type="checkbox" bind:checked={deviceCode} disabled={busy} />Use a device code</label>
    <button class="text-button" disabled={busy || confirmLogout} onclick={onFallback}>Use terminal sign-in instead</button>
  {/if}
  {#if confirmLogout}
    <div class="confirmation">
      <strong>Sign out of Dev Tunnels?</strong><p>Your running connections keep running. The account tunnel list will be cleared.</p>
      <div class="button-row"><button disabled={busy} onclick={() => (confirmLogout = false)}>Cancel</button>
        <button class="danger" disabled={busy} onclick={async () => { if (await onLogout()) confirmLogout = false; }}>Sign out</button></div>
    </div>
  {/if}
</div>
