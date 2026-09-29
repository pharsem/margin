<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Panel from "$lib/Panel.svelte";
  import Capture from "$lib/Capture.svelte";

  const label = getCurrentWindow().label;
</script>

{#if label === "capture"}
  <Capture />
{:else}
  <Panel />
{/if}

<style>
  :global(:root) {
    --bg: #1e1f22;
    --bg-raised: #2b2d31;
    --bg-selected: #35373c;
    --border: #3a3b3f;
    --text: #e6e6e6;
    --muted: #9a9ca3;
    --accent: #7aa2f7;
    --danger: #f7768e;
    --danger-bg: #3b2229;
    --warn: #e0af68;
    --warn-bg: #3a3222;
    color-scheme: dark;
  }
  :global(html, body) {
    margin: 0;
    height: 100%;
    overflow: hidden;
    background: var(--bg);
    color: var(--text);
    font-family: "Segoe UI Variable", "Segoe UI", sans-serif;
    font-size: 14px;
    user-select: none;
  }
  :global(button) {
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
    cursor: pointer;
  }
</style>
