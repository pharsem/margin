<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, tick } from "svelte";
  import { api, type Parsed } from "./api";
  import { clock, isTomorrow, relative } from "./time";

  let text = $state("");
  let parsed = $state<Parsed>({ title: "", due_at: null });
  let error = $state("");
  let input: HTMLInputElement | undefined = $state();

  const hint = $derived.by(() => {
    if (error) return error;
    if (!parsed.title) return text.trim() ? "Add a title before the time" : "Type a line, optionally ending in 30m, 2h, 1h30m or 14:30";
    if (parsed.due_at === null) return "No timer";
    const now = Date.now();
    const day = isTomorrow(parsed.due_at, now) ? " tomorrow" : "";
    return `Due ${clock(parsed.due_at)}${day} (${relative(parsed.due_at, now)})`;
  });

  async function update() {
    error = "";
    parsed = await api.parse(text);
  }

  async function focusInput(select: boolean) {
    await tick();
    input?.focus();
    if (select) input?.select();
  }

  async function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      await api.cancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!parsed.title) return;
      try {
        await api.submit(text);
      } catch (err) {
        error = String(err);
      }
    }
  }

  onMount(() => {
    const unlisten = [
      listen<string>("capture-open", async (e) => {
        text = e.payload;
        await update();
        focusInput(true);
      }),
      getCurrentWindow().onFocusChanged(({ payload }) => payload && focusInput(false)),
    ];
    return () => unlisten.forEach((p) => p.then((f) => f()));
  });
</script>

<div class="capture">
  <input
    bind:this={input}
    bind:value={text}
    oninput={update}
    onkeydown={onKey}
    placeholder="Follow up on…"
    spellcheck="false"
    autocomplete="off"
  />
  <p class:error={!!error} class:due={parsed.due_at !== null && !error}>{hint}</p>
</div>

<style>
  .capture {
    display: flex;
    flex-direction: column;
    justify-content: center;
    height: 100vh;
    padding: 0 16px;
    box-sizing: border-box;
    border: 1px solid var(--border);
    background: var(--bg-raised);
  }
  input {
    font: inherit;
    font-size: 18px;
    color: var(--text);
    background: transparent;
    border: 0;
    outline: none;
    padding: 4px 0;
  }
  p {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--muted);
  }
  p.due {
    color: var(--accent);
  }
  p.error {
    color: var(--danger);
  }
</style>
