<script lang="ts">
  let {
    title,
    onConfirm,
    onCancel,
    isCreate = false,
    archiveName = "",
    error = "",
    password = $bindable(""),
    remember = $bindable(false),
    busy = false,
  } = $props<{
    title: string;
    onConfirm: (password: string, remember: boolean) => void;
    onCancel: () => void;
    isCreate?: boolean;
    archiveName?: string;
    error?: string;
    password?: string;
    remember?: boolean;
    busy?: boolean;
  }>();
</script>

<div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="password-dialog-title">
  <div class="modal-content create-dialog password-dialog">
    <div id="password-dialog-title" class="modal-header monospace">{title}</div>
    <div class="modal-body monospace create-body">
      <p class="create-hint">
        Enter the password to {isCreate ? 'encrypt' : 'decrypt'} this archive.
      </p>
      {#if archiveName}
        <p class="create-hint password-archive" title={archiveName}>{archiveName}</p>
      {/if}
      <div class="create-field">
        <label class="create-label" for="password-input">Password</label>
        <input
          id="password-input"
          type="password"
          bind:value={password}
          placeholder="Type your password"
          class="create-input"
          disabled={busy}
        />
      </div>
      {#if error}
        <p class="password-error">{error}</p>
      {/if}
      <label class="create-check">
        <input
          type="checkbox"
          bind:checked={remember}
          disabled={busy}
        />
        Remember on this session
      </label>
      {#if !isCreate}
        <p class="create-hint">For opening existing encrypted archive.</p>
      {/if}
    </div>
    <div class="modal-footer">
      <button type="button" onclick={onCancel} disabled={busy}>Cancel</button>
      <button
        type="button"
        class="create-primary"
        onclick={() => onConfirm(password.trim(), remember)}
        disabled={busy || !password.trim()}
      >
        {isCreate ? "Create" : "Confirm"}
      </button>
    </div>
  </div>
</div>

<style>
  .create-input {
    width: 100%;
    padding: 0.5rem;
    background: #2a2a2a;
    border: 1px solid #555;
    color: #eee;
    border-radius: 0.25rem;
    font-family: monospace;
  }
  .create-check {
    margin-top: 1rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .create-check input[type="checkbox"] {
    accent-color: #00ccff;
  }
  .password-dialog p.create-hint {
    margin-bottom: 1rem;
    color: #bbb;
  }
  .password-archive {
    word-break: break-all;
    opacity: 0.7;
  }
  .password-error {
    margin-top: 0.5rem;
    color: #ff6666;
  }
</style>
