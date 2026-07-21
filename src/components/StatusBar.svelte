<script lang="ts">
  let {
    itemCount = 0,
    selectedItemCount = 0,
    totalUncompressedSize = 0,
    totalCompressedSize = null as number | null,
    statusText = 'Ready'
  } = $props<{
    itemCount?: number;
    selectedItemCount?: number;
    totalUncompressedSize?: number;
    /** Packed archive size on disk when known (from stats). */
    totalCompressedSize?: number | null;
    statusText?: string;
  }>();

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  }

  let formattedUncompressed = $derived(formatBytes(totalUncompressedSize));
  let packedLabel = $derived.by(() => {
    if (totalCompressedSize === null || totalCompressedSize === undefined) return null;
    return formatBytes(totalCompressedSize);
  });
  let ratioLabel = $derived.by(() => {
    if (
      totalCompressedSize === null ||
      totalCompressedSize === undefined ||
      totalUncompressedSize <= 0
    ) {
      return null;
    }
    if (totalCompressedSize >= totalUncompressedSize) return '0%';
    return `${Math.round((1 - totalCompressedSize / totalUncompressedSize) * 100)}%`;
  });
</script>

<div class="status-bar monospace" role="contentinfo">
  <span>
    <span class="prompt-user">archi@client</span><span class="prompt-path">:/archive</span><span class="prompt-symbol">$</span>
    Items: <strong id="visible-count">{itemCount}</strong> |
    Selected: <strong id="selected-count">{selectedItemCount}</strong> |
    Size: <strong id="total-size">{formattedUncompressed}</strong>
    {#if packedLabel}
      | Packed: <strong id="packed-size">{packedLabel}</strong>
    {/if}
    {#if ratioLabel}
      | Ratio: <strong id="pack-ratio">{ratioLabel}</strong>
    {/if}
  </span>
</div>

<style>
  .prompt-user {
    color: var(--pastel-rose);
    font-weight: 500;
  }
  .prompt-path {
    color: var(--pastel-lavender);
  }
  .prompt-symbol {
    color: var(--pastel-mint);
    margin-right: 8px;
  }
</style>
