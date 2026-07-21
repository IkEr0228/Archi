<script lang="ts">
  import { onMount } from 'svelte';

  let {
    canExtract = false,
    canExtractSelected = false,
    canExtractHere = true,
    canTest = false,
    canShowProperties = false,
    canAdd = false,
    canNewFolder = false,
    canRename = false,
    canDelete = false,
    canReplace = false,
    canCompact = false,
    canEditStrategy = false,
    editStrategy = 'auto' as 'auto' | 'preferFast' | 'preferCompact',
    extractTitle = "Extract All Files",
    onOpenArchive,
    onCreateArchive,
    onExtractDefault,
    onExtractAll,
    onExtractSelected,
    onExtractHere,
    onExtractNamed,
    onTestArchive,
    onShowProperties,
    onAddToArchive,
    onNewFolder,
    onRename,
    onDelete,
    onReplace,
    onCompact,
    onEditStrategyChange,
    onFileAssociations,
  } = $props<{
    canExtract?: boolean;
    canExtractSelected?: boolean;
    canExtractHere?: boolean;
    canTest?: boolean;
    canShowProperties?: boolean;
    canAdd?: boolean;
    canNewFolder?: boolean;
    canRename?: boolean;
    canDelete?: boolean;
    canReplace?: boolean;
    canCompact?: boolean;
    canEditStrategy?: boolean;
    editStrategy?: 'auto' | 'preferFast' | 'preferCompact';
    extractTitle?: string;
    onOpenArchive: () => void;
    onCreateArchive: () => void;
    onExtractDefault: () => void;
    onExtractAll: () => void;
    onExtractSelected: () => void;
    onExtractHere: () => void;
    onExtractNamed: () => void;
    onTestArchive: () => void;
    onShowProperties: () => void;
    onAddToArchive: () => void;
    onNewFolder: () => void;
    onRename: () => void;
    onDelete: () => void;
    onReplace: () => void;
    onCompact: () => void;
    onEditStrategyChange: (strategy: 'auto' | 'preferFast' | 'preferCompact') => void;
    onFileAssociations: () => void;
  }>();

  type EditStrategy = 'auto' | 'preferFast' | 'preferCompact';

  let menuOpen = $state(false);
  let splitEl: HTMLDivElement | undefined = $state();
  let extractMainEl: HTMLButtonElement | undefined = $state();
  let extractCaretEl: HTMLButtonElement | undefined = $state();

  let editMenuOpen = $state(false);
  let editSplitEl: HTMLDivElement | undefined = $state();
  let editTriggerEl: HTMLButtonElement | undefined = $state();

  function strategyLabel(value: EditStrategy): string {
    if (value === 'preferFast') return 'Fast';
    if (value === 'preferCompact') return 'Compact';
    return 'Auto';
  }

  function toggleMenu() {
    if (!canExtract) return;
    editMenuOpen = false;
    menuOpen = !menuOpen;
  }

  function toggleEditMenu() {
    if (!canEditStrategy) return;
    menuOpen = false;
    editMenuOpen = !editMenuOpen;
  }

  function run(action: () => void) {
    menuOpen = false;
    editMenuOpen = false;
    action();
  }

  function closeMenu(restoreFocus = false) {
    if (!menuOpen) return;
    menuOpen = false;
    if (restoreFocus) {
      queueMicrotask(() => {
        (extractCaretEl ?? extractMainEl)?.focus();
      });
    }
  }

  function closeEditMenu(restoreFocus = false) {
    if (!editMenuOpen) return;
    editMenuOpen = false;
    if (restoreFocus) {
      queueMicrotask(() => editTriggerEl?.focus());
    }
  }

  function pickStrategy(value: EditStrategy) {
    editMenuOpen = false;
    onEditStrategyChange(value);
  }

  onMount(() => {
    function onPointerDown(event: PointerEvent) {
      const target = event.target as Node | null;
      if (menuOpen && splitEl && target && !splitEl.contains(target)) {
        closeMenu(false);
      }
      if (editMenuOpen && editSplitEl && target && !editSplitEl.contains(target)) {
        closeEditMenu(false);
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== 'Escape') return;
      if (menuOpen) closeMenu(true);
      if (editMenuOpen) closeEditMenu(true);
    }

    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  });
</script>

