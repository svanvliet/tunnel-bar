<script lang="ts">
  import Icon from "./Icon.svelte";
  let { status, tone = "busy" }: { status: string; tone?: "busy" | "ok" } = $props();
</script>

<div class="skeleton-list" aria-busy="true" aria-label={status}>
  <div class="skeleton-status" class:ok={tone === "ok"} role="status">
    {#if tone === "ok"}<Icon name="check" size={14} />{:else}<span class="skeleton-spin" aria-hidden="true"></span>{/if}{status}
  </div>
  <div class="skeleton-heading"><i class="shimmer"></i></div>
  {#each [0, 1, 2] as i (i)}
    <div class="skeleton-card" aria-hidden="true">
      <i class="square shimmer"></i><i class="line-1 shimmer"></i><i class="line-2 shimmer"></i>
      <div class="row"><i class="wide shimmer"></i><i class="shimmer"></i><i class="shimmer"></i></div>
    </div>
  {/each}
</div>

<style>
  .skeleton-list { padding-top: 2px; }
  .skeleton-status { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--muted); padding: 2px 2px 10px; }
  .skeleton-status.ok { color: var(--ok); }
  .skeleton-spin { width: 12px; height: 12px; border-radius: 50%; border: 1.6px solid var(--accent); border-right-color: transparent; flex: none; animation: skeleton-rotate .8s linear infinite; }
  .skeleton-heading { margin: 0 0 6px; }
  .skeleton-heading i { display: block; height: 10px; width: 110px; }
  .skeleton-card { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 10px; margin-bottom: 8px; display: grid; grid-template-columns: 30px 1fr; gap: 6px 9px; }
  .shimmer { display: block; border-radius: 6px; background: linear-gradient(100deg, var(--soft) 30%, color-mix(in srgb, var(--line) 70%, var(--surface)) 50%, var(--soft) 70%); background-size: 300% 100%; animation: skeleton-shimmer 1.5s ease-in-out infinite; }
  .square { width: 30px; height: 30px; border-radius: 8px; grid-row: span 2; }
  .line-1 { height: 11px; width: 55%; margin-top: 3px; }
  .line-2 { height: 9px; width: 35%; }
  .row { grid-column: 1 / -1; display: flex; gap: 6px; margin-top: 6px; }
  .row i { height: 24px; width: 64px; border-radius: 7px; }
  .row i.wide { width: 120px; }
  @keyframes skeleton-shimmer { from { background-position: 100% 0; } to { background-position: 0 0; } }
  @keyframes skeleton-rotate { to { transform: rotate(360deg); } }
</style>
