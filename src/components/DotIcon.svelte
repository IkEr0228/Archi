<script lang="ts">
  interface Props {
    isDir?: boolean;
    name?: string;
    size?: number; // size in px
    type?: string; // direct icon override, e.g. "search" or "filter"
    usagePercent?: number;
  }

  let { isDir = false, name = "", size = 28, type = "", usagePercent = undefined }: Props = $props();

  // Determine file type from extension
  function getFileType(fileName: string, isDirectory: boolean): string {
    if (isDirectory) return "folder";
    const ext = fileName.split(".").pop()?.toLowerCase() || "";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico"].includes(ext)) {
      return "image";
    }
    if (["js", "ts", "html", "css", "rs", "py", "json", "md", "c", "cpp", "go", "sh", "bat", "xml", "yaml", "yml"].includes(ext)) {
      return "code";
    }
    if (["zip", "rar", "7z", "tar", "gz", "bz2", "xz"].includes(ext)) {
      return "archive";
    }
    if (["mp3", "wav", "flac", "ogg", "m4a", "mp4", "mkv", "avi", "mov"].includes(ext)) {
      return "media";
    }
    return "file";
  }

  let fileType = $derived(type || getFileType(name, isDir));

  // Define 8x8 matrices (0 = inactive dot, 1 = active dot)
  const matrices: Record<string, number[][]> = {
    folder: [
      [0, 1, 1, 1, 0, 0, 0, 0],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    file: [
      [1, 1, 1, 1, 1, 1, 0, 0],
      [1, 0, 0, 0, 0, 1, 1, 0],
      [1, 0, 0, 0, 0, 0, 1, 0],
      [1, 0, 0, 0, 0, 0, 1, 0],
      [1, 0, 0, 0, 0, 0, 1, 0],
      [1, 0, 0, 0, 0, 0, 1, 0],
      [1, 1, 1, 1, 1, 1, 1, 0],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    image: [
      [1, 1, 1, 1, 1, 1, 1, 1],
      [1, 0, 0, 0, 0, 0, 1, 1],
      [1, 0, 1, 0, 0, 0, 0, 1],
      [1, 1, 0, 1, 0, 0, 0, 1],
      [1, 0, 0, 0, 1, 0, 1, 1],
      [1, 0, 0, 1, 0, 1, 0, 1],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    code: [
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 1, 0, 0, 0, 0, 1, 0],
      [1, 0, 0, 1, 1, 0, 0, 1],
      [0, 0, 1, 0, 0, 1, 0, 0],
      [1, 0, 0, 1, 1, 0, 0, 1],
      [0, 1, 0, 0, 0, 0, 1, 0],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    archive: [
      [1, 1, 1, 1, 1, 1, 1, 1],
      [1, 1, 0, 1, 1, 0, 1, 1],
      [1, 1, 0, 1, 1, 0, 1, 1],
      [1, 1, 1, 0, 0, 1, 1, 1],
      [1, 1, 1, 0, 0, 1, 1, 1],
      [1, 1, 0, 1, 1, 0, 1, 1],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    media: [
      [0, 0, 1, 1, 1, 1, 0, 0],
      [0, 1, 0, 0, 0, 0, 1, 0],
      [1, 0, 1, 0, 0, 1, 0, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 0, 1, 1, 1, 1, 0, 1],
      [0, 1, 0, 0, 0, 0, 1, 0],
      [0, 0, 1, 1, 1, 1, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    search: [
      [0, 1, 1, 1, 0, 0, 0, 0],
      [1, 0, 0, 0, 1, 0, 0, 0],
      [1, 0, 0, 0, 1, 0, 0, 0],
      [0, 1, 1, 1, 0, 0, 0, 0],
      [0, 0, 0, 0, 1, 0, 0, 0],
      [0, 0, 0, 0, 0, 1, 0, 0],
      [0, 0, 0, 0, 0, 0, 1, 0],
      [0, 0, 0, 0, 0, 0, 0, 1]
    ],
    filter: [
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 1, 1, 1, 1, 1, 1, 0],
      [0, 0, 1, 1, 1, 1, 0, 0],
      [0, 0, 0, 1, 1, 0, 0, 0],
      [0, 0, 0, 1, 1, 0, 0, 0],
      [0, 0, 0, 1, 1, 0, 0, 0],
      [0, 0, 0, 1, 1, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    drive: [
      [0, 0, 0, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 0, 2, 0, 0, 0, 0, 1],
      [1, 0, 0, 0, 0, 0, 0, 1],
      [1, 1, 1, 1, 1, 1, 1, 1],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ],
    view: [
      [1, 1, 0, 1, 1, 0, 0, 0],
      [1, 1, 0, 1, 1, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
      [1, 1, 0, 1, 1, 0, 0, 0],
      [1, 1, 0, 1, 1, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0]
    ]
  };

  // Select matrix based on fileType, fallback to 'file'
  let matrix = $derived(matrices[fileType] || matrices.file);

  // Set colors based on fileType
  function getColor(type: string): string {
    switch (type) {
      case "search":
        return "currentColor";
      case "filter":
        return "currentColor";
      case "drive":
        return "currentColor";
      case "view":
        return "currentColor";
      case "folder":
        return "var(--pastel-lavender)";
      case "image":
        return "var(--pastel-rose)";
      case "code":
        return "var(--pastel-mint)";
      case "archive":
        return "var(--pastel-peach)";
      case "media":
        return "var(--pastel-purple)";
      default:
        return "var(--text-muted)";
    }
  }

  let activeColor = $derived(getColor(fileType));

  // Determine dynamic LED color based on disk space usage percentage
  let ledColor = $derived.by(() => {
    if (usagePercent === undefined) return activeColor;
    if (usagePercent < 50) return "var(--pastel-mint)";
    if (usagePercent < 90) return "var(--pastel-peach)";
    return "var(--pastel-rose)";
  });
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 80 80"
  class="dot-icon"
  aria-label={fileType}
>
  <!-- Only paint lit cells (was 64 circles/row → ~20 rects). Cheaper DOM/GPU. -->
  {#each matrix as row, rowIndex}
    {#each row as cell, colIndex}
      {#if cell === 1 || cell === 2}
        <rect
          x={colIndex * 10 + 1.5}
          y={rowIndex * 10 + 1.5}
          width="7"
          height="7"
          rx="2"
          fill={cell === 2 ? ledColor : activeColor}
        />
      {/if}
    {/each}
  {/each}
</svg>

<style>
  .dot-icon {
    display: inline-block;
    flex-shrink: 0;
  }
</style>
