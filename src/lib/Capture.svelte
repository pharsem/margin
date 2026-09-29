<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, tick } from "svelte";
  import { api, type Parsed, type Suggestion } from "./api";
  import { clock, isTomorrow, relative } from "./time";

  const WIDTH = 560;
  const BASE_HEIGHT = 96;
  const ROW_HEIGHT = 34;
  const SOURCE_LABEL = { page: "Tab", window: "Window", clipboard: "Clipboard" } as const;

  let text = $state("");
  let parsed = $state<Parsed>({ title: "", due_at: null });
  let error = $state("");
  let input: HTMLInputElement | undefined = $state();
  let generation = 0;
  let suggestions = $state<Suggestion[]>([]);
  /** -1 is the input itself. */
  let highlight = $state(-1);
  /** What the user typed before moving into the suggestions. */
  let typed = "";

  const chosen = $derived(highlight >= 0 ? suggestions[highlight] : undefined);
  const chip = $derived(chosen?.url && typed.trim() ? chosen.text : "");

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
    else input?.setSelectionRange(text.length, text.length);
  }

  function resize(count: number) {
    const height = BASE_HEIGHT + (count > 0 ? 8 + count * ROW_HEIGHT : 0);
    getCurrentWindow().setSize(new LogicalSize(WIDTH, height)).catch(() => {});
  }

  function choose(index: number) {
    if (highlight < 0) typed = text;
    highlight = index;
    if (index < 0) {
      text = typed;
    } else if (!typed.trim()) {
      text = suggestions[index].text;
    }
    update();
    focusInput(false);
  }

  async function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown" && suggestions.length > 0) {
      e.preventDefault();
      choose(Math.min(highlight + 1, suggestions.length - 1));
    } else if (e.key === "ArrowUp" && highlight >= 0) {
      e.preventDefault();
      choose(highlight - 1);
    } else if (e.key === "Escape") {
      e.preventDefault();
      await api.cancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!parsed.title) return;
      try {
        await api.submit(text, chosen?.url ?? null);
      } catch (err) {
        error = String(err);
      }
    }
  }

  onMount(() => {
    const unlisten = [
      listen<{ text: string; generation: number }>("capture-open", async (e) => {
        generation = e.payload.generation;
        text = e.payload.text;
        typed = "";
        highlight = -1;
        suggestions = [];
        resize(0);
        await update();
        focusInput(true);
      }),
      listen<{ generation: number; suggestions: Suggestion[] }>("capture-context", (e) => {
        if (e.payload.generation !== generation) return;
        const previous = chosen;
        suggestions = e.payload.suggestions;
        // A late URL result replaces the list. Keep the highlight only if the same row is still there.
        if (previous && suggestions[highlight]?.text !== previous.text) choose(-1);
        resize(suggestions.length);
      }),
      getCurrentWindow().onFocusChanged(({ payload }) => payload && focusInput(false)),
    ];
    return () => unlisten.forEach((p) => p.then((f) => f()));
  });
</script>

<div class="capture">
  <div class="field">
    <input
      bind:this={input}
      bind:value={text}
      oninput={update}
      onkeydown={onKey}
      placeholder="Follow up on…"
      spellcheck="false"
      autocomplete="off"
    />
    <p class:error={!!error} class:due={parsed.due_at !== null && !error}>
      {#if chip}<span class="chip">🔗 {chip}</span>{/if}
      {hint}
    </p>
  </div>
  {#if suggestions.length > 0}
    <ul>
      {#each suggestions as suggestion, i}
        <li>
          <button class:active={i === highlight} onclick={() => choose(i)}>
            <span class="source">{SOURCE_LABEL[suggestion.source]}</span>
            <span class="text">{suggestion.text}</span>
            {#if suggestion.url}<span class="link">🔗</span>{/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .capture {
    display: flex;
    flex-direction: column;
    height: 100vh;
    box-sizing: border-box;
    border: 1px solid var(--border);
    background: var(--bg-raised);
  }
  .field {
    display: flex;
    flex-direction: column;
    justify-content: center;
    height: 94px;
    flex: none;
    padding: 0 16px;
    box-sizing: border-box;
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
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  p.due {
    color: var(--accent);
  }
  p.error {
    color: var(--danger);
  }
  .chip {
    margin-right: 8px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--bg-selected);
    color: var(--text);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 4px 6px;
    border-top: 1px solid var(--border);
  }
  li button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 34px;
    padding: 0 10px;
    border-radius: 4px;
    text-align: left;
  }
  li button.active,
  li button:hover {
    background: var(--bg-selected);
  }
  li button.active {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .source {
    flex: none;
    width: 64px;
    font-size: 11px;
    color: var(--muted);
  }
  .text {
    flex: 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .link {
    font-size: 12px;
  }
</style>
