<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';

  import TitleBarComponent from '../components/TitleBar.svelte';
  import ToolbarComponent from '../components/Toolbar.svelte';
  import ArchiveSearchBar from '../components/ArchiveSearchBar.svelte';
  import BreadcrumbsComponent from '../components/Breadcrumbs.svelte';
  import StatusBarComponent from '../components/StatusBar.svelte';
  import EmptyStateComponent from '../components/EmptyState.svelte';
  import ArchiveTableComponent from '../components/ArchiveTable.svelte';
  import CreateArchiveModal from '../components/CreateArchiveModal.svelte';
  import FileAssociationsModal from '../components/FileAssociationsModal.svelte';
  import {
    canExtractArchive,
    shouldShowRiskBanner,
    extractButtonTitle,
    warningDisplayText
  } from '../lib/archiveRisks.js';
  import { parentDir, resolveExtractDestination } from '../lib/extractDest.js';
  import { filterAndSortEntries, isArchiveQueryActive } from '../lib/archiveQuery.js';
  import { buildArchiveIndexes } from '../lib/archiveIndex.js';
  import { formatInvokeError } from '../lib/invokeError.js';
  import {
    ensureCreateExtension,
    isArchivePath,
    withCreateExtension,
  } from '../lib/createPaths.js';

  interface ArchiveEntry {
    path: string;
    name: string;
    parent_path: string;
    is_directory: boolean;
    uncompressed_size: number;
    compressed_size: number | null;
    modified_at: string | null;
    method: string | null;
  }

  interface ArchiveCapabilities {
    open: boolean;
    list: boolean;
    extract: boolean;
    create: boolean;
    edit: boolean;
    encrypt: boolean;
    test: boolean;
  }

  interface ArchiveStats {
    file_count: number;
    folder_count: number;
    total_uncompressed: number;
    total_compressed: number;
    methods: string[];
  }

  interface ArchiveInfo {
    archive_path: string;
    format: string;
    entries: ArchiveEntry[];
    capabilities: ArchiveCapabilities;
    warnings: { code: string; message: string }[];
    stats: ArchiveStats;
  }

  interface OperationProgress {
    operation_id: string;
    percentage: number;
    current_file: string;
    extracted_files?: number;
    total_files?: number;
    /** Optional phase: plan | append | rebuild | extract | repack | finalize */
    phase?: string | null;
  }

  interface OperationSummary {
    operation_id: string;
    extracted_files: number;
    total_files: number;
    skipped_files: number;
    destination: string;
  }

  interface TestArchiveSummary {
    operation_id: string;
    total_entries: number;
    tested_ok: number;
    tested_failed: number;
    failures: { path: string; message: string }[];
  }

  interface ActiveOperation {
    id: string;
    kind: 'extract' | 'create' | 'test' | 'edit';
  }

  interface EditSummary {
    operation_id: string;
    destination: string;
    members_written: number;
    strategy_used?: string | null;
  }

  const emptyStats: ArchiveStats = {
    file_count: 0,
    folder_count: 0,
    total_uncompressed: 0,
    total_compressed: 0,
    methods: []
  };

  interface ExtractConflictEvent {
    operation_id: string;
    conflict_id: string;
    entry_path: string;
    dest_path: string;
  }

  type ConflictDecision = 'overwrite' | 'skip' | 'rename' | 'cancel';

  const unavailableCapabilities: ArchiveCapabilities = {
    open: false,
    list: false,
    extract: false,
    create: false,
    edit: false,
    encrypt: false,
    test: false
  };

  // --- State management ---
  let currentArchivePath = $state('');
  let currentArchiveFormat = $state('');
  let currentInternalPath = $state('/');
  let archiveEntries = $state<ArchiveEntry[]>([]);
  let selectedPaths = $state<Set<string>>(new Set());
  let operationStatus = $state('Ready');
  let errorMessage = $state('');
  let openRequestId = 0;
  let activeOperation = $state<ActiveOperation | null>(null);
  let archiveCapabilities = $state<ArchiveCapabilities>({ ...unavailableCapabilities });
  let archiveWarnings = $state<{ code: string; message: string }[]>([]);
  let archiveStats = $state<ArchiveStats>({ ...emptyStats });
  let risksAcknowledged = $state(true);
  let showPropertiesModal = $state(false);

  // File associations (P3.5, opt-in)
  let showAssocModal = $state(false);
  let assocBusy = $state(false);
  let assocStatus = $state<{
    supported: boolean;
    enabled: boolean;
    associatedExtensions: string[];
    exePath: string | null;
    message: string;
  } | null>(null);

  // Create archive modal state
  let showCreateModal = $state(false);
  let createSources = $state<string[]>([]);
  let createFormat = $state<'zip' | 'tar' | 'tarGz' | 'tarBz2' | 'tarXz' | 'sevenZ'>('zip');
  let createCompression = $state<'store' | 'fast' | 'normal' | 'max'>('normal');
  let createIncludeRoot = $state(true);
  let createOverwrite = $state(false);
  let createOutputPath = $state('');

  // Search / filter / sort (table wiring of filters in Task 3)
  let searchInput = $state('');
  let searchQuery = $state('');
  let typeFilter = $state<'all' | 'files' | 'folders'>('all');
  let extensionFilter = $state('');
  let sortKey = $state('name');
  let sortDir = $state<'asc' | 'desc'>('asc');

  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  function handleSearchInput(value: string) {
    searchInput = value;
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      searchQuery = value;
    }, 120);
  }

  function clearSearchFilters() {
    searchInput = '';
    searchQuery = '';
    typeFilter = 'all';
    extensionFilter = '';
    if (debounceTimer) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }
  }

  function handleSortChange(key: string, dir: 'asc' | 'desc') {
    sortKey = key;
    sortDir = dir;
  }

  // Progress Modal state
  let showProgressModal = $state(false);
  let progressPercentage = $state(0);
  let progressText = $state('');
  let progressPhase = $state('');

  // Extract conflict dialog state
  let conflictPrompt = $state<null | {
    conflict_id: string;
    entry_path: string;
    dest_path: string;
  }>(null);
  let applyToAllChecked = $state(false);
  let overwriteBtnEl = $state<HTMLButtonElement | null>(null);

  // Prefer keyboard focus on the primary conflict action when the dialog opens.
  $effect(() => {
    if (conflictPrompt && overwriteBtnEl) {
      overwriteBtnEl.focus();
    }
  });

  // Derived state — prefer backend stats; avoid O(n) reduce when present.
  let totalUncompressedSize = $derived.by(() => {
    if (archiveStats.total_uncompressed > 0) {
      return archiveStats.total_uncompressed;
    }
    return archiveEntries.reduce((acc, entry) => acc + entry.uncompressed_size, 0);
  });

  let isArchiveOpen = $derived(currentArchivePath !== '');

  let canExtract = $derived(
    canExtractArchive({
      extractCapability: archiveCapabilities.extract,
      warnings: archiveWarnings,
      risksAcknowledged,
      busy: activeOperation !== null
    })
  );

  let canTest = $derived(
    isArchiveOpen && archiveCapabilities.test && activeOperation === null
  );
  let canShowProperties = $derived(isArchiveOpen);

  let canEditBase = $derived(
    isArchiveOpen && archiveCapabilities.edit && activeOperation === null
  );
  let canAdd = $derived(canEditBase);
  let canNewFolder = $derived(canEditBase);
  let canDelete = $derived(canEditBase && selectedPaths.size > 0);
  let canRename = $derived(canEditBase && selectedPaths.size === 1);

  // Rebuild only when the open archive entry list changes (open/edit/reload).
  let archiveIndexes = $derived(buildArchiveIndexes(archiveEntries));

  let singleSelectedEntry = $derived.by((): ArchiveEntry | null => {
    if (selectedPaths.size !== 1) return null;
    const path = Array.from(selectedPaths)[0];
    const hit = archiveIndexes.byPath.get(path);
    return hit ? (hit as ArchiveEntry) : null;
  });

  let canReplace = $derived(
    canEditBase &&
      singleSelectedEntry !== null &&
      !singleSelectedEntry.is_directory
  );

  let showRiskBanner = $derived(shouldShowRiskBanner(archiveWarnings, risksAcknowledged));

  let canExtractSelected = $derived(canExtract && selectedPaths.size > 0);

  let canExtractHere = $derived(
    canExtract && parentDir(currentArchivePath) !== null
  );

  let extractTitle = $derived(
    !canExtract
      ? extractButtonTitle(archiveWarnings, risksAcknowledged)
      : selectedPaths.size > 0
        ? 'Extract Selected'
        : extractButtonTitle(archiveWarnings, risksAcknowledged)
  );

  let archiveQueryActive = $derived(
    isArchiveQueryActive({ query: searchQuery, typeFilter, extension: extensionFilter })
  );

  // Single filter+sort per query/nav/sort change (shared by table + matchCount).
  let visibleEntries = $derived(
    filterAndSortEntries({
      entries: archiveEntries,
      indexes: archiveIndexes,
      currentInternalPath,
      query: searchQuery,
      typeFilter,
      extension: extensionFilter,
      sortKey,
      sortDir
    })
  );

  let matchCount = $derived(archiveQueryActive ? visibleEntries.length : null);

  /** Highlight drop target when dragging files over the window. */
  let fileDragActive = $state(false);

  onMount(() => {
    // External file DnD (Explorer → Archi):
    // - Archive open + editable: add into current virtual folder
    // - No archive: single archive path → open; otherwise open Create modal
    const unlistenDragDrop = getCurrentWebview().onDragDropEvent((event) => {
      const kind = event.payload.type;
      if (kind === 'enter' || kind === 'over') {
        fileDragActive = true;
        return;
      }
      if (kind === 'leave') {
        fileDragActive = false;
        return;
      }
      if (kind !== 'drop') return;
      fileDragActive = false;
      if (activeOperation) return;
      const paths = event.payload.paths;
      if (!paths?.length) return;
      handleExternalFileDrop(paths);
    });

    // rAF-coalesce progress: stash latest payload; apply at most once per frame.
    /** @type {OperationProgress | null} */
    let pendingProgress: OperationProgress | null = null;
    /** @type {number | null} */
    let progressRaf: number | null = null;

    const flushProgress = () => {
      progressRaf = null;
      const payload = pendingProgress;
      pendingProgress = null;
      if (!payload || !showProgressModal) return;
      const nextPhase = payload.phase ?? '';
      // Skip no-op updates (same % + current file + phase).
      if (
        payload.percentage === progressPercentage &&
        payload.current_file === progressText &&
        nextPhase === progressPhase
      ) {
        return;
      }
      progressPercentage = payload.percentage;
      progressText = payload.current_file;
      progressPhase = nextPhase;
    };

    const updateProgress = (event: { payload: unknown }) => {
      const payload = event.payload as OperationProgress;
      if (payload.operation_id !== activeOperation?.id) return;
      if (!showProgressModal) return;
      pendingProgress = payload;
      if (progressRaf !== null) return;
      progressRaf = requestAnimationFrame(flushProgress);
    };
    const unlistenExtractProgress = listen('extract-progress', updateProgress);
    const unlistenCreateProgress = listen('create-progress', updateProgress);
    const unlistenTestProgress = listen('test-progress', updateProgress);
    const unlistenEditProgress = listen('edit-progress', updateProgress);

    const unlistenExtractConflict = listen('extract-conflict', (event: { payload: unknown }) => {
      const payload = event.payload as ExtractConflictEvent;
      if (payload.operation_id !== activeOperation?.id) return;
      conflictPrompt = {
        conflict_id: payload.conflict_id,
        entry_path: payload.entry_path,
        dest_path: payload.dest_path
      };
      applyToAllChecked = false;
    });

    // Second-instance CLI open: open path when idle, else report busy.
    const unlistenCliOpen = listen('cli-open', (event: { payload: unknown }) => {
      const payload = event.payload as { path?: string | null };
      const path = payload?.path;
      if (!path) return;
      if (activeOperation) {
        errorMessage =
          'Cannot open archive from CLI: an operation is already in progress.';
        return;
      }
      openArchiveAtPath(path);
    });

    // First-instance startup path (if launched with archive arg).
    void (async () => {
      try {
        const path = await invoke<string | null>('get_startup_cli_path');
        if (path) {
          openArchiveAtPath(path);
        }
      } catch (e: unknown) {
        errorMessage = `Failed to read startup CLI path: ${formatInvokeError(e)}`;
      }
    })();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && conflictPrompt) {
        event.preventDefault();
        resolveConflict('cancel');
      }
    };
    window.addEventListener('keydown', onKeyDown);

    return () => {
      if (progressRaf !== null) {
        cancelAnimationFrame(progressRaf);
        progressRaf = null;
      }
      pendingProgress = null;
      unlistenDragDrop.then((fn) => fn());
      unlistenExtractProgress.then((fn) => fn());
      unlistenCreateProgress.then((fn) => fn());
      unlistenTestProgress.then((fn) => fn());
      unlistenEditProgress.then((fn) => fn());
      unlistenExtractConflict.then((fn) => fn());
      unlistenCliOpen.then((fn) => fn());
      window.removeEventListener('keydown', onKeyDown);
    };
  });

  /** Normalize archive folder path for add/create-folder parent (`/` → empty). */
  function archiveParentForAdd(internalPath: string): string {
    if (!internalPath || internalPath === '/') return '';
    return internalPath.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  }

  /** Join parent folder + leaf name into an archive entry path. */
  function joinArchiveEntryPath(parent: string, name: string): string {
    const leaf = name.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
    const base = archiveParentForAdd(parent);
    return base ? `${base}/${leaf}` : leaf;
  }

  /** Whether an internal folder path still exists (byPath or has children via byParent). */
  function folderStillExists(
    indexes: { byPath: Map<string, unknown>; byParent: Map<string, unknown> },
    folderPath: string
  ): boolean {
    if (!folderPath || folderPath === '/') return true;
    return indexes.byPath.has(folderPath) || indexes.byParent.has(folderPath);
  }

  async function openArchiveAtPath(
    path: string,
    options?: { preserveInternalPath?: string }
  ) {
    const requestId = ++openRequestId;
    const preservePath = options?.preserveInternalPath;
    try {
      operationStatus = 'Opening archive...';
      errorMessage = '';
      archiveCapabilities = { ...unavailableCapabilities };
      archiveWarnings = [];
      risksAcknowledged = true;
      const info = await invoke<ArchiveInfo>('open_archive_metadata', { path });
      if (requestId !== openRequestId) return;
      currentArchivePath = info.archive_path;
      currentArchiveFormat = info.format ?? '';
      archiveEntries = info.entries;
      archiveCapabilities = info.capabilities;
      archiveWarnings = info.warnings ?? [];
      archiveStats = info.stats ?? { ...emptyStats };
      risksAcknowledged = archiveWarnings.length === 0;
      // Indexes for preserve-path check (same shape as $derived archiveIndexes).
      const openIndexes = buildArchiveIndexes(info.entries);
      if (preservePath && folderStillExists(openIndexes, preservePath)) {
        currentInternalPath = preservePath;
      } else {
        currentInternalPath = '/';
      }
      selectedPaths = new Set();
      operationStatus = `Loaded ${info.entries.length} entries.`;
    } catch (e: any) {
      if (requestId !== openRequestId) return;
      console.error(e);
      archiveCapabilities = { ...unavailableCapabilities };
      archiveWarnings = [];
      archiveStats = { ...emptyStats };
      risksAcknowledged = true;
      errorMessage = `Failed to open archive: ${formatInvokeError(e)}`;
      operationStatus = 'Error';
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  }

  function openProperties() {
    if (!isArchiveOpen) return;
    showPropertiesModal = true;
  }

  async function handleTestArchive() {
    if (!canTest || activeOperation) return;
    let operationId: string | null = null;
    try {
      operationId = crypto.randomUUID();
      activeOperation = { id: operationId, kind: 'test' };
      showProgressModal = true;
      progressPercentage = 0;
      progressPhase = '';
      progressText = 'Starting integrity test...';
      errorMessage = '';

      const summary = await invoke<TestArchiveSummary>('test_archive_command', {
        operationId,
        zipPath: currentArchivePath
      });
      if (summary.operation_id !== operationId || activeOperation?.id !== operationId) return;

      if (summary.tested_failed === 0) {
        operationStatus = `Test OK: ${summary.tested_ok} file(s).`;
      } else {
        operationStatus = `Test failed: ${summary.tested_failed} of ${summary.total_entries} file(s).`;
        const first = summary.failures[0];
        errorMessage = first
          ? `Test failed: ${first.path} — ${first.message}`
          : `Test failed: ${summary.tested_failed} file(s) failed integrity check.`;
      }
    } catch (e: any) {
      if (operationId && activeOperation?.id !== operationId) return;
      errorMessage = `Archive test failed: ${formatInvokeError(e)}`;
      operationStatus = 'Error';
    } finally {
      if (operationId && activeOperation?.id === operationId) {
        showProgressModal = false;
        activeOperation = null;
      }
    }
  }

  async function handleOpenArchive() {
    try {
      const path = await invoke<string | null>('select_archive_file');
      if (path) {
        openArchiveAtPath(path);
      }
    } catch (e: any) {
      errorMessage = `Dialog selection error: ${formatInvokeError(e)}`;
    }
  }

  type ExtractMode = 'all' | 'selected' | 'here' | 'named';

  async function runExtract(mode: ExtractMode) {
    if (!isArchiveOpen || activeOperation) return;
    if (!canExtractArchive({
      extractCapability: archiveCapabilities.extract,
      warnings: archiveWarnings,
      risksAcknowledged,
      busy: false
    })) {
      return;
    }
    if (mode === 'selected' && selectedPaths.size === 0) return;

    let operationId: string | null = null;
    try {
      let chosenFolder: string | null = null;
      if (mode === 'all' || mode === 'selected' || mode === 'named') {
        chosenFolder = await invoke<string | null>('select_directory');
        if (!chosenFolder || activeOperation) return;
      }

      let destDir = resolveExtractDestination({
        mode,
        archivePath: currentArchivePath,
        chosenFolder
      });
      if (!destDir) {
        errorMessage = 'Could not resolve extract destination.';
        return;
      }

      if (mode === 'named') {
        destDir = await invoke<string>('ensure_directory', { path: destDir });
      }

      operationId = crypto.randomUUID();
      activeOperation = { id: operationId, kind: 'extract' };
      showProgressModal = true;
      progressPercentage = 0;
      progressPhase = '';
      progressText = 'Starting extraction...';

      const selected =
        mode === 'selected' ? Array.from(selectedPaths) : null;

      const summary = await invoke<OperationSummary>('extract_archive_command', {
        operationId,
        zipPath: currentArchivePath,
        destDir,
        selectedPaths: selected
      });
      if (summary.operation_id !== operationId || activeOperation?.id !== operationId) return;
      const skipped =
        summary.skipped_files > 0 ? ` (${summary.skipped_files} skipped)` : '';
      operationStatus = `Extracted ${summary.extracted_files} entries to: ${summary.destination}${skipped}`;
    } catch (e: any) {
      if (operationId && activeOperation?.id !== operationId) return;
      errorMessage = `Extraction failed: ${formatInvokeError(e)}`;
      operationStatus = 'Error';
    } finally {
      if (operationId && activeOperation?.id === operationId) {
        showProgressModal = false;
        conflictPrompt = null;
        applyToAllChecked = false;
        activeOperation = null;
      }
    }
  }

  async function resolveConflict(decision: ConflictDecision) {
    const conflict = conflictPrompt;
    const operation = activeOperation;
    if (!conflict || !operation) return;
    // Clear immediately so Esc/double-clicks cannot double-submit.
    conflictPrompt = null;
    try {
      await invoke('resolve_extract_conflict', {
        operationId: operation.id,
        conflictId: conflict.conflict_id,
        decision,
        applyToAll: applyToAllChecked
      });
    } catch (e: any) {
      errorMessage = `Could not resolve conflict: ${formatInvokeError(e)}`;
    }
  }

  function handleExtractDefault() {
    runExtract(selectedPaths.size > 0 ? 'selected' : 'all');
  }

  function acknowledgeRisks() {
    risksAcknowledged = true;
  }

  async function cancelOperation() {
    if (!activeOperation) return;
    try {
      if (await invoke<boolean>('cancel_operation', { operationId: activeOperation.id })) {
        progressText = 'Cancelling operation...';
      }
    } catch (e: any) {
      errorMessage = `Could not cancel operation: ${formatInvokeError(e)}`;
    }
  }

  function resetCreateOptions() {
    createFormat = 'zip';
    createCompression = 'normal';
    createIncludeRoot = true;
    createOverwrite = false;
    createOutputPath = '';
  }

  function openCreateModal(sources: string[]) {
    if (activeOperation || !sources.length) return;
    createSources = sources;
    resetCreateOptions();
    showCreateModal = true;
    errorMessage = '';
  }

  function handleCreateFormatChange(format: 'zip' | 'tar' | 'tarGz' | 'tarBz2' | 'tarXz' | 'sevenZ') {
    createFormat = format;
    if (format === 'tar') {
      createCompression = 'store';
    } else if (format === 'sevenZ') {
      // Default 7z create to maximum LZMA2 effort.
      createCompression = 'max';
    }
    if (createOutputPath.trim()) {
      createOutputPath = withCreateExtension(createOutputPath, format);
    }
  }

  async function handleCreateArchive() {
    if (activeOperation) return;
    try {
      const sources = await invoke<string[] | null>('select_multiple_files');
      if (!sources || sources.length === 0) return;
      openCreateModal(sources);
    } catch (e: any) {
      errorMessage = `Could not select sources: ${formatInvokeError(e)}`;
    }
  }

  async function browseCreateOutput() {
    try {
      const picked = await invoke<string | null>('select_save_archive', {
        format: createFormat
      });
      if (picked) createOutputPath = ensureCreateExtension(picked, createFormat);
    } catch (e: any) {
      errorMessage = `Could not choose output: ${formatInvokeError(e)}`;
    }
  }

  async function confirmCreateArchive() {
    if (activeOperation || !showCreateModal) return;
    if (!createSources.length || !createOutputPath.trim()) return;
    let operationId: string | null = null;
    const outputPath = ensureCreateExtension(createOutputPath.trim(), createFormat);
    const sources = [...createSources];
    const compression = createFormat === 'tar' ? 'store' : createCompression;
    const options = {
      format: createFormat,
      compression,
      includeRoot: createIncludeRoot,
      overwrite: createOverwrite
    };
    showCreateModal = false;
    try {
      operationStatus = 'Creating archive...';
      errorMessage = '';
      operationId = crypto.randomUUID();
      activeOperation = { id: operationId, kind: 'create' };
      showProgressModal = true;
      progressPercentage = 0;
      progressPhase = '';
      progressText = 'Starting archive creation...';

      const summary = await invoke<OperationSummary>('create_archive_command', {
        sourcePaths: sources,
        outputZipPath: outputPath,
        operationId,
        options
      });
      if (summary.operation_id !== operationId || activeOperation?.id !== operationId) return;
      operationStatus = `Created ${summary.extracted_files} entries at: ${summary.destination}`;
      await openArchiveAtPath(outputPath);
    } catch (e: any) {
      if (operationId && activeOperation?.id !== operationId) return;
      errorMessage = `Archive creation failed: ${formatInvokeError(e)}`;
      operationStatus = 'Error';
    } finally {
      if (operationId && activeOperation?.id === operationId) {
        showProgressModal = false;
        conflictPrompt = null;
        applyToAllChecked = false;
        activeOperation = null;
      }
    }
  }

  async function refreshAssociationStatus() {
    try {
      assocStatus = await invoke('get_file_association_status_command');
    } catch (e: any) {
      errorMessage = `Could not read association status: ${formatInvokeError(e)}`;
    }
  }

  async function openAssociationsModal() {
    showAssocModal = true;
    errorMessage = '';
    await refreshAssociationStatus();
  }

  async function enableAssociations() {
    if (assocBusy) return;
    assocBusy = true;
    try {
      assocStatus = await invoke('register_file_associations_command');
      operationStatus = assocStatus?.enabled
        ? 'File associations enabled for this user.'
        : assocStatus?.message ?? 'Associations updated.';
    } catch (e: any) {
      errorMessage = `Could not enable associations: ${formatInvokeError(e)}`;
    } finally {
      assocBusy = false;
    }
  }

  async function disableAssociations() {
    if (assocBusy) return;
    assocBusy = true;
    try {
      assocStatus = await invoke('unregister_file_associations_command');
      operationStatus = 'File associations cleared for this user.';
    } catch (e: any) {
      errorMessage = `Could not disable associations: ${formatInvokeError(e)}`;
    } finally {
      assocBusy = false;
    }
  }

  function handleNavigate(path: string) {
    currentInternalPath = path;
    selectedPaths = new Set();
  }

  function handleNavigateBack() {
    if (currentInternalPath === '/' || !currentInternalPath) return;
    
    const parts = currentInternalPath.split('/').filter(Boolean);
    parts.pop();
    
    if (parts.length === 0) {
      currentInternalPath = '/';
    } else {
      currentInternalPath = parts.join('/');
    }
    selectedPaths = new Set();
  }

  function handleSelectionChange(newSelection: Set<string>) {
    selectedPaths = newSelection;
  }

  async function runEditOperation(
    label: string,
    work: (operationId: string) => Promise<EditSummary>
  ) {
    if (!canEditBase || activeOperation) return;
    let operationId: string | null = null;
    const zipPath = currentArchivePath;
    const preservePath = currentInternalPath;
    try {
      operationId = crypto.randomUUID();
      activeOperation = { id: operationId, kind: 'edit' };
      showProgressModal = true;
      progressPercentage = 0;
      progressPhase = '';
      progressText = `Starting ${label}...`;
      errorMessage = '';
      operationStatus = `${label}...`;

      const summary = await work(operationId);
      if (summary.operation_id !== operationId || activeOperation?.id !== operationId) return;

      operationStatus = `${label} complete (${summary.members_written} members).`;
      await openArchiveAtPath(zipPath, { preserveInternalPath: preservePath });
    } catch (e: any) {
      if (operationId && activeOperation?.id !== operationId) return;
      errorMessage = `${label} failed: ${formatInvokeError(e)}`;
      operationStatus = 'Error';
    } finally {
      if (operationId && activeOperation?.id === operationId) {
        showProgressModal = false;
        activeOperation = null;
      }
    }
  }

  async function handleDeleteEntries() {
    if (!canDelete) return;
    const paths = Array.from(selectedPaths);
    if (paths.length === 0) return;
    const label =
      paths.length === 1
        ? `Delete "${paths[0]}"?`
        : `Delete ${paths.length} selected entries?`;
    if (!window.confirm(label)) return;

    await runEditOperation('Delete', (operationId) =>
      invoke<EditSummary>('delete_archive_entries_command', {
        operationId,
        archivePath: currentArchivePath,
        paths
      })
    );
  }

  async function handleRenameEntry() {
    if (!canRename || !singleSelectedEntry) return;
    const fromPath = singleSelectedEntry.path;
    const currentName = singleSelectedEntry.name;
    const nextName = window.prompt('New name:', currentName);
    if (nextName == null) return;
    const trimmed = nextName.trim();
    if (!trimmed || trimmed === currentName) return;
    if (trimmed.includes('/') || trimmed.includes('\\')) {
      errorMessage = 'Rename failed: new name must not contain path separators.';
      return;
    }
    const parent =
      singleSelectedEntry.parent_path === '/' || !singleSelectedEntry.parent_path
        ? ''
        : singleSelectedEntry.parent_path;
    const toPath = parent ? `${parent}/${trimmed}` : trimmed;

    await runEditOperation('Rename', (operationId) =>
      invoke<EditSummary>('rename_archive_entry_command', {
        operationId,
        archivePath: currentArchivePath,
        fromPath,
        toPath
      })
    );
  }

  async function handleNewFolder() {
    if (!canNewFolder) return;
    const name = window.prompt('New folder name:');
    if (name == null) return;
    const trimmed = name.trim();
    if (!trimmed) return;
    if (trimmed.includes('/') || trimmed.includes('\\')) {
      errorMessage = 'New folder failed: name must not contain path separators.';
      return;
    }
    const folderPath = joinArchiveEntryPath(currentInternalPath, trimmed);

    await runEditOperation('New folder', (operationId) =>
      invoke<EditSummary>('create_archive_folder_command', {
        operationId,
        archivePath: currentArchivePath,
        folderPath
      })
    );
  }

  /** Add disk paths into the open archive at the current virtual folder. */
  async function addSourcesToCurrentFolder(sources: string[]) {
    if (!canAdd || activeOperation || !sources.length || !currentArchivePath) return;
    const archiveParent = archiveParentForAdd(currentInternalPath);
    await runEditOperation('Add', (operationId) =>
      invoke<EditSummary>('add_to_archive_command', {
        operationId,
        archivePath: currentArchivePath,
        sourcePaths: sources,
        archiveParent
      })
    );
  }

  /**
   * Drop from Explorer onto the app window.
   * When an editable archive is open, files go into the current folder (breadcrumb path).
   */
  function handleExternalFileDrop(paths: string[]) {
    if (activeOperation) return;
    if (isArchiveOpen) {
      if (!canAdd) {
        errorMessage =
          'This archive cannot be edited (add files is only available for ZIP, TAR family, and 7z).';
        return;
      }
      void addSourcesToCurrentFolder(paths);
      return;
    }
    // No archive open: open one archive file, or start Create with dropped sources.
    if (paths.length === 1 && isArchivePath(paths[0])) {
      openArchiveAtPath(paths[0]);
      return;
    }
    openCreateModal(paths);
  }

  async function handleAddToArchive() {
    if (!canAdd || activeOperation) return;
    try {
      const sources = await invoke<string[] | null>('select_multiple_files');
      if (!sources || sources.length === 0) return;
      if (activeOperation) return;
      await addSourcesToCurrentFolder(sources);
    } catch (e: any) {
      errorMessage = `Could not select files to add: ${formatInvokeError(e)}`;
    }
  }

  /** In-archive drag-and-drop: move entries into a folder (leaf names kept). */
  async function handleMoveEntries(sources: string[], destFolder: string) {
    if (!canEditBase || activeOperation || !currentArchivePath || !sources.length) return;
    const dest = destFolder === '/' ? '' : destFolder;
    await runEditOperation('Move', (operationId) =>
      invoke<EditSummary>('move_archive_entries_command', {
        operationId,
        archivePath: currentArchivePath,
        sourcePaths: sources,
        destFolder: dest
      })
    );
  }

  async function handleReplaceFile() {
    if (!canReplace || !singleSelectedEntry || activeOperation) return;
    try {
      const sources = await invoke<string[] | null>('select_multiple_files');
      if (!sources || sources.length === 0) return;
      const sourceFile = sources[0];
      if (!sourceFile || activeOperation) return;
      const entryPath = singleSelectedEntry.path;

      await runEditOperation('Replace', (operationId) =>
        invoke<EditSummary>('replace_archive_file_command', {
          operationId,
          archivePath: currentArchivePath,
          entryPath,
          sourceFile
        })
      );
    } catch (e: any) {
      errorMessage = `Could not select replacement file: ${formatInvokeError(e)}`;
    }
  }
</script>

<div class="app-container">
  <TitleBarComponent 
    currentArchivePath={currentArchivePath} 
    operationStatus={operationStatus} 
  />

  <ToolbarComponent
    canExtract={canExtract}
    canExtractSelected={canExtractSelected}
    canExtractHere={canExtractHere}
    canTest={canTest}
    canShowProperties={canShowProperties}
    canAdd={canAdd}
    canNewFolder={canNewFolder}
    canRename={canRename}
    canDelete={canDelete}
    canReplace={canReplace}
    extractTitle={extractTitle}
    onOpenArchive={handleOpenArchive}
    onCreateArchive={handleCreateArchive}
    onExtractDefault={handleExtractDefault}
    onExtractAll={() => runExtract('all')}
    onExtractSelected={() => runExtract('selected')}
    onExtractHere={() => runExtract('here')}
    onExtractNamed={() => runExtract('named')}
    onTestArchive={handleTestArchive}
    onShowProperties={openProperties}
    onAddToArchive={handleAddToArchive}
    onNewFolder={handleNewFolder}
    onRename={handleRenameEntry}
    onDelete={handleDeleteEntries}
    onReplace={handleReplaceFile}
    onFileAssociations={openAssociationsModal}
  />

  {#if isArchiveOpen}
    <ArchiveSearchBar
      queryInput={searchInput}
      typeFilter={typeFilter}
      extension={extensionFilter}
      matchCount={matchCount}
      onQueryInput={handleSearchInput}
      onTypeFilter={(v) => (typeFilter = v)}
      onExtension={(v) => (extensionFilter = v)}
      onClear={clearSearchFilters}
    />
  {/if}

  <div
    class="main-content"
    class:drop-target-active={fileDragActive && isArchiveOpen && canAdd}
    class:drop-target-create={fileDragActive && !isArchiveOpen}
  >
    {#if fileDragActive && isArchiveOpen && canAdd}
      <div class="drop-hint monospace" aria-live="polite">
        Drop to add into {currentInternalPath === '/' || !currentInternalPath
          ? 'archive root'
          : currentInternalPath}
      </div>
    {:else if fileDragActive && !isArchiveOpen}
      <div class="drop-hint monospace" aria-live="polite">
        Drop an archive to open, or files/folders to create
      </div>
    {/if}
    {#if isArchiveOpen}
      <div class="breadcrumbs-area">
        <div class="breadcrumbs-container">
          <BreadcrumbsComponent 
            currentInternalPath={currentInternalPath} 
            onNavigate={handleNavigate} 
          />
          {#if currentInternalPath !== '/' && currentInternalPath}
            <button 
              onclick={handleNavigateBack}
              class="parent-btn monospace"
              title="Go to Parent Folder"
            >
              Up
            </button>
          {/if}
        </div>
      </div>

      {#if showRiskBanner}
        <div
          class="warning-banner monospace"
          role="region"
          aria-label="Archive risk warnings"
        >
          <div class="warning-banner-body">
            <div class="warning-banner-title">Archive warnings</div>
            <div class="warning-banner-help">Review these risks before extracting.</div>
            <ul class="warning-banner-list">
              {#each archiveWarnings as warning (warning.code + warning.message)}
                <li>{warningDisplayText(warning)}</li>
              {/each}
            </ul>
          </div>
          <button type="button" class="warning-banner-continue" onclick={acknowledgeRisks}>
            Continue
          </button>
        </div>
      {/if}

      <ArchiveTableComponent 
        visibleEntries={visibleEntries}
        archiveMode={archiveQueryActive}
        currentInternalPath={currentInternalPath}
        selectedPaths={selectedPaths}
        query={searchQuery}
        typeFilter={typeFilter}
        extension={extensionFilter}
        sortKey={sortKey}
        sortDir={sortDir}
        canEdit={canEditBase}
        onSortChange={handleSortChange}
        onNavigate={handleNavigate}
        onSelectionChange={handleSelectionChange}
        onMoveEntries={handleMoveEntries}
      />
    {:else}
      <EmptyStateComponent 
        onOpenArchive={handleOpenArchive}
        onCreateArchive={handleCreateArchive}
      />
    {/if}

    <!-- Error Alert Banner -->
    {#if errorMessage}
      <div class="error-banner monospace">
        <span class="error-banner-text">{errorMessage}</span>
        <button onclick={() => errorMessage = ''} class="error-banner-dismiss">Dismiss</button>
      </div>
    {/if}
  </div>

  <StatusBarComponent 
    itemCount={archiveEntries.length} 
    selectedItemCount={selectedPaths.size} 
    totalUncompressedSize={totalUncompressedSize}
    totalCompressedSize={isArchiveOpen ? archiveStats.total_compressed : null}
    statusText={operationStatus}
  />
</div>

<!-- Extraction Progress Modal -->
{#if showProgressModal}
  <div class="modal-overlay">
    <div class="modal-content">
      <div class="modal-header monospace">
        {#if activeOperation?.kind === 'create'}
          CREATING ARCHIVE
        {:else if activeOperation?.kind === 'test'}
          TESTING ARCHIVE
        {:else if activeOperation?.kind === 'edit'}
          EDITING ARCHIVE
        {:else}
          EXTRACTING ARCHIVE
        {/if}
      </div>
      <div class="modal-body monospace">
        <div class="progress-percent">Progress: {progressPercentage.toFixed(1)}%</div>
        <div class="progress-track">
          <div class="progress-bar" style="width: {progressPercentage}%;"></div>
        </div>
        <div class="progress-file">
          {#if progressPhase}{progressPhase} · {/if}{progressText}
        </div>
      </div>
      <div class="modal-footer">
        <button class="cancel-operation" onclick={cancelOperation} disabled={conflictPrompt !== null}>
          Cancel
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- File associations (opt-in) -->
{#if showAssocModal}
  <FileAssociationsModal
    status={assocStatus}
    busy={assocBusy}
    onEnable={enableAssociations}
    onDisable={disableAssociations}
    onRefresh={refreshAssociationStatus}
    onClose={() => (showAssocModal = false)}
  />
{/if}

<!-- Create Archive Modal -->
{#if showCreateModal}
  <CreateArchiveModal
    sources={createSources}
    format={createFormat}
    compression={createCompression}
    includeRoot={createIncludeRoot}
    overwrite={createOverwrite}
    outputPath={createOutputPath}
    busy={!!activeOperation}
    onFormat={handleCreateFormatChange}
    onCompression={(v: 'store' | 'fast' | 'normal' | 'max') => (createCompression = v)}
    onIncludeRoot={(v: boolean) => (createIncludeRoot = v)}
    onOverwrite={(v: boolean) => (createOverwrite = v)}
    onBrowseOutput={browseCreateOutput}
    onCreate={confirmCreateArchive}
    onCancel={() => (showCreateModal = false)}
  />
{/if}

<!-- Archive Properties -->
{#if showPropertiesModal}
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="props-dialog-title">
    <div class="modal-content properties-dialog">
      <div id="props-dialog-title" class="modal-header monospace">ARCHIVE PROPERTIES</div>
      <div class="modal-body monospace properties-body">
        <div class="properties-row">
          <span class="properties-label">Path</span>
          <span class="properties-value" title={currentArchivePath}>{currentArchivePath}</span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Format</span>
          <span class="properties-value">{currentArchiveFormat || '—'}</span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Files</span>
          <span class="properties-value">{archiveStats.file_count}</span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Folders</span>
          <span class="properties-value">{archiveStats.folder_count}</span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Uncompressed</span>
          <span class="properties-value">{formatBytes(archiveStats.total_uncompressed)}</span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Compressed</span>
          <span class="properties-value">{formatBytes(archiveStats.total_compressed)}</span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Ratio</span>
          <span class="properties-value">
            {#if archiveStats.total_uncompressed > 0 && archiveStats.total_compressed < archiveStats.total_uncompressed}
              {Math.round((1 - archiveStats.total_compressed / archiveStats.total_uncompressed) * 100)}%
            {:else}
              —
            {/if}
          </span>
        </div>
        <div class="properties-row">
          <span class="properties-label">Methods</span>
          <span class="properties-value">
            {archiveStats.methods.length > 0 ? archiveStats.methods.join(', ') : '—'}
          </span>
        </div>
        {#if archiveWarnings.length > 0}
          <div class="properties-row properties-warnings">
            <span class="properties-label">Warnings</span>
            <ul class="properties-warning-list">
              {#each archiveWarnings as warning (warning.code + warning.message)}
                <li>{warningDisplayText(warning)}</li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>
      <div class="modal-footer">
        <button type="button" class="primary" onclick={() => (showPropertiesModal = false)}>
          Close
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Extract Conflict Dialog (over progress modal) -->
{#if conflictPrompt}
  <div class="modal-overlay conflict-overlay" role="dialog" aria-modal="true" aria-labelledby="conflict-dialog-title">
    <div class="modal-content conflict-dialog">
      <div id="conflict-dialog-title" class="modal-header monospace">FILE CONFLICT</div>
      <div class="modal-body monospace">
        <p class="conflict-intro">A file already exists at the destination.</p>
        <div class="conflict-field">
          <span class="conflict-label">Archive entry</span>
          <span class="conflict-path" title={conflictPrompt.entry_path}>{conflictPrompt.entry_path}</span>
        </div>
        <div class="conflict-field">
          <span class="conflict-label">Destination</span>
          <span class="conflict-path" title={conflictPrompt.dest_path}>{conflictPrompt.dest_path}</span>
        </div>
        <label class="conflict-apply-all">
          <input type="checkbox" bind:checked={applyToAllChecked} />
          Apply to all remaining conflicts
        </label>
      </div>
      <div class="modal-footer conflict-actions">
        <button
          type="button"
          class="primary"
          bind:this={overwriteBtnEl}
          onclick={() => resolveConflict('overwrite')}
        >
          Overwrite
        </button>
        <button type="button" onclick={() => resolveConflict('skip')}>Skip</button>
        <button type="button" onclick={() => resolveConflict('rename')}>Rename</button>
        <button type="button" class="cancel-operation" onclick={() => resolveConflict('cancel')}>
          Cancel
        </button>
      </div>
    </div>
  </div>
{/if}
