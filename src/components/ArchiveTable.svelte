<script lang="ts">
  import DotIcon from './DotIcon.svelte';
  import { getVirtualRange } from '../lib/virtualization.js';

  interface ArchiveEntry {
    path: string;
    name: string;
    parent_path: string;
    is_directory: boolean;
    uncompressed_size: number;
    compressed_size: number | null;
    modified_at: string | null;
    method?: string | null;
  }

  // Precomputed filtered+sorted rows from +page (single query pipeline).
  // Table only virtualizes and handles selection — no second filter/sort.
  let {
    visibleEntries = [] as ArchiveEntry[],
    archiveMode = false,
    currentInternalPath = '/',
    selectedPaths = new Set<string>(),
    query = '',
    typeFilter = 'all' as 'all' | 'files' | 'folders',
    extension = '',
    sortKey = 'name',
    sortDir = 'asc' as 'asc' | 'desc',
    onSortChange,
    onNavigate,
    onSelectionChange
  } = $props<{
    visibleEntries: ArchiveEntry[];
    archiveMode?: boolean;
    currentInternalPath?: string;
    selectedPaths: Set<string>;
    query?: string;
    typeFilter?: 'all' | 'files' | 'folders';
    extension?: string;
    sortKey?: string;
    sortDir?: 'asc' | 'desc';
    onSortChange: (key: string, dir: 'asc' | 'desc') => void;
    onNavigate: (path: string) => void;
    onSelectionChange: (paths: Set<string>) => void;
  }>();

  let focusedIndex = $state(-1);
  let anchorIndex = $state(-1);
  let container: HTMLDivElement;
  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  let rowHeight = $state(0);

  // Smaller overscan = fewer DOM nodes on weak GPUs (still smooth with rAF scroll).
  const OVERSCAN = 4;

  let range = $derived(getVirtualRange(visibleEntries.length, scrollTop, viewportHeight, rowHeight, OVERSCAN));
  let renderedEntries = $derived(visibleEntries.slice(range.start, range.end));

  let scrollRaf: number | null = null;

  function handleScroll() {
    if (scrollRaf !== null) return;
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = null;
      if (container) scrollTop = container.scrollTop;
    });
  }

  function toggleSort(key: string) {
    if (sortKey === key) {
      onSortChange(key, sortDir === 'asc' ? 'desc' : 'asc');
    } else {
      const dir = key === 'name' ? 'asc' : 'desc';
      onSortChange(key, dir);
    }
  }

  function ariaSortFor(key: string): 'ascending' | 'descending' | 'none' {
    if (sortKey !== key) return 'none';
    return sortDir === 'asc' ? 'ascending' : 'descending';
  }

  $effect(() => {
    if (!container) return;

    const updateViewport = () => {
      scrollTop = container.scrollTop;
      viewportHeight = container.clientHeight;
      const measuredRowHeight = Number.parseFloat(
        getComputedStyle(container).getPropertyValue('--archive-row-height')
      );
      if (Number.isFinite(measuredRowHeight)) rowHeight = measuredRowHeight;
    };

    // Re-measure when archive mode toggles (row height CSS var changes).
    void archiveMode;
    updateViewport();
    const resizeObserver = new ResizeObserver(updateViewport);
    resizeObserver.observe(container);
    window.addEventListener('resize', updateViewport);
    return () => {
      resizeObserver.disconnect();
      window.removeEventListener('resize', updateViewport);
    };
  });

  $effect(() => {
    if (!container || focusedIndex < 0 || visibleEntries.length === 0 || rowHeight <= 0) return;

    const header = container.querySelector('thead');
    const headerHeight = header instanceof HTMLElement ? header.offsetHeight : 0;
    const rowTop = headerHeight + focusedIndex * rowHeight;
    const rowBottom = rowTop + rowHeight;
    const viewportTop = container.scrollTop;
    const viewportBottom = viewportTop + container.clientHeight;

    if (rowTop < viewportTop + headerHeight) {
      container.scrollTo({ top: Math.max(0, rowTop - headerHeight), behavior: 'auto' });
    } else if (rowBottom > viewportBottom) {
      container.scrollTo({ top: rowBottom - container.clientHeight, behavior: 'auto' });
    }
  });

  // Reset focus/anchor when directory or filter/query changes
  $effect(() => {
    void currentInternalPath;
    void query;
    void typeFilter;
    void extension;
    focusedIndex = -1;
    anchorIndex = -1;
  });

  function formatBytes(bytes: number | null): string {
    if (bytes === null || bytes === undefined) return '-';
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  }

  function calculateRatio(uncompressed: number, compressed: number | null): string {
    if (compressed === null || compressed === undefined || uncompressed <= 0) return '-';
    if (compressed >= uncompressed) return '0%';
    const ratio = Math.round((1 - compressed / uncompressed) * 100);
    return `${ratio}%`;
  }

  function handleRowClick(e: MouseEvent, index: number) {
    const entry = visibleEntries[index];
    const pathStr = entry.path as string;
    let newSelection = new Set(selectedPaths);

    if (e.shiftKey && anchorIndex !== -1) {
      const start = Math.min(anchorIndex, index);
      const end = Math.max(anchorIndex, index);
      if (!e.ctrlKey) {
        newSelection.clear();
      }
      for (let i = start; i <= end; i++) {
        newSelection.add(visibleEntries[i].path as string);
      }
    } else if (e.ctrlKey) {
      if (newSelection.has(pathStr)) {
        newSelection.delete(pathStr);
      } else {
        newSelection.add(pathStr);
      }
      anchorIndex = index;
    } else {
      newSelection.clear();
      newSelection.add(pathStr);
      anchorIndex = index;
    }

    focusedIndex = index;
    onSelectionChange(newSelection);
  }

  function handleDoubleClick(index: number) {
    const entry = visibleEntries[index];
    if (entry.is_directory) {
      onNavigate(entry.path as string);
      return;
    }
    if (archiveMode) {
      const parent = (entry.parent_path as string) || '/';
      onNavigate(parent);
      onSelectionChange(new Set([entry.path as string]));
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (visibleEntries.length === 0) return;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusedIndex = Math.min(focusedIndex + 1, visibleEntries.length - 1);
      if (anchorIndex === -1) anchorIndex = focusedIndex;
      
      let newSelection = new Set<string>();
      if (e.shiftKey) {
        const start = Math.min(anchorIndex, focusedIndex);
        const end = Math.max(anchorIndex, focusedIndex);
        for (let i = start; i <= end; i++) {
          newSelection.add(visibleEntries[i].path as string);
        }
      } else {
        newSelection.add(visibleEntries[focusedIndex].path as string);
        anchorIndex = focusedIndex;
      }
      onSelectionChange(newSelection);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      focusedIndex = Math.max(focusedIndex - 1, 0);
      if (anchorIndex === -1) anchorIndex = focusedIndex;
      
      let newSelection = new Set<string>();
      if (e.shiftKey) {
        const start = Math.min(anchorIndex, focusedIndex);
        const end = Math.max(anchorIndex, focusedIndex);
        for (let i = start; i <= end; i++) {
          newSelection.add(visibleEntries[i].path as string);
        }
      } else {
        newSelection.add(visibleEntries[focusedIndex].path as string);
        anchorIndex = focusedIndex;
      }
      onSelectionChange(newSelection);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (focusedIndex !== -1) {
        handleDoubleClick(focusedIndex);
      }
    } else if (e.key === ' ') {
      e.preventDefault();
      if (focusedIndex !== -1) {
        const pathStr = visibleEntries[focusedIndex].path as string;
        let newSelection = new Set(selectedPaths);
        if (newSelection.has(pathStr)) {
          newSelection.delete(pathStr);
        } else {
          newSelection.add(pathStr);
        }
        onSelectionChange(newSelection);
      }
    }
  }
