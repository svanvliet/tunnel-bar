<script lang="ts">
  export type IconName = "plus" | "refresh" | "settings" | "more" | "copy" | "terminal" | "arrow" | "back" | "lock" | "globe" | "warning" | "laptop" | "server" | "inspect" | "check" | "close";
  let { name, size = 16 }: { name: IconName; size?: number } = $props();
  const paths: Record<IconName, string> = {
    plus: "M12 5v14M5 12h14",
    refresh: "M20 8a8 8 0 1 0 0 8M20 3v5h-5",
    settings: "m9 3-1 3-3 1 1 3-2 2 2 2-1 3 3 1 1 3h6l1-3 3-1-1-3 2-2-2-2 1-3-3-1-1-3ZM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
    more: "M5 12h.01M12 12h.01M19 12h.01",
    copy: "M8 8h12v13H8ZM15 8V3H3v13h5",
    terminal: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2Zm2 5 3 3-3 3m6 0h4",
    arrow: "M4 12h16m-5-5 5 5-5 5",
    back: "m15 5-7 7 7 7",
    lock: "M7 10h10a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2Zm1 0V7a4 4 0 0 1 8 0v3",
    globe: "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0ZM3 12h18m-9-9c-5 4-5 14 0 18 5-4 5-14 0-18",
    warning: "m12 3 10 18H2ZM12 9v5m0 3v.01",
    laptop: "M7 4h10a2 2 0 0 1 2 2v10H5V6a2 2 0 0 1 2-2ZM2 20h20l-3-4H5Z",
    server: "M4 3h16v8H4Zm0 10h16v8H4ZM7 7h1m-1 10h1",
    inspect: "M16 10a6 6 0 1 1-12 0 6 6 0 0 1 12 0Zm-1 5 6 6",
    check: "m5 12 4 4L19 6",
    close: "m6 6 12 12M18 6 6 18",
  };
</script>

<svg class:copy-check={name === "check"} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width={name === "more" ? 3 : 1.6} stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  <path d={paths[name]} />
</svg>
