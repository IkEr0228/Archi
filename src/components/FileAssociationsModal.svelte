<script lang="ts">
  type Status = {
    supported: boolean;
    enabled: boolean;
    associatedExtensions: string[];
    exePath: string | null;
    message: string;
  };

  let {
    status,
    busy = false,
    onEnable,
    onDisable,
    onRefresh,
    onClose,
  } = $props<{
    status: Status | null;
    busy?: boolean;
    onEnable: () => void;
    onDisable: () => void;
    onRefresh: () => void;
    onClose: () => void;
  }>();

  const extList = $derived(
    status?.associatedExtensions?.length
      ? status.associatedExtensions.map((e) => `.${e}`).join(", ")
      : "—"
  );
</script>

<div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="assoc-dialog-title">
  <div class="modal-content create-dialog">
    <div id="assoc-dialog-title" class="modal-header monospace">FILE ASSOCIATIONS</div>
    <div class="modal-body monospace create-body">
      <p class="create-hint">
        Opt-in only. Registers Archi for archive types under your Windows user account (HKCU).
        Does not change machine-wide defaults. Reversible anytime.
      </p>
      {#if status}
        <div class="create-field">
          <span class="create-label">Platform</span>
          <span class="create-value">{status.supported ? "Windows" : "Unsupported"}</span>
        </div>
        <div class="create-field">
          <span class="create-label">Status</span>
          <span class="create-value">{status.enabled ? "Enabled" : "Disabled"}</span>
        </div>
        <div class="create-field">
          <span class="create-label">Extensions</span>
          <span class="create-value" title={extList}>{extList}</span>
        </div>
        {#if status.exePath}
          <div class="create-field">
            <span class="create-label">App path</span>
            <span class="create-value create-path" title={status.exePath}>{status.exePath}</span>
          </div>
        {/if}
        <p class="create-hint">{status.message}</p>
      {:else}
        <p class="create-hint">Loading association status…</p>
      {/if}
    </div>
    <div class="modal-footer assoc-footer">
      <button type="button" onclick={onRefresh} disabled={busy}>Refresh</button>
      <button type="button" onclick={onClose} disabled={busy}>Close</button>
      <button
        type="button"
        class="create-primary"
        onclick={onDisable}
        disabled={busy || !status?.supported || (!status?.enabled && !(status?.associatedExtensions?.length))}
      >
        Disable
      </button>
      <button
        type="button"
        class="create-primary"
        onclick={onEnable}
        disabled={busy || !status?.supported}
      >
        {status?.enabled ? "Repair / Update" : "Enable"}
      </button>
    </div>
  </div>
</div>

<style>
  .assoc-footer {
    flex-wrap: wrap;
    gap: 0.5rem;
  }
</style>
