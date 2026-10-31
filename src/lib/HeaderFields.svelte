<script lang="ts">
  import { HEADER_UNCHANGED } from "./types";
  let { hostHeader = $bindable(""), originHeader = $bindable(""), disabled = false }: {
    hostHeader?: string; originHeader?: string; disabled?: boolean;
  } = $props();
</script>

<div class="header-fields">
  <div class="button-row"><strong class="field-title">Host / Origin rewriting</strong><span class="spacer"></span>
    <button type="button" class="quiet" {disabled} onclick={() => { hostHeader = HEADER_UNCHANGED; originHeader = HEADER_UNCHANGED; }}>Dev server preset</button>
  </div>
  <label class="field"><span>Host header</span><input name="hostHeader" data-header="host" bind:value={hostHeader} placeholder="Default / leave unchanged" {disabled} /></label>
  <label class="field"><span>Origin header</span><input name="originHeader" data-header="origin" bind:value={originHeader} placeholder="Default / leave unchanged" {disabled} /></label>
  <p class="field-hint">Use <code>unchanged</code> to pass the browser's header through, or enter a literal value without spaces. Empty fields leave existing rewriting alone.</p>
  <p class="field-hint">The Vite / Next.js preset keeps the public hostname. Vite also needs <code>.devtunnels.ms</code> in <code>server.allowedHosts</code>.</p>
</div>
