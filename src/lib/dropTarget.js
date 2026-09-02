/**
 * Helpers for Drag-and-Drop drop target resolution and validation.
 */

/**
 * Checks whether a given disk path is an internal Archi drag-out staging path.
 * @param {string} path
 * @returns {boolean}
 */
export function isStagedDragPath(path) {
  if (!path || typeof path !== 'string') return false;
  return path.toLowerCase().includes('archi-dnd-');
}

/**
 * Validates whether `dest` folder path is a valid move destination for `sources`.
 * Moving into itself or a subfolder of itself is invalid.
 * @param {string[]} sources
 * @param {string} dest
 * @returns {boolean}
 */
export function isValidMoveDest(sources, dest) {
  if (!sources || !sources.length) return false;
  // Root ('/' or '') is always a valid destination for moving out of subfolders
  if (!dest || dest === '/') return true;
  const normalizedDest = dest.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  return !sources.some((s) => {
    const normSource = s.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
    return normalizedDest === normSource || normalizedDest.startsWith(normSource + '/');
  });
}

/**
 * Resolves drop target folder under client coordinates (logical pixels).
 * Checks `[data-drop-folder]` and directory table rows.
 * @param {number} clientX
 * @param {number} clientY
 * @returns {string | null}
 */
export function folderUnderPoint(clientX, clientY) {
  if (typeof document === 'undefined' || !document.elementFromPoint) return null;
  const el = document.elementFromPoint(clientX, clientY);
  if (!el) return null;
  const dropEl = el.closest('[data-drop-folder]');
  if (dropEl && dropEl instanceof HTMLElement && dropEl.dataset.dropFolder != null && dropEl.dataset.dropFolder !== '') {
    return dropEl.dataset.dropFolder;
  }
  const row = el.closest('tr[data-entry-path]');
  if (row && row instanceof HTMLElement && row.dataset.isDir === 'true') {
    return row.dataset.entryPath ?? null;
  }
  return null;
}

/**
 * Highlights DOM elements matching the target folder for drop.
 * @param {string | null} dest
 */
export function setDropHighlight(dest) {
  if (typeof document === 'undefined') return;
  const nodes = document.querySelectorAll('[data-drop-folder]');
  for (let i = 0; i < nodes.length; i++) {
    const el = nodes[i];
    if (el instanceof HTMLElement) {
      const path = el.dataset.dropFolder ?? '';
      const match =
        dest != null &&
        (path === dest ||
          ((dest === '/' || dest === '') && (path === '/' || path === '')));
      el.classList.toggle('drop-folder', match);
    }
  }
}
