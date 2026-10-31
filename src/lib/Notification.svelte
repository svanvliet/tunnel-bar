<script lang="ts">
  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import Icon from "./Icon.svelte";
  import { motionDuration } from "./motion";

  export interface Notice {
    id: number;
    kind: "success" | "error";
    title: string;
    detail: string;
    retry?: () => Promise<void>;
  }
  let { notice, reduced, onDismiss }: { notice: Notice; reduced: boolean; onDismiss: () => void } = $props();
  let hovered = $state(false);
  let focused = $state(false);
  let hidden = $state(false);
  let retrying = $state(false);
  let root = $state<HTMLDivElement>();
  let noticeId = 0;
  let remaining = 2600;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let started = 0;

  function pause() {
    if (timer !== undefined) {
      clearTimeout(timer);
      remaining -= Date.now() - started;
      timer = undefined;
    }
  }
  function resume() {
    if (notice.kind !== "success" || hovered || focused || hidden || timer !== undefined) return;
    started = Date.now();
    timer = setTimeout(onDismiss, Math.max(0, remaining));
  }
  $effect(() => {
    if (notice.id !== noticeId) {
      pause(); remaining = 2600; noticeId = notice.id;
      focused = !!root?.contains(document.activeElement);
    }
    if (notice.kind === "error" || hovered || focused || hidden) pause(); else resume();
  });
  onMount(() => {
    hidden = document.hidden;
    const visibility = () => { hidden = document.hidden; };
    document.addEventListener("visibilitychange", visibility);
    return () => { pause(); document.removeEventListener("visibilitychange", visibility); };
  });
  async function retry() {
    if (!notice.retry || retrying) return;
    retrying = true;
    await notice.retry();
    retrying = false;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="notification" bind:this={root} class:notification-error={notice.kind === "error"}
  in:fly={{ y: reduced ? 0 : 6, duration: motionDuration(reduced, 180) }}
  out:fly={{ y: reduced ? 0 : 4, duration: motionDuration(reduced, 120) }}
  onmouseenter={() => (hovered = true)} onmouseleave={() => (hovered = false)}
  onfocusin={() => (focused = true)} onfocusout={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) focused = false; }}>
  <span class="notification-mark"><Icon name={notice.kind === "error" ? "warning" : "check"} /></span>
  <div class="notification-copy" role={notice.kind === "error" ? "alert" : "status"} aria-atomic="true">
    <strong>{notice.title}</strong>
    <span class:selectable={notice.kind === "error"} title={notice.detail}>{notice.detail}</span>
  </div>
  {#if notice.retry}<button class="quiet" disabled={retrying} onclick={retry}>{retrying ? "Retrying…" : "Retry"}</button>{/if}
  <button class="icon-button" title="Dismiss notification" aria-label="Dismiss notification" disabled={retrying} onclick={onDismiss}><Icon name="close" size={14} /></button>
</div>
