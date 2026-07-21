<script lang="ts">
  type CreateFormat = "zip" | "tar" | "tarGz" | "tarBz2" | "tarXz" | "sevenZ";
  type Compression = "store" | "fast" | "normal" | "max";

  let {
    sources = [],
    format = "zip",
    compression = "normal",
    includeRoot = true,
    overwrite = false,
    outputPath = "",
    busy = false,
    onFormat,
    onCompression,
    onIncludeRoot,
    onOverwrite,
    onBrowseOutput,
    onCreate,
    onCancel,
  } = $props<{
    sources: string[];
    format: CreateFormat;
    compression: Compression;
    includeRoot: boolean;
    overwrite: boolean;
    outputPath: string;
    busy?: boolean;
    onFormat: (v: CreateFormat) => void;
    onCompression: (v: Compression) => void;
    onIncludeRoot: (v: boolean) => void;
    onOverwrite: (v: boolean) => void;
    onBrowseOutput: () => void;
    onCreate: () => void;
    onCancel: () => void;
  }>();

  const canCreate = $derived(
    !busy && sources.length > 0 && outputPath.trim().length > 0
  );
  const compressionDisabled = $derived(format === "tar" || busy);
  const usesCodecLevels = $derived(
    format === "tarGz" || format === "tarBz2" || format === "tarXz" || format === "sevenZ"
  );
</script>

<div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="create-dialog-title">
  <div class="modal-content create-dialog">
    <div id="create-dialog-title" class="modal-header monospace">CREATE ARCHIVE</div>
    <div class="modal-body monospace create-body">
      <div class="create-field">
        <span class="create-label">Sources</span>
        <span class="create-value" title={sources.join('\n')}>
          {sources.length} path{sources.length === 1 ? '' : 's'} selected
        </span>
      </div>
      <div class="create-field">
        <label class="create-label" for="create-format">Format</label>
        <select
          id="create-format"
          class="create-select"
          value={format}
          disabled={busy}
          onchange={(e) => onFormat(/** @type {any} */ (e.currentTarget.value))}
        >
          <option value="zip">ZIP</option>
          <option value="tar">TAR</option>
          <option value="tarGz">TAR.GZ</option>
          <option value="tarBz2">TAR.BZ2</option>
          <option value="tarXz">TAR.XZ</option>
          <option value="sevenZ">7z</option>
        </select>
      </div>
      <div class="create-field create-output-row">
        <span class="create-label">Output</span>
        <span class="create-value create-path" title={outputPath}>{outputPath || '(choose save path)'}</span>
        <button type="button" class="create-browse" onclick={onBrowseOutput} disabled={busy}>Browse…</button>
      </div>
      <div class="create-field">
        <label class="create-label" for="create-compression">Compression</label>
        <select
          id="create-compression"
          class="create-select"
          value={format === "tar" ? "store" : compression}
          disabled={compressionDisabled}
          onchange={(e) => onCompression(/** @type {any} */ (e.currentTarget.value))}
        >
          {#if format === "zip"}
            <option value="store">Store (no compression)</option>
            <option value="fast">Fast (deflate 1)</option>
            <option value="normal">Normal (deflate 6)</option>
            <option value="max">Max (deflate 9)</option>
          {:else if format === "tar"}
            <option value="store">Store (no compression)</option>
          {:else if format === "tarGz"}
            <option value="store">Store (gzip 0)</option>
            <option value="fast">Fast (gzip 1)</option>
            <option value="normal">Normal (gzip 6)</option>
            <option value="max">Max (gzip 9)</option>
          {:else if format === "tarBz2"}
            <option value="store">Store → Fast (bzip2 1)</option>
            <option value="fast">Fast (bzip2 1)</option>
            <option value="normal">Normal (bzip2 6)</option>
            <option value="max">Max (bzip2 9)</option>
          {:else if format === "sevenZ"}
            <option value="store">Store (LZMA2 0)</option>
            <option value="fast">Fast (LZMA2 3)</option>
            <option value="normal">Normal (LZMA2 5)</option>
            <option value="max">Max (LZMA2 9)</option>
          {:else}
            <option value="store">Store (xz 0)</option>
            <option value="fast">Fast (xz 1)</option>
            <option value="normal">Normal (xz 6)</option>
            <option value="max">Max (xz 9)</option>
          {/if}
        </select>
      </div>
      {#if format === "tar"}
        <p class="create-hint">Plain TAR stores files without compression.</p>
      {:else if format === "sevenZ"}
        <p class="create-hint">
          7z uses LZMA2. Prefer Max (level 9) for smallest archives; slower than ZIP/TAR.GZ. Encrypted 7z not supported yet.
        </p>
      {:else if usesCodecLevels}
        <p class="create-hint">
          Levels map to the codec (1 / 6 / 9). Best size among tar family: TAR.XZ Max; 7z Max is usually smallest overall. Already-compressed media barely shrinks.
        </p>
      {/if}
      <label class="create-check">
        <input
          type="checkbox"
          checked={includeRoot}
          disabled={busy}
          onchange={(e) => onIncludeRoot(e.currentTarget.checked)}
        />
        Include root folder
      </label>
      <label class="create-check">
        <input
          type="checkbox"
          checked={overwrite}
          disabled={busy}
          onchange={(e) => onOverwrite(e.currentTarget.checked)}
        />
        Overwrite if exists
      </label>
    </div>
    <div class="modal-footer">
      <button type="button" onclick={onCancel} disabled={busy}>Cancel</button>
      <button type="button" class="create-primary" onclick={onCreate} disabled={!canCreate}>
        Create
      </button>
    </div>
  </div>
</div>
