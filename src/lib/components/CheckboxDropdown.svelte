<script lang="ts">
  // Reusable searchable multi-select dropdown (checkbox list). Used for the run
  // filters (car / zone / season) and each graph lane's channel picker.
  interface Option {
    value: string;
    label: string;
    swatch?: string;
    hint?: string;
  }

  let {
    label,
    options,
    selected,
    onToggle,
    searchable = true,
    allLabel = 'all',
    emptyText = 'no matches',
    align = 'left',
  }: {
    label: string;
    options: Option[];
    selected: string[];
    onToggle: (value: string) => void;
    searchable?: boolean;
    allLabel?: string;
    emptyText?: string;
    align?: 'left' | 'right';
  } = $props();

  let open = $state(false);
  let query = $state('');
  let root = $state<HTMLDivElement | null>(null);

  let filtered = $derived(
    query.trim()
      ? options.filter((o) => o.label.toLowerCase().includes(query.trim().toLowerCase()))
      : options,
  );

  function onWindowPointerDown(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="dd" bind:this={root}>
  <button type="button" class="dd-btn" class:active={selected.length > 0} onclick={() => (open = !open)}>
    <span class="dd-label">{label}</span>
    <span class="dd-count mono">{selected.length > 0 ? selected.length : allLabel}</span>
    <span class="dd-caret" class:open>▾</span>
  </button>

  {#if open}
    <div class="dd-panel" class:right={align === 'right'}>
      {#if searchable}
        <input class="dd-search" placeholder="Filter…" bind:value={query} />
      {/if}
      <div class="dd-list">
        {#each filtered as opt (opt.value)}
          <label class="dd-opt">
            <input
              type="checkbox"
              checked={selected.includes(opt.value)}
              onchange={() => onToggle(opt.value)}
            />
            {#if opt.swatch}<span class="dd-sw" style:background={opt.swatch}></span>{/if}
            <span class="dd-opt-label">{opt.label}</span>
            {#if opt.hint}<span class="dd-opt-hint mono">{opt.hint}</span>{/if}
          </label>
        {/each}
        {#if filtered.length === 0}
          <div class="dd-empty">{emptyText}</div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .dd { position: relative; }
  .dd-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: inherit;
    font-size: 0.7rem;
    color: var(--tx-mid);
    background: var(--bg-card);
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    padding: 0.22rem 0.5rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .dd-btn:hover { border-color: var(--bd-strong); color: var(--tx-hi); }
  .dd-btn.active { border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel)); }
  .dd-count { color: var(--ac); font-size: 0.66rem; }
  .dd-btn:not(.active) .dd-count { color: var(--tx-dim); }
  .dd-caret { color: var(--tx-dim); font-size: 0.6rem; transition: transform 0.15s; }
  .dd-caret.open { transform: rotate(180deg); }

  .dd-panel {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 50;
    width: max(180px, 100%);
    background: var(--bg-panel);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
    padding: 5px;
  }
  .dd-panel.right { left: auto; right: 0; }
  .dd-search {
    width: 100%;
    font-family: inherit;
    font-size: 0.7rem;
    color: var(--tx-mid);
    background: var(--bg-card);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-sm);
    padding: 0.25rem 0.5rem;
    margin-bottom: 5px;
  }
  .dd-list { max-height: 220px; overflow-y: auto; }
  .dd-opt {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 0.72rem;
    color: var(--tx-mid);
    padding: 0.22rem 0.32rem;
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .dd-opt:hover { background: var(--bg-elevated); }
  .dd-opt input { accent-color: var(--ac); cursor: pointer; }
  .dd-sw { width: 9px; height: 9px; border-radius: 2px; flex: none; }
  .dd-opt-label { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .dd-opt-hint { color: var(--tx-dim); font-size: 0.64rem; }
  .dd-empty { font-size: 0.7rem; color: var(--tx-dim); padding: 0.3rem 0.4rem; }
</style>
