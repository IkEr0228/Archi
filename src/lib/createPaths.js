// @ts-check

/**
 * @param {string} path
 * @returns {boolean}
 */
export function isZipPath(path) {
  if (!path || typeof path !== 'string') return false;
  const normalized = path.replace(/[\\/]+$/, '');
  const base = normalized.split(/[\\/]/).pop() ?? '';
  return base.toLowerCase().endsWith('.zip');
}

/**
 * True if the path looks like an archive Archi can open (extension hint).
 * Backend still does content-based detection on open.
 * @param {string} path
 * @returns {boolean}
 */
export function isArchivePath(path) {
  if (!path || typeof path !== 'string') return false;
  const normalized = path.replace(/[\\/]+$/, '');
  const base = (normalized.split(/[\\/]/).pop() ?? '').toLowerCase();
  return (
    base.endsWith('.zip') ||
    base.endsWith('.tar') ||
    base.endsWith('.tar.gz') ||
    base.endsWith('.tgz') ||
    base.endsWith('.gz') ||
    base.endsWith('.tar.bz2') ||
    base.endsWith('.tbz2') ||
    base.endsWith('.tbz') ||
    base.endsWith('.bz2') ||
    base.endsWith('.tar.xz') ||
    base.endsWith('.txz') ||
    base.endsWith('.xz') ||
    base.endsWith('.7z')
  );
}

/** @typedef {'zip' | 'tar' | 'tarGz' | 'tarBz2' | 'tarXz' | 'sevenZ'} CreateFormat */

/**
 * @param {CreateFormat} format
 * @returns {string} extension without leading dot
 */
export function defaultExtensionForCreateFormat(format) {
  switch (format) {
    case 'tar':
      return 'tar';
    case 'tarGz':
      return 'tar.gz';
    case 'tarBz2':
      return 'tar.bz2';
    case 'tarXz':
      return 'tar.xz';
    case 'sevenZ':
      return '7z';
    case 'zip':
    default:
      return 'zip';
  }
}

/**
 * True if basename ends with a known create extension (including multi-part).
 * @param {string} base lowercased file name
 * @returns {boolean}
 */
function hasCreateExtension(base) {
  return (
    base.endsWith('.zip') ||
    base.endsWith('.tar.gz') ||
    base.endsWith('.tgz') ||
    base.endsWith('.tar.bz2') ||
    base.endsWith('.tbz2') ||
    base.endsWith('.tbz') ||
    base.endsWith('.tar.xz') ||
    base.endsWith('.txz') ||
    base.endsWith('.tar') ||
    base.endsWith('.7z')
  );
}

/**
 * Strip a known create extension from a lowercased basename; return original-case stem
 * using length of the matched extension.
 * @param {string} fileName
 * @returns {string}
 */
export function stripCreateExtension(fileName) {
  const lower = fileName.toLowerCase();
  const suffixes = [
    '.tar.gz',
    '.tar.bz2',
    '.tar.xz',
    '.tbz2',
    '.tbz',
    '.tgz',
    '.txz',
    '.tar',
    '.zip',
    '.7z',
  ];
  for (const s of suffixes) {
    if (lower.endsWith(s)) {
      return fileName.slice(0, fileName.length - s.length);
    }
  }
  return fileName;
}

/**
 * Ensure path uses the preferred extension for the create format.
 * Replaces an existing create extension when present; otherwise appends.
 * @param {string} path
 * @param {CreateFormat} format
 * @returns {string}
 */
export function withCreateExtension(path, format) {
  if (!path || typeof path !== 'string') return path;
  const normalized = path.replace(/[\\/]+$/, '');
  const slash = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  const dir = slash >= 0 ? normalized.slice(0, slash + 1) : '';
  const fileName = slash >= 0 ? normalized.slice(slash + 1) : normalized;
  if (!fileName) return path;
  const stem = stripCreateExtension(fileName);
  const ext = defaultExtensionForCreateFormat(format);
  return `${dir}${stem}.${ext}`;
}

/**
 * If path has no create extension, append the format's default.
 * Used after Save dialogs that may drop multi-part extensions on Windows.
 * @param {string} path
 * @param {CreateFormat} format
 * @returns {string}
 */
export function ensureCreateExtension(path, format) {
  if (!path || typeof path !== 'string') return path;
  const normalized = path.replace(/[\\/]+$/, '');
  const base = (normalized.split(/[\\/]/).pop() ?? '').toLowerCase();
  if (hasCreateExtension(base)) {
    // Still normalize to preferred multi-part when format needs it (e.g. user picked .tar for tar.gz filter).
    const preferred = defaultExtensionForCreateFormat(format);
    if (
      (format === 'tarGz' && (base.endsWith('.tar.gz') || base.endsWith('.tgz'))) ||
      (format === 'tarBz2' &&
        (base.endsWith('.tar.bz2') || base.endsWith('.tbz2') || base.endsWith('.tbz'))) ||
      (format === 'tarXz' && (base.endsWith('.tar.xz') || base.endsWith('.txz'))) ||
      (format === 'tar' && base.endsWith('.tar') && !base.endsWith('.tar.gz') && !base.endsWith('.tar.bz2') && !base.endsWith('.tar.xz')) ||
      (format === 'zip' && base.endsWith('.zip')) ||
      (format === 'sevenZ' && base.endsWith('.7z'))
    ) {
      return path;
    }
    // Wrong or incomplete extension for selected format → rewrite.
    return withCreateExtension(path, format);
  }
  return withCreateExtension(path, format);
}
