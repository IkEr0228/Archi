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
    canEdit = false,
    onSortChange,
    onNavigate,
    onSelectionChange,
    onMoveEntries
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
    /** When true, rows are draggable into folder rows (in-archive move). */
    canEdit?: boolean;
    onSortChange: (key: string, dir: 'asc' | 'desc') => void;
    onNavigate: (path: string) => void;
    onSelectionChange: (paths: Set<string>) => void;
    /** Move sources into dest folder path (archive-relative; '' or '/' = root). */
    onMoveEntries?: (sources: string[], destFolder: string) => void;
  }>();

  let focusedIndex = $state(-1);
  let anchorIndex = $state(-1);
  /** Folder path currently under an internal pointer drag. */
  let dragOverFolder = $state<string | null>(null);
  /** True after pointer moved past threshold — in-archive move in progress. */
  let isInternalDragging = $state(false);
  let container: HTMLDivElement;
  let scrollTop = $state(0);
  let viewportHeight = $state(400);
  // Default until CSS var is measured — never leave 0 (hides all virtual rows).
  let rowHeight = $state(35);

  // Pointer-based internal DnD (HTML5 DnD is intercepted by Tauri/WebView2 OS file-drop).
  const DRAG_THRESHOLD_PX = 6;
  type PendingDrag = {
    pointerId: number;
    startX: number;
    startY: number;
    index: number;
    path: string;
    sources: string[] | null;
  };
  let pendingDrag: PendingDrag | null = null;
  let activeDragSources: string[] | null = null;
  let suppressClickAfterDrag = false;

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
      const h = container.clientHeight;
      if (h > 0) viewportHeight = h;
      const measuredRowHeight = Number.parseFloat(
        getComputedStyle(container).getPropertyValue('--archive-row-height')
      );
      if (Number.isFinite(measuredRowHeight) && measuredRowHeight > 0) {
        rowHeight = measuredRowHeight;
      } else {
        rowHeight = archiveMode ? 48 : 35;
      }
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

  // Tear down window listeners if the table unmounts mid-drag.
  $effect(() => {
    return () => {
      window.removeEventListener('pointermove', onPointerMoveDuringDrag);
      window.removeEventListener('pointerup', onPointerUpDuringDrag);
      window.removeEventListener('pointercancel', onPointerCancelDuringDrag);
      window.removeEventListener('keydown', onKeyDownDuringDrag);
      clearInternalDrag();
    };
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

  /** Prefer top-level paths when both a folder and its children are selected. */
  function topLevelPaths(paths: string[]): string[] {
    return paths.filter(
      (p) => !paths.some((o) => o !== p && p.startsWith(o + '/'))
    );
  }

  function resolveDragSources(index: number, path: string): string[] {
    if (selectedPaths.has(path) && selectedPaths.size > 0) {
      return topLevelPaths(Array.from(selectedPaths));
    }
    return [path];
  }

  /** Parent of current folder (`/` when one level deep). */
  function parentFolderPath(path: string): string {
    if (!path || path === '/') return '/';
    const parts = path.replace(/\\/g, '/').split('/').filter(Boolean);
    parts.pop();
    return parts.length ? parts.join('/') : '/';
  }

  let parentDropPath = $derived(
    currentInternalPath && currentInternalPath !== '/'
      ? parentFolderPath(currentInternalPath as string)
      : null
  );
  let showParentUpRow = $derived(!!parentDropPath);

  /**
   * Resolve drop folder under the pointer.
   * Supports table folder rows, parent ".." row, breadcrumbs, and Up button
   * via `[data-drop-folder]` (path or `/` for archive root).
   */
  function folderUnderPoint(clientX: number, clientY: number): string | null {
    const el = document.elementFromPoint(clientX, clientY);
    if (!el) return null;
    const dropEl = el.closest('[data-drop-folder]') as HTMLElement | null;
    if (dropEl?.dataset.dropFolder != null && dropEl.dataset.dropFolder !== '') {
      return dropEl.dataset.dropFolder;
    }
    const row = el.closest('tr[data-entry-path]') as HTMLElement | null;
    if (!row || row.dataset.isDir !== 'true') return null;
    return row.dataset.entryPath ?? null;
  }

  function isValidMoveDest(sources: string[], dest: string): boolean {
    // Root (`/` or '') is always a valid destination for move-out-of-folder.
    if (!dest || dest === '/') return true;
    return !sources.some((s) => dest === s || dest.startsWith(s + '/'));
  }

  /** Highlight table rows + external targets (breadcrumbs, Up) sharing data-drop-folder. */
  function setDropHighlight(dest: string | null) {
    dragOverFolder = dest;
    if (typeof document === 'undefined') return;
    for (const node of document.querySelectorAll('[data-drop-folder]')) {
      const el = node as HTMLElement;
      const path = el.dataset.dropFolder ?? '';
      const match =
        dest != null &&
        (path === dest ||
          ((dest === '/' || dest === '') && (path === '/' || path === '')));
      el.classList.toggle('drop-folder', match);
    }
  }

  function clearInternalDrag() {
    pendingDrag = null;
    activeDragSources = null;
    isInternalDragging = false;
    setDropHighlight(null);
  }

  function onPointerMoveDuringDrag(e: PointerEvent) {
    const pending = pendingDrag;
    if (!pending || e.pointerId !== pending.pointerId) return;

    if (!isInternalDragging) {
      const dx = e.clientX - pending.startX;
      const dy = e.clientY - pending.startY;
      if (dx * dx + dy * dy < DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX) return;

      // Start drag after threshold — select row if it wasn't already multi-selected.
      let sources = pending.sources;
      if (!sources) {
        sources = resolveDragSources(pending.index, pending.path);
        pending.sources = sources;
        if (!(selectedPaths.has(pending.path) && selectedPaths.size > 1)) {
          onSelectionChange(new Set([pending.path]));
          focusedIndex = pending.index;
          anchorIndex = pending.index;
        }
      }
      activeDragSources = sources;
      isInternalDragging = true;
      suppressClickAfterDrag = true;
    }

    const sources = activeDragSources;
    if (!sources?.length) return;

    const folder = folderUnderPoint(e.clientX, e.clientY);
    if (folder && isValidMoveDest(sources, folder)) {
      setDropHighlight(folder);
    } else {
      setDropHighlight(null);
    }
  }

  function onPointerUpDuringDrag(e: PointerEvent) {
    const pending = pendingDrag;
    if (!pending || e.pointerId !== pending.pointerId) return;

    const sources = activeDragSources;
    const dest = dragOverFolder;
    const didDrag = isInternalDragging;

    window.removeEventListener('pointermove', onPointerMoveDuringDrag);
    window.removeEventListener('pointerup', onPointerUpDuringDrag);
    window.removeEventListener('pointercancel', onPointerCancelDuringDrag);
    window.removeEventListener('keydown', onKeyDownDuringDrag);

    clearInternalDrag();

    if (
      didDrag &&
      sources?.length &&
      dest &&
      isValidMoveDest(sources, dest) &&
      onMoveEntries
    ) {
      onMoveEntries(sources, dest);
    }

    // Allow the next click cycle to run after a short delay if we dragged.
    if (didDrag) {
      requestAnimationFrame(() => {
        suppressClickAfterDrag = false;
      });
    }
  }

  function onPointerCancelDuringDrag(e: PointerEvent) {
    if (!pendingDrag || e.pointerId !== pendingDrag.pointerId) return;
    window.removeEventListener('pointermove', onPointerMoveDuringDrag);
    window.removeEventListener('pointerup', onPointerUpDuringDrag);
    window.removeEventListener('pointercancel', onPointerCancelDuringDrag);
    window.removeEventListener('keydown', onKeyDownDuringDrag);
    clearInternalDrag();
    suppressClickAfterDrag = false;
  }

  function onKeyDownDuringDrag(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    window.removeEventListener('pointermove', onPointerMoveDuringDrag);
    window.removeEventListener('pointerup', onPointerUpDuringDrag);
    window.removeEventListener('pointercancel', onPointerCancelDuringDrag);
    window.removeEventListener('keydown', onKeyDownDuringDrag);
    clearInternalDrag();
    suppressClickAfterDrag = false;
  }

  function handleRowPointerDown(e: PointerEvent, index: number) {
    if (!canEdit || !onMoveEntries) return;
    // Only primary button; ignore modifier multi-select gestures for drag start.
    if (e.button !== 0) return;
    if (e.ctrlKey || e.metaKey || e.shiftKey) return;

    const entry = visibleEntries[index];
    if (!entry) return;

    pendingDrag = {
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      index,
      path: entry.path as string,
      sources: null
    };

    window.addEventListener('pointermove', onPointerMoveDuringDrag);
    window.addEventListener('pointerup', onPointerUpDuringDrag);
    window.addEventListener('pointercancel', onPointerCancelDuringDrag);
    window.addEventListener('keydown', onKeyDownDuringDrag);
  }

  function handleRowClickGuarded(e: MouseEvent, index: number) {
    if (suppressClickAfterDrag || isInternalDragging) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    handleRowClick(e, index);
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
  class:internal-dragging={isInternalDragging}
  class:can-internal-drag={canEdit && !!onMoveEntries}
  bind:this={container}
  tabindex="0" 
  onkeydown={handleKeyDown}
  onscroll={handleScroll}
  ondragstart={(e) => {
    // Block native HTML5 drag so WebView2 never shows the OS "not-allowed" cursor
    // for in-archive moves (we use pointer events instead).
    e.preventDefault();
  }}
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
      {#if showParentUpRow && parentDropPath}
        <tr
          class="data-row parent-up-row"
          class:drop-folder={dragOverFolder === parentDropPath}
          data-drop-folder={parentDropPath}
          data-is-dir="true"
          title="Go up one level (drop files here to move out of this folder)"
          onclick={() => onNavigate(parentDropPath)}
          ondblclick={() => onNavigate(parentDropPath)}
        >
          <td>
            <div class="name-cell-inner">
              <span class="icon-span parent-up-icon" aria-hidden="true">↑</span>
              <div class="name-stack">
                <span class="name-text">..</span>
                {#if archiveMode}
                  <span class="path-text">Parent folder</span>
                {/if}
              </div>
            </div>
          </td>
          <td>{'<DIR>'}</td>
          <td>-</td>
          <td>-</td>
          <td>-</td>
        </tr>
      {/if}
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
            class:drop-folder={dragOverFolder === entry.path}
            data-entry-path={entry.path as string}
            data-drop-folder={entry.is_directory ? (entry.path as string) : undefined}
            data-is-dir={entry.is_directory ? 'true' : 'false'}
            onpointerdown={(e) => handleRowPointerDown(e, index)}
            onclick={(e) => handleRowClickGuarded(e, index)}
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
  :global(.archive-table-container.can-internal-drag) tr.data-row:not(.parent-up-row) {
    cursor: grab;
  }
  :global(.archive-table-container.internal-dragging) {
    cursor: grabbing;
    user-select: none;
  }
  :global(.archive-table-container.internal-dragging) tr.data-row {
    cursor: grabbing;
  }
  tr.parent-up-row {
    color: var(--text-muted);
    cursor: pointer;
  }
  tr.parent-up-row .name-text {
    font-weight: 600;
  }
  .parent-up-icon {
    font-size: 14px;
    opacity: 0.85;
  }
</style>
