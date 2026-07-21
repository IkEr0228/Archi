<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";

  const appWindow = getCurrentWindow();

  let { currentArchivePath = '', operationStatus = 'Ready' } = $props<{
    currentArchivePath?: string;
    operationStatus?: string;
  }>();

  function minimize() {
    appWindow.minimize();
  }

  function toggleMaximize() {
    appWindow.toggleMaximize();
  }

  function close() {
    appWindow.close();
  }

  let archiveName = $derived.by(() => {
    if (!currentArchivePath) return '';
    return currentArchivePath.split(/[/\\]/).pop() || currentArchivePath;
  });

  // Normalize status text (e.g. UPPERCASE terminal style)
  let statusText = $derived.by(() => {
    let clean = operationStatus.trim().toUpperCase();
    if (clean.includes('READY')) return 'READY';
    if (clean.includes('LOADED')) return clean;
    if (clean.includes('ERROR') || clean.includes('FAILED')) return 'ERROR';
    if (clean.includes('CREATING')) return 'CREATING...';
    if (clean.includes('OPENING')) return 'OPENING...';
    if (clean.includes('EXTRACTING')) return 'EXTRACTING...';
    return clean;
  });
</script>

<div class="titlebar" data-tauri-drag-region>
  <div class="app-title" data-tauri-drag-region>
    <span class="dot-indicator">●</span>
    <span class="monospace text-title title-brand">Archi // archive_manager</span>
    
    {#if archiveName}
      <span class="title-sep">|</span>
      <span class="current-archive" title={currentArchivePath}>
        Open: <span id="archived-name">{archiveName}</span>
      </span>
    {/if}
    
    <span class="status-indicator" role="status" title={operationStatus}>{statusText}</span>
  </div>
  
  <div class="window-controls">
    <button class="control-btn minimize" onclick={minimize} title="Minimize">
      <span class="btn-dot"></span>
    </button>
    <button class="control-btn maximize" onclick={toggleMaximize} title="Maximize">
      <span class="btn-dot"></span>
    </button>
    <button class="control-btn close" onclick={close} title="Close">
      <span class="btn-dot"></span>
    </button>
  </div>
</div>

<style>
  .title-brand {
    flex-shrink: 0;
    white-space: nowrap;
  }
  .title-sep {
    color: var(--border-color);
    margin: 0 8px;
    flex-shrink: 0;
  }
  .current-archive {
    font-size: 11px;
    color: var(--text-muted);
    font-weight: normal;
    min-width: 0;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .current-archive span {
    color: var(--pastel-lavender);
  }
  .status-indicator {
    font-size: 10px;
    font-weight: bold;
    color: var(--pastel-mint);
    border: 1.5px dotted var(--pastel-mint);
    background: rgba(91, 178, 165, 0.08);
    padding: 2px 6px;
    border-radius: 3px;
    letter-spacing: 0.5px;
    margin-left: 12px;
    min-width: 0;
    max-width: min(42%, 28rem);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex-shrink: 1;
  }
</style>