<div class="toolbar monospace" role="toolbar">
  <button
    type="button"
    onclick={onOpenArchive}
    aria-label="Open Archive"
    title="Open Archive (ZIP)"
  >
    Open
  </button>
  <button
    type="button"
    onclick={onCreateArchive}
    aria-label="Create Archive"
    title="Create Archive from Files"
  >
    Create
  </button>
  <div class="extract-split" bind:this={splitEl}>
    <button
      type="button"
      class="extract-main"
      bind:this={extractMainEl}
      onclick={() => run(onExtractDefault)}
      disabled={!canExtract}
      aria-label={extractTitle}
      title={extractTitle}
    >
      Extract
    </button>
    <button
      type="button"
      class="extract-caret"
      bind:this={extractCaretEl}
      onclick={toggleMenu}
      disabled={!canExtract}
      aria-label="Extract options"
      aria-haspopup="menu"
      aria-expanded={menuOpen}
      title="Extract options"
    >
      ▾
    </button>
    {#if menuOpen}
      <div class="extract-menu monospace" role="menu">
        <button type="button" role="menuitem" onclick={() => run(onExtractAll)}>
          Extract All…
        </button>
        <button
          type="button"
          role="menuitem"
          disabled={!canExtractSelected}
          title={!canExtractSelected ? "Select entries to extract" : "Extract selected entries"}
          onclick={() => run(onExtractSelected)}
        >
          Extract Selected…
        </button>
        <button
          type="button"
          role="menuitem"
          disabled={!canExtractHere}
          title={!canExtractHere ? "Archive has no parent directory" : "Extract beside the archive"}
          onclick={() => run(onExtractHere)}
        >
          Extract Here
        </button>
        <button type="button" role="menuitem" onclick={() => run(onExtractNamed)}>
          Extract to archive folder…
        </button>
      </div>
    {/if}
  </div>
  <button
    type="button"
    onclick={onTestArchive}
    disabled={!canTest}
    aria-label="Test Archive"
    title={canTest ? "Test archive integrity (CRC)" : "Test archive unavailable"}
  >
    Test
  </button>
  <button
    type="button"
    onclick={onShowProperties}
    disabled={!canShowProperties}
    aria-label="Archive Properties"
    title={canShowProperties ? "Show archive properties" : "Open an archive first"}
  >
    Properties
  </button>

  <span class="toolbar-sep" aria-hidden="true"></span>

  <button
    type="button"
    onclick={onAddToArchive}
    disabled={!canAdd}
    aria-label="Add Files to Archive"
    title={canAdd ? "Add files or folders to the current archive folder" : "Edit unavailable for this archive"}
  >
    Add
  </button>
  <button
    type="button"
    onclick={onNewFolder}
    disabled={!canNewFolder}
    aria-label="New Folder in Archive"
    title={canNewFolder ? "Create a folder in the current archive folder" : "Edit unavailable for this archive"}
  >
    New Folder
  </button>
  <button
    type="button"
    onclick={onRename}
    disabled={!canRename}
    aria-label="Rename Selected Entry"
    title={canRename ? "Rename selected entry" : "Select exactly one entry to rename"}
  >
    Rename
  </button>
  <button
    type="button"
    onclick={onDelete}
    disabled={!canDelete}
    aria-label="Delete Selected Entries"
    title={canDelete ? "Delete selected entries (speed follows Edit mode)" : "Select entries to delete"}
  >
    Delete
  </button>
  <button
    type="button"
    onclick={onReplace}
    disabled={!canReplace}
    aria-label="Replace Selected File"
    title={canReplace ? "Replace selected file contents" : "Select exactly one file to replace"}
  >
    Replace
  </button>
  <button
    type="button"
    onclick={onCompact}
    disabled={!canCompact}
    aria-label="Compact Archive"
    title={
      canCompact
        ? "Rewrite archive cleanly (reclaim space after fast ZIP delete)"
        : "Open an editable archive to compact"
    }
  >
    Compact
  </button>

  <div
    class="toolbar-edit-mode archive-search-label"
    class:disabled={!canEditStrategy}
    title="Edit mode: Auto balances speed and size; Fast prefers logical/pack-copy; Compact always full rebuild"
  >
    <span class="toolbar-edit-mode-label" id="edit-mode-label">Edit</span>
    <div class="type-filter-split" bind:this={editSplitEl}>
      <button
        type="button"
        class="type-filter-trigger"
        bind:this={editTriggerEl}
        onclick={toggleEditMenu}
        disabled={!canEditStrategy}
        aria-labelledby="edit-mode-label"
        aria-label="Edit speed mode"
        aria-haspopup="menu"
        aria-expanded={editMenuOpen}
        title="Edit speed mode"
      >
        {strategyLabel(editStrategy)}
        <span class="type-filter-caret" aria-hidden="true">▾</span>
      </button>
      {#if editMenuOpen}
        <div class="extract-menu type-filter-menu monospace" role="menu" aria-label="Edit speed mode">
          <button
            type="button"
            role="menuitemradio"
            aria-checked={editStrategy === 'auto'}
            class:menu-item-active={editStrategy === 'auto'}
            onclick={() => pickStrategy('auto')}
          >
            Auto
          </button>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={editStrategy === 'preferFast'}
            class:menu-item-active={editStrategy === 'preferFast'}
            onclick={() => pickStrategy('preferFast')}
          >
            Fast
          </button>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={editStrategy === 'preferCompact'}
            class:menu-item-active={editStrategy === 'preferCompact'}
            onclick={() => pickStrategy('preferCompact')}
          >
            Compact
          </button>
        </div>
      {/if}
    </div>
  </div>

  <span class="toolbar-sep" aria-hidden="true"></span>

  <button
    type="button"
    onclick={onFileAssociations}
    aria-label="File Associations"
    title="Opt-in Explorer file associations (Windows)"
  >
    Associations
  </button>
</div>
