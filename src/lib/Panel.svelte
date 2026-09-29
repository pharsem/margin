<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, tick } from "svelte";
  import { api, type Item, type Session, type Snapshot } from "./api";
  import { clock, relative, span } from "./time";

  type Row = { key: string; session?: Session; item?: Item };

  const SNOOZE = [10, 30, 120];

  let snap = $state<Snapshot>({
    sessions: [],
    open: [],
    done_today: [],
    collapsed: false,
    errors: [],
    hotkey: "",
    focus_hotkey: "",
  });
  let focused = $state(false);
  let notice = $state("");
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  function showNotice(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ""), 5000);
  }

  function openSession(session: Session) {
    selectedKey = `s:${session.id}`;
    api.focusSession(session.id).catch((e) => showNotice(String(e)));
  }
  let now = $state(Date.now());
  let selectedKey = $state<string | null>(null);
  let showDone = $state(false);
  let edge = $state<"left" | "right">("right");
  let list: HTMLElement | undefined = $state();

  const overdue = $derived(snap.open.filter((i) => isOverdue(i, now)).length);
  const waiting = $derived(snap.sessions.filter((s) => s.status !== "running").length);
  const rows = $derived(rowsOf(snap));
  const selectedIndex = $derived(rows.findIndex((r) => r.key === selectedKey));

  const STATUS_LABEL = { running: "Running", needs_input: "Needs input", done: "Done" } as const;

  function rowsOf(s: Snapshot): Row[] {
    return [
      ...s.sessions.map((session) => ({ key: `s:${session.id}`, session })),
      ...s.open.map((item) => ({ key: `i:${item.id}`, item })),
    ];
  }

  function idleMs(s: Session) {
    return s.waiting_since === null ? 0 : Math.max(0, now - s.waiting_since);
  }

  function idleLevel(ms: number) {
    const minutes = ms / 60_000;
    return minutes >= 15 ? "alert" : minutes >= 5 ? "warn" : "";
  }

  function idleText(ms: number) {
    const minutes = Math.floor(ms / 60_000);
    return minutes < 1 ? "just now" : span(minutes);
  }

  function isOverdue(item: Item, at: number) {
    return item.due_at !== null && item.due_at <= at;
  }

  function apply(s: Snapshot) {
    const previousIndex = selectedIndex;
    snap = s;
    now = Date.now();
    const next = rowsOf(s);
    if (selectedKey !== null && !next.some((r) => r.key === selectedKey)) {
      const index = Math.min(Math.max(previousIndex, 0), next.length - 1);
      selectedKey = next[index]?.key ?? null;
    }
  }

  async function refresh() {
    apply(await api.snapshot());
  }

  function report(p: Promise<unknown>) {
    p.catch((e) => console.error(e));
  }

  function select(offset: number) {
    if (rows.length === 0) return;
    const start = selectedIndex < 0 ? (offset > 0 ? -1 : rows.length) : selectedIndex;
    const index = Math.min(Math.max(start + offset, 0), rows.length - 1);
    selectedKey = rows[index].key;
    tick().then(() => list?.querySelector(".selected")?.scrollIntoView({ block: "nearest" }));
  }

  function onKey(e: KeyboardEvent) {
    if (snap.collapsed) return;
    const row = rows.find((r) => r.key === selectedKey);
    if (row?.session && e.key === "Enter") {
      openSession(row.session);
      e.preventDefault();
      return;
    }
    if (row?.session && ["d", "r", "Delete"].includes(e.key)) {
      report(api.review(row.session.id));
      e.preventDefault();
      return;
    }
    const id = row?.item?.id ?? null;
    switch (e.key) {
      case "ArrowDown":
        select(1);
        break;
      case "ArrowUp":
        select(-1);
        break;
      case "Escape":
        report(api.releaseFocus());
        break;
      case "Enter":
      case "d":
        if (id !== null) report(api.done(id));
        break;
      case "1":
      case "2":
      case "3":
        if (id !== null) report(api.snooze(id, SNOOZE[Number(e.key) - 1]));
        break;
      case "e":
        if (id !== null) report(api.edit(id));
        break;
      case "Delete":
        if (id !== null) report(api.remove(id));
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  function snoozeLabel(minutes: number) {
    return minutes < 60 ? `${minutes}m` : `${minutes / 60}h`;
  }

  onMount(() => {
    refresh();
    invoke<{ edge: "left" | "right" } | null>("appbar_state").then((s) => s && (edge = s.edge));
    const timers = [
      setInterval(() => (now = Date.now()), 15_000),
      // Picks up the date change at midnight for "Done today".
      setInterval(refresh, 60_000),
    ];
    const unlisten = [
      listen<Snapshot>("snapshot", (e) => apply(e.payload)),
      listen("panel-focus", () => {
        if (selectedKey === null) select(1);
        list?.focus();
      }),
      getCurrentWindow().onFocusChanged(({ payload }) => (focused = payload)),
    ];
    return () => {
      timers.forEach(clearInterval);
      unlisten.forEach((p) => p.then((f) => f()));
    };
  });
</script>

<svelte:window onkeydown={onKey} />

{#if snap.collapsed}
  <button class="strip" class:focused onclick={() => api.setCollapsed(false)} title="Expand">
    <span class="icon">{edge === "right" ? "" : ""}</span>
    {#if waiting > 0}
      <span class="badge warn" title="Sessions waiting">{waiting}</span>
    {/if}
    {#if overdue > 0}
      <span class="badge danger">{overdue}</span>
    {/if}
    <span class="badge">{snap.open.length}</span>
  </button>
{:else}
  <main class:left={edge === "left"} class:focused>
    <header>
      <h1>Now <span class="count">{snap.open.length}</span></h1>
      <button class="icon" title="Add ({snap.hotkey})" onclick={() => report(api.openCapture())}>{""}</button>
      <button class="icon" title="Collapse" onclick={() => api.setCollapsed(true)}>
        {edge === "right" ? "" : ""}
      </button>
    </header>

    {#each snap.errors as error}
      <p class="error">{error}</p>
    {/each}
    {#if notice}
      <p class="error">{notice}</p>
    {/if}

    <div class="scroll" bind:this={list} tabindex="-1">
      {#if snap.sessions.length > 0}
        <h2>Sessions</h2>
        <ul>
          {#each snap.sessions as session (session.id)}
            {@const idle = idleMs(session)}
            <li
              class="session s-{session.status} {session.status === 'running' ? '' : idleLevel(idle)}"
              class:selected={`s:${session.id}` === selectedKey}
            >
              <button class="row" onclick={() => openSession(session)} title="Open the session window ({session.cwd})">
                <span class="dot"></span>
                <span class="title">
                  <strong>{session.folder}</strong>
                  {#if session.title ?? session.prompt}<span class="prompt">{session.title ?? session.prompt}</span>{/if}
                </span>
                <span class="due">
                  {STATUS_LABEL[session.status]}
                  {#if session.status !== "running"}<br />{idleText(idle)}{/if}
                </span>
              </button>
              <div class="actions">
                <button class="icon" title="Reviewed (R)" onclick={() => report(api.review(session.id))}>{"\uE73E"}</button>
              </div>
            </li>
          {/each}
        </ul>
        <h2>Follow-ups</h2>
      {/if}
      <ul>
      {#each snap.open as item (item.id)}
        {@const late = isOverdue(item, now)}
        <li class:overdue={late} class:selected={`i:${item.id}` === selectedKey}>
          <button class="row" onclick={() => (selectedKey = `i:${item.id}`)} ondblclick={() => report(api.edit(item.id))}>
            <span class="title">{item.title}</span>
            {#if item.due_at !== null}
              <span class="due" title={clock(item.due_at)}>{relative(item.due_at, now)}</span>
            {/if}
          </button>
          <div class="actions">
            <button class="icon" title="Done (Enter)" onclick={() => report(api.done(item.id))}>{""}</button>
            {#each SNOOZE as minutes, i}
              <button class="snooze" title="Snooze (key {i + 1})" onclick={() => report(api.snooze(item.id, minutes))}>
                +{snoozeLabel(minutes)}
              </button>
            {/each}
            <button class="icon" title="Edit (E)" onclick={() => report(api.edit(item.id))}>{""}</button>
            <button class="icon" title="Delete (Del)" onclick={() => report(api.remove(item.id))}>{""}</button>
          </div>
        </li>
      {:else}
        <li class="empty">Nothing waiting. {snap.hotkey} to add.</li>
      {/each}
      </ul>
    </div>

    {#if focused}
      <p class="keys">↑↓ select · Enter done or open · 1 2 3 snooze · E edit · Del delete · R reviewed · Esc leave</p>
    {:else if snap.focus_hotkey}
      <p class="keys">{snap.focus_hotkey} to use the keyboard</p>
    {/if}

    {#if snap.done_today.length > 0}
      <footer>
        <button class="done-toggle" onclick={() => (showDone = !showDone)}>
          {showDone ? "" : ""} Done today ({snap.done_today.length})
        </button>
        {#if showDone}
          <ul class="done">
            {#each snap.done_today as item (item.id)}
              <li>
                <button class="row" title="Reopen" onclick={() => report(api.reopen(item.id))}>
                  <span class="title">{item.title}</span>
                  <span class="due">{clock(item.done_at ?? 0)}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </footer>
    {/if}
  </main>
{/if}

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    box-sizing: border-box;
    border-left: 1px solid var(--border);
  }
  main.left {
    border-left: 0;
    border-right: 1px solid var(--border);
  }
  main.focused,
  .strip.focused {
    border-color: var(--accent);
    box-shadow: inset 0 3px 0 var(--accent);
  }
  main.focused h1 {
    color: var(--accent);
  }
  .keys {
    margin: 0;
    padding: 6px 14px;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 11px;
  }
  main.focused .keys {
    color: var(--accent);
  }
  header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 10px 8px 6px 14px;
  }
  h1 {
    flex: 1;
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .count {
    color: var(--muted);
    font-weight: 400;
  }
  .icon {
    font-family: "Segoe Fluent Icons", "Segoe MDL2 Assets";
    font-size: 12px;
    width: 28px;
    height: 28px;
    border-radius: 4px;
  }
  .icon:hover,
  .snooze:hover {
    background: var(--bg-selected);
  }
  .error {
    margin: 4px 12px;
    padding: 6px 8px;
    border-radius: 4px;
    background: var(--danger-bg);
    color: var(--danger);
    font-size: 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0 6px;
    overflow-y: auto;
    outline: none;
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    outline: none;
  }
  h2 {
    margin: 8px 14px 2px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .session .title {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .prompt {
    color: var(--muted);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .session .due {
    text-align: right;
  }
  li.session {
    display: flex;
    align-items: center;
  }
  li.session .row {
    flex: 1;
    min-width: 0;
  }
  li.session .actions {
    padding: 0 6px 0 0;
  }
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    align-self: center;
    background: var(--accent);
  }
  .session.s-needs_input .dot,
  .session.s-done .dot {
    background: var(--warn);
  }
  .session.s-needs_input .due {
    color: var(--warn);
    font-weight: 600;
  }
  .session.warn {
    background: var(--warn-bg);
  }
  .session.alert {
    background: var(--danger-bg);
  }
  .session.alert .dot {
    background: var(--danger);
  }
  .session.alert .due {
    color: var(--danger);
    font-weight: 600;
  }
  li {
    border-radius: 6px;
    margin: 2px 0;
  }
  li.selected {
    background: var(--bg-selected);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  li.overdue {
    background: var(--danger-bg);
  }
  li.overdue.selected {
    box-shadow: inset 2px 0 0 var(--danger);
  }
  .row {
    display: flex;
    width: 100%;
    gap: 8px;
    padding: 8px;
    text-align: left;
    align-items: baseline;
  }
  .title {
    flex: 1;
    overflow-wrap: anywhere;
  }
  .due {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  li.overdue .due {
    color: var(--danger);
    font-weight: 600;
  }
  .actions {
    display: none;
    gap: 2px;
    padding: 0 6px 6px;
  }
  li:hover .actions,
  li.selected .actions {
    display: flex;
  }
  .snooze {
    font-size: 12px;
    padding: 0 6px;
    height: 28px;
    border-radius: 4px;
    color: var(--muted);
  }
  .empty {
    color: var(--muted);
    padding: 12px 8px;
    font-size: 13px;
  }
  footer {
    border-top: 1px solid var(--border);
    padding: 6px 0;
    max-height: 40vh;
    display: flex;
    flex-direction: column;
  }
  .done-toggle {
    text-align: left;
    padding: 4px 14px;
    color: var(--muted);
    font-size: 12px;
  }
  .done .title {
    color: var(--muted);
    text-decoration: line-through;
  }
  .strip {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    width: 100vw;
    height: 100vh;
    padding-top: 12px;
    box-sizing: border-box;
    border-left: 1px solid var(--border);
  }
  .strip .icon {
    height: auto;
  }
  .badge {
    min-width: 24px;
    padding: 2px 6px;
    border-radius: 12px;
    background: var(--bg-raised);
    color: var(--muted);
    font-size: 12px;
    box-sizing: border-box;
  }
  .badge.warn {
    background: var(--warn);
    color: #1e1f22;
    font-weight: 700;
  }
  .badge.danger {
    background: var(--danger);
    color: #1e1f22;
    font-weight: 700;
  }
</style>
