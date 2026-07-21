<script lang="ts">
  import { onMount } from 'svelte';

  type TypeFilter = 'all' | 'files' | 'folders';

  let {
    queryInput = '',
    typeFilter = 'all' as TypeFilter,
    extension = '',
    matchCount = null as number | null,
    onQueryInput,
    onTypeFilter,
    onExtension,
    onClear
  } = $props<{
    queryInput?: string;
    typeFilter?: TypeFilter;
    extension?: string;
    matchCount?: number | null;
    onQueryInput: (value: string) => void;
    onTypeFilter: (value: TypeFilter) => void;
    onExtension: (value: string) => void;
    onClear: () => void;
  }>();

  let typeMenuOpen = $state(false);
  let typeSplitEl: HTMLDivElement | undefined = $state();
  let typeTriggerEl: HTMLButtonElement | undefined = $state();

  function typeLabel(value: TypeFilter): string {
    if (value === 'files') return 'Files';
    if (value === 'folders') return 'Folders';
    return 'All';
  }

  function toggleTypeMenu() {
    typeMenuOpen = !typeMenuOpen;
  }

  function closeTypeMenu(restoreFocus = false) {
    if (!typeMenuOpen) return;
    typeMenuOpen = false;
    if (restoreFocus) {
      queueMicrotask(() => typeTriggerEl?.focus());
    }
  }

  function pickType(value: TypeFilter) {
    typeMenuOpen = false;
    onTypeFilter(value);
  }

  onMount(() => {
    function onPointerDown(event: PointerEvent) {
      if (!typeMenuOpen || !typeSplitEl) return;
      const target = event.target as Node | null;
      if (target && !typeSplitEl.contains(target)) {
        closeTypeMenu(false);
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape' && typeMenuOpen) {
        closeTypeMenu(true);
      }
    }

    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  });
</script>

<div class="archive-search-bar monospace" role="search">
  <input
    type="search"
    class="archive-search-input"
    placeholder="Search archive…"
    aria-label="Search archive"
    value={queryInput}
    oninput={(e) => onQueryInput(e.currentTarget.value)}
  />
  <div class="archive-search-label">
    <span id="type-filter-label">Type</span>
    <div class="type-filter-split" bind:this={typeSplitEl}>
      <button
        type="button"
        class="type-filter-trigger"
        bind:this={typeTriggerEl}
        onclick={toggleTypeMenu}
        aria-labelledby="type-filter-label"
        aria-label="Entry type filter"
        aria-haspopup="menu"
        aria-expanded={typeMenuOpen}
        title="Filter by entry type"
      >
        {typeLabel(typeFilter)}
        <span class="type-filter-caret" aria-hidden="true">▾</span>
      </button>
      {#if typeMenuOpen}
        <div class="extract-menu type-filter-menu monospace" role="menu" aria-label="Entry type">
          <button
            type="button"
            role="menuitemradio"
            aria-checked={typeFilter === 'all'}
            class:menu-item-active={typeFilter === 'all'}
            onclick={() => pickType('all')}
          >
            All
          </button>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={typeFilter === 'files'}
            class:menu-item-active={typeFilter === 'files'}
            onclick={() => pickType('files')}
          >
            Files
          </button>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={typeFilter === 'folders'}
            class:menu-item-active={typeFilter === 'folders'}
            onclick={() => pickType('folders')}
          >
            Folders
          </button>
        </div>
      {/if}
    </div>
  </div>
  <label class="archive-search-label">
    Ext
    <input
      type="text"
      class="archive-search-ext"
      placeholder="e.g. png"
      aria-label="Extension filter"
      value={extension}
      oninput={(e) => onExtension(e.currentTarget.value)}
    />
  </label>
  <button type="button" class="archive-search-clear" onclick={onClear} title="Clear search and filters">
    Clear
  </button>
  {#if matchCount !== null}
    <span class="archive-search-count" aria-live="polite">{matchCount} matches</span>
  {/if}
</div>