</script>

<div 
  class="archive-table-container monospace"
  class:archive-mode={archiveMode}
  bind:this={container}
  tabindex="0" 
  onkeydown={handleKeyDown}
  onscroll={handleScroll}
  role="grid"
  aria-label="Archive files list"
>
  <table class="archive-table">
    <thead>
      <tr>
        <th style="width: 45%;" aria-sort={ariaSortFor('name')}>
          <button type="button" class="sort-header" onclick={() => toggleSort('name')}>
            Name {#if sortKey === 'name'}{sortDir === 'asc' ? '▲' : '▼'}{/if}
          </button>
        </th>
        <th style="width: 12%;" aria-sort={ariaSortFor('size')}>
          <button type="button" class="sort-header" onclick={() => toggleSort('size')}>
            Size {#if sortKey === 'size'}{sortDir === 'asc' ? '▲' : '▼'}{/if}
          </button>
        </th>
        <th style="width: 12%;" aria-sort={ariaSortFor('compressed')}>
          <button type="button" class="sort-header" onclick={() => toggleSort('compressed')}>
            Compressed {#if sortKey === 'compressed'}{sortDir === 'asc' ? '▲' : '▼'}{/if}
          </button>
        </th>
        <th style="width: 11%;" aria-sort={ariaSortFor('ratio')}>
          <button type="button" class="sort-header" onclick={() => toggleSort('ratio')}>
            Ratio {#if sortKey === 'ratio'}{sortDir === 'asc' ? '▲' : '▼'}{/if}
          </button>
        </th>
        <th style="width: 20%;" aria-sort={ariaSortFor('modified')}>
          <button type="button" class="sort-header" onclick={() => toggleSort('modified')}>
            Modified {#if sortKey === 'modified'}{sortDir === 'asc' ? '▲' : '▼'}{/if}
          </button>
        </th>
      </tr>
    </thead>
    <tbody>
      {#if visibleEntries.length === 0}
        <tr>
          <td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">
            {archiveMode ? 'No matching entries.' : 'This folder is empty.'}
          </td>
        </tr>
      {:else}
        <tr class="virtual-spacer" aria-hidden="true">
          <td colspan="5" style={`height: ${range.top}px;`}></td>
        </tr>
        {#each renderedEntries as entry, renderedIndex (entry.path)}
          {@const index = range.start + renderedIndex}
          <tr
            class="data-row"
            class:selected={selectedPaths.has(entry.path as string)}
            class:focused={focusedIndex === index}
            onclick={(e) => handleRowClick(e, index)}
            ondblclick={() => handleDoubleClick(index)}
          >
            <td>
              <div class="name-cell-inner">
                <span class="icon-span">
                  <DotIcon isDir={entry.is_directory} name={entry.name as string} size={22} />
                </span>
                <div class="name-stack">
                  <span class="name-text">{entry.name}</span>
                  {#if archiveMode}
                    <span class="path-text">{entry.path}</span>
                  {/if}
                </div>
              </div>
            </td>
            <td>
              {entry.is_directory ? '<DIR>' : formatBytes(entry.uncompressed_size)}
            </td>
            <td>
              {entry.is_directory ? '-' : formatBytes(entry.compressed_size)}
            </td>
            <td>
              {entry.is_directory ? '-' : calculateRatio(entry.uncompressed_size, entry.compressed_size)}
            </td>
            <td>
              {entry.modified_at || '-'}
            </td>
          </tr>
        {/each}
        <tr class="virtual-spacer" aria-hidden="true">
          <td colspan="5" style={`height: ${range.bottom}px;`}></td>
        </tr>
      {/if}
    </tbody>
  </table>
</div>

<style>
  .name-cell-inner {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .icon-span {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    flex-shrink: 0;
  }
  .name-stack {
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
  }
  .name-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 10px;
    color: var(--text-muted);
    line-height: 1.2;
  }
  tr.focused {
    outline: 1.5px dotted var(--pastel-rose) !important;
    outline-offset: -1.5px;
  }
</style>
