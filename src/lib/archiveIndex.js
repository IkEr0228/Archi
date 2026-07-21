// @ts-check

/**
 * @typedef {import('./archiveQuery.js').ArchiveEntry} ArchiveEntry
 */

/**
 * @typedef {{ byParent: Map<string, ArchiveEntry[]>, byPath: Map<string, ArchiveEntry> }} ArchiveIndexes
 */

/**
 * Build O(1) path lookup and parent→children indexes for archive entries.
 * Pure data transform — no UI. Callers use byParent for folder browse
 * and byPath for random access without scanning the full list.
 *
 * Also attaches `nameLower` / `pathLower` on each entry once so archive search
 * can avoid repeated toLowerCase during filter.
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
    // Search lowercase caches (idempotent if rebuild runs again).
    entry.nameLower = (entry.name || '').toLowerCase();
    entry.pathLower = (entry.path || '').toLowerCase();
    byPath.set(entry.path, entry);
    const parent = entry.parent_path ?? '';
    let list = byParent.get(parent);
    if (!list) {
      list = [];
      byParent.set(parent, list);
    }
    list.push(entry);
  }
  return { byParent, byPath };
}
