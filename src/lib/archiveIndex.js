// @ts-check

/**
 * @typedef {{ byParent: Map<string, object[]>, byPath: Map<string, object> }} ArchiveIndexes
 */

/**
 * Build O(1) path lookup and parent→children indexes for archive entries.
 * Pure data transform — no UI. Callers use byParent for folder browse
 * and byPath for random access without scanning the full list.
 *
 * Also attaches `nameLower` / `pathLower` on each entry once so archive search
 * can avoid repeated toLowerCase during filter.
 *
 * @param {Array<{ path: string, name?: string, parent_path?: string | null, nameLower?: string, pathLower?: string } & Record<string, unknown>> | null | undefined} entries
 * @returns {ArchiveIndexes}
 */
export function buildArchiveIndexes(entries) {
  const byParent = new Map();
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
