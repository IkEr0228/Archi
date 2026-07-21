// @ts-check

/**
 * @param {string} archivePath
 * @returns {string}
 */
export function zipStem(archivePath) {
  const normalized = archivePath.replace(/\\/g, '/');
  const base = normalized.split('/').filter(Boolean).pop() || '';
  const idx = base.lastIndexOf('.');
  if (idx <= 0) return base;
  return base.slice(0, idx);
}

/**
 * @param {string} archivePath
 * @returns {string | null}
 */
export function parentDir(archivePath) {
  const sep = archivePath.includes('\\') ? '\\' : '/';
  const normalized = archivePath.replace(/\\/g, '/');
  const parts = normalized.split('/').filter((p, i) => p || i === 0);
  if (parts.length <= 1) return null;
  parts.pop();
  let joined = parts.join('/');
  // Restore drive letter form C:/...
  if (/^[A-Za-z]:$/.test(parts[0]) && parts.length === 1) {
    joined = parts[0] + '/';
  }
  if (sep === '\\') return joined.replace(/\//g, '\\');
  return joined;
}

/**
 * @param {string} base
 * @param {string} child
 * @returns {string}
 */
export function joinPath(base, child) {
  const sep = base.includes('\\') ? '\\' : '/';
  const b = base.replace(/[\\/]+$/, '');
  return `${b}${sep}${child}`;
}

/**
 * @param {{ mode: 'all' | 'selected' | 'here' | 'named', archivePath: string, chosenFolder: string | null }} input
 * @returns {string | null}
 */
export function resolveExtractDestination({ mode, archivePath, chosenFolder }) {
  if (mode === 'all' || mode === 'selected') {
    return chosenFolder || null;
  }
  if (mode === 'here') {
    return parentDir(archivePath);
  }
  if (mode === 'named') {
    if (!chosenFolder) return null;
    return joinPath(chosenFolder, zipStem(archivePath));
  }
  return null;
}
