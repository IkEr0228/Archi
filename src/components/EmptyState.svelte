<script lang="ts">
  let { onOpenArchive, onCreateArchive } = $props<{
    onOpenArchive: () => void;
    onCreateArchive: () => void;
  }>();
</script>

<div class="empty-state" role="status">
  <p>No archive loaded.</p>
  <p>Open an existing archive, create a new one (ZIP / TAR family), or drop a file here.</p>
  <div class="actions">
    <button onclick={onOpenArchive}>Open Archive</button>
    <button onclick={onCreateArchive}>Create Archive</button>
  </div>
</div>