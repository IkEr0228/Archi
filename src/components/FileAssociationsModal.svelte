<script lang="ts">
  type AssocStatus = {
    supported: boolean;
    enabled: boolean;
    associatedExtensions: string[];
    exePath: string | null;
    message: string;
  };

  type ContextMenuStatus = {
    supported: boolean;
    archiveMenuEnabled: boolean;
    filesMenuEnabled: boolean;
    message: string;
  };

  let {
    status,
    contextMenuStatus = null,
    busy = false,
    onEnable,
    onDisable,
    onEnableContextMenu,
    onDisableContextMenu,
    onRefresh,
    onClose,
  } = $props<{
    status: AssocStatus | null;
    contextMenuStatus?: ContextMenuStatus | null;
    busy?: boolean;
    onEnable: () => void;
    onDisable: () => void;
    onEnableContextMenu?: () => void;
    onDisableContextMenu?: () => void;
    onRefresh: () => void;
    onClose: () => void;
  }>();

  const extList = $derived(
    status?.associatedExtensions?.length
      ? status.associatedExtensions.map((e: string) => `.${e}`).join(", ")
      : "—"
  );

  const isMenuFullyEnabled = $derived(
    Boolean(contextMenuStatus?.archiveMenuEnabled && contextMenuStatus?.filesMenuEnabled)
  );

  const isMenuPartiallyEnabled = $derived(
    Boolean(contextMenuStatus?.archiveMenuEnabled || contextMenuStatus?.filesMenuEnabled)
  );
</script>

<div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="assoc-dialog-title">
  <div class="modal-content create-dialog">
    <div id="assoc-dialog-title" class="modal-header monospace">INTEGRATION & ASSOCIATIONS</div>
    <div class="modal-body monospace create-body">
      <p class="create-hint">
        Opt-in only. Configures file associations and Explorer context menu under your Windows user account (HKCU).
        Does not require admin privileges. 100% reversible anytime.
      </p>

      <!-- Section 1: Default File Opener -->
      <div class="section-title">FILE ASSOCIATIONS</div>
      {#if status}
        <div class="create-field">
          <span class="create-label">Status</span>
          <span class="create-value">{status.enabled ? "Enabled" : "Disabled"}</span>
        </div>
        <div class="create-field">
          <span class="create-label">Extensions</span>
          <span class="create-value" title={extList}>{extList}</span>
        </div>
        <div class="section-actions">
          <button
            type="button"
            class="action-btn"
            onclick={onDisable}
            disabled={busy || !status.supported || (!status.enabled && !status.associatedExtensions?.length)}
          >
            Disable Associations
          </button>
          <button
            type="button"
            class="action-btn primary"
            onclick={onEnable}
            disabled={busy || !status.supported}
          >
            {status.enabled ? "Update Associations" : "Enable Associations"}
          </button>
        </div>
      {:else}
        <p class="create-hint">Loading association status…</p>
      {/if}

      <hr class="section-divider" />

      <!-- Section 2: Explorer Right-Click Context Menu -->
      <div class="section-title">EXPLORER CONTEXT MENU (ПКМ)</div>
      {#if contextMenuStatus}
        <div class="create-field">
          <span class="create-label">Status</span>
          <span class="create-value">
            {#if isMenuFullyEnabled}
              Enabled (Archives & Files)
            {:else if isMenuPartiallyEnabled}
              Partially Enabled
            {:else}
              Disabled
            {/if}
          </span>
        </div>
        <div class="create-field">
          <span class="create-label">Menu Verbs</span>
          <span class="create-value" style="font-size: 0.75rem; line-height: 1.3;">
            • Archives: Open with Archi, Extract here, Extract to folder<br />
            • Files & Folders: Add to archive..., Quick .zip, Quick .7z
          </span>
        </div>
        <div class="section-actions">
          <button
            type="button"
            class="action-btn"
            onclick={onDisableContextMenu}
            disabled={busy || !contextMenuStatus.supported || !isMenuPartiallyEnabled}
          >
            Disable Context Menu
          </button>
          <button
            type="button"
            class="action-btn primary"
            onclick={onEnableContextMenu}
            disabled={busy || !contextMenuStatus.supported}
          >
            {isMenuFullyEnabled ? "Update Context Menu" : "Enable Context Menu"}
          </button>
        </div>
      {:else}
        <p class="create-hint">Loading context menu status…</p>
      {/if}
    </div>

    <div class="modal-footer assoc-footer">
      <button type="button" onclick={onRefresh} disabled={busy}>Refresh Status</button>
      <button type="button" class="primary" onclick={onClose} disabled={busy}>Close</button>
    </div>
  </div>
</div>

<style>
  .section-title {
    font-size: 0.8rem;
    font-weight: 700;
    color: var(--pastel-mint);
    letter-spacing: 0.05em;
    margin-top: 0.5rem;
    margin-bottom: 0.25rem;
  }
  .section-divider {
    border: none;
    border-top: 1px dashed rgba(255, 255, 255, 0.12);
    margin: 0.8rem 0;
  }
  .section-actions {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
    margin-top: 0.5rem;
  }
  .action-btn {
    background: var(--bg-hover);
    border: 1.5px dashed var(--border-color);
    color: var(--text-main);
    font-family: var(--font-mono);
    font-size: 11px;
    padding: 4px 10px;
    border-radius: 3px;
    cursor: pointer;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.05);
    transition: all 0.15s cubic-bezier(0.4, 0, 0.2, 1);
    outline: none;
    white-space: nowrap;
  }
  .action-btn:hover:not(:disabled) {
    color: var(--pastel-rose);
    border-color: var(--pastel-rose);
    background: var(--bg-active);
    transform: translateY(-1px);
  }
  .action-btn:active:not(:disabled) {
    transform: translateY(0);
  }
  .action-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
    transform: none;
  }
  .action-btn:focus-visible {
    outline: 1.5px dotted var(--pastel-rose);
  }
  .action-btn.primary {
    border-color: var(--pastel-rose);
    color: var(--pastel-rose);
  }
  .action-btn.primary:hover:not(:disabled) {
    border-color: var(--pastel-rose);
    color: var(--pastel-rose);
    background: var(--bg-active);
  }
  .assoc-footer {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
</style>
