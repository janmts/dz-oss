<script lang="ts">
  // Reusable searchable multi-select dropdown (checkbox list). Used for the run
  // filters (car / zone / season) and each graph lane's channel picker. The
  // panel is position:fixed and viewport-anchored to the trigger so it never
  // gets clipped by a scrolling / overflow:hidden container (lane, rail).
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
  let trigger = $state<HTMLButtonElement | null>(null);
  let pos = $state<{
    top: number | null;
    bottom: number | null;
    left: number | null;
    right: number | null;
    minWidth: number;
    maxHeight: number;
  }>({ top: 0, bottom: null, left: 0, right: null, minWidth: 180, maxHeight: 290 });

  let filtered = $derived(
    query.trim()
      ? options.filter((o) => o.label.toLowerCase().includes(query.trim().toLowerCase()))
      : options,
  );

  function place() {
    if (!trigger) return;
    const r = trigger.getBoundingClientRect();
    const minWidth = Math.max(180, r.width);
    const left = align === 'right' ? null : Math.min(r.left, window.innerWidth - minWidth - 8);
    const right = align === 'right' ? Math.max(8, window.innerWidth - r.right) : null;
    const estH = 290;
    const spaceBelow = window.innerHeight - r.bottom - 8;
    const spaceAbove = r.top - 8;
    if (spaceBelow < estH && spaceAbove > spaceBelow) {
      // Not enough room below (e.g. the bottom lane) → flip up, anchoring the
      // panel's bottom just above the trigger, and cap height to the space.
      pos = { top: null, bottom: window.innerHeight - r.top + 4, left, right, minWidth, maxHeight: Math.min(estH, spaceAbove) };
    } else {
      pos = { top: r.bottom + 4, bottom: null, left, right, minWidth, maxHeight: Math.min(estH, spaceBelow) };
    }
  }

  function toggleOpen() {
    if (open) {
      open = false;
    } else {
      place();
      open = true;
    }
  }

  // Keep the panel anchored to the trigger while scrolling/resizing (capture
  // catches inner scroll containers too); pointerdown outside closes it.
  $effect(() => {
    if (!open) return;
    const onMove = () => place();
    window.addEventListener('scroll', onMove, true);
    window.addEventListener('resize', onMove);
    return () => {
      window.removeEventListener('scroll', onMove, true);
      window.removeEventListener('resize', onMove);
    };
  });

  function onWindowPointerDown(e: PointerEvent) {
    if (!open) return;
    const t = e.target as HTMLElement;
    if (trigger?.contains(t) || t.closest?.('.dd-panel')) return;
    open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<button
  bind:this={trigger}
  type="button"
  class="dd-btn"
  class:active={selected.length > 0}
  onclick={toggleOpen}
>
  <span class="dd-label">{label}</span>
  <span class="dd-count mono">{selected.length > 0 ? selected.length : allLabel}</span>
  <span class="dd-caret" class:open>▾</span>
</button>

{#if open}
  <div
    class="dd-panel"
    style:top={pos.top != null ? `${pos.top}px` : ''}
    style:bottom={pos.bottom != null ? `${pos.bottom}px` : ''}
    style:left={pos.left != null ? `${pos.left}px` : ''}
    style:right={pos.right != null ? `${pos.right}px` : ''}
    style:min-width="{pos.minWidth}px"
    style:max-height="{pos.maxHeight}px"
  >
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

<style>
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
    position: fixed;
    z-index: 1100;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
    padding: 5px;
    overflow: hidden;
  }
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
  .dd-list { flex: 1; min-height: 0; overflow-y: auto; }
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
