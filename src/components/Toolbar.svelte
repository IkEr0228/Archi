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
    onFileAssociations: () => void;
  }>();

  let menuOpen = $state(false);
  let splitEl: HTMLDivElement | undefined = $state();
  let extractMainEl: HTMLButtonElement | undefined = $state();
  let extractCaretEl: HTMLButtonElement | undefined = $state();

  function toggleMenu() {
    if (!canExtract) return;
    menuOpen = !menuOpen;
  }

  function run(action: () => void) {
    menuOpen = false;
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

  onMount(() => {
    function onPointerDown(event: PointerEvent) {
      if (!menuOpen || !splitEl) return;
      const target = event.target as Node | null;
      if (target && !splitEl.contains(target)) {
        closeMenu(false);
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape' && menuOpen) {
        closeMenu(true);
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
    title={canAdd ? "Add files or folders to the current archive folder" : "ZIP edit unavailable"}
  >
    Add
  </button>
  <button
    type="button"
    onclick={onNewFolder}
    disabled={!canNewFolder}
    aria-label="New Folder in Archive"
    title={canNewFolder ? "Create a folder in the current archive folder" : "ZIP edit unavailable"}
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
    title={canDelete ? "Delete selected entries" : "Select entries to delete"}
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
