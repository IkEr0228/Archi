// @ts-check

/**
 * @typedef {import('./archiveQuery.js').ArchiveEntry} ArchiveEntry
 */

/**
 * @typedef {{ byParent: Map<string, ArchiveEntry[]>, byPath: Map<string, ArchiveEntry> }} ArchiveIndexes
 */

/**
 * Normalize parent keys so UI (`/`) and backend root (`/` or `""`) match.
 * @param {string | null | undefined} parent
 * @returns {string}
 */
export function normalizeParentPath(parent) {
  if (parent == null || parent === '' || parent === '/') return '/';
  return String(parent).replace(/\\/g, '/');
}

/**
 * Build O(1) path lookup and parent→children indexes for archive entries.
 * Pure data transform — does **not** mutate entries (safe inside Svelte `$derived`).
 *
 * @param {ArchiveEntry[] | null | undefined} entries
 * @returns {ArchiveIndexes}
 */
export function buildArchiveIndexes(entries) {
  /** @type {Map<string, ArchiveEntry[]>} */
  const byParent = new Map();
  /** @type {Map<string, ArchiveEntry>} */
  const byPath = new Map();
  for (const entry of entries || []) {
    if (!entry || typeof entry.path !== 'string') continue;
    byPath.set(entry.path, entry);
    const parent = normalizeParentPath(entry.parent_path);
    let list = byParent.get(parent);
    if (!list) {
      list = [];
      byParent.set(parent, list);
    }
    list.push(entry);
  }
  return { byParent, byPath };
}
