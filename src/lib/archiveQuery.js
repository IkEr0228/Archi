// @ts-check

import { normalizeParentPath } from './archiveIndex.js';

/**
 * @typedef {{
 *   path: string,
 *   name: string,
 *   parent_path: string,
 *   is_directory: boolean,
 *   uncompressed_size: number,
 *   compressed_size: number | null,
 *   modified_at: string | null,
 *   method?: string | null,
 *   nameLower?: string,
 *   pathLower?: string
 * }} ArchiveEntry
 */

/** Shared collator for name/path sorts — avoid allocating options per compare. */
const namePathCollator = new Intl.Collator(undefined, { sensitivity: 'base' });

/** @param {string} input */
export function normalizeExtension(input) {
  if (typeof input !== 'string') return '';
  let e = input.trim().toLowerCase();
  if (e.startsWith('.')) e = e.slice(1);
  return e;
}

/**
 * Lowercase name/path for search. Pure — no mutation (safe in `$derived`).
 * @param {ArchiveEntry} entry
 */
function entryNameLower(entry) {
  return (entry.name || '').toLowerCase();
}

/**
 * @param {ArchiveEntry} entry
 */
function entryPathLower(entry) {
  return (entry.path || '').toLowerCase();
}

/** @param {ArchiveEntry} entry @param {string} query */
export function matchesSearch(entry, query) {
  const q = (query || '').trim().toLowerCase();
  if (!q) return true;
  return entryNameLower(entry).includes(q) || entryPathLower(entry).includes(q);
}

/**
 * @param {ArchiveEntry} entry
 * @param {'all' | 'files' | 'folders'} typeFilter
 */
export function matchesType(entry, typeFilter) {
  if (typeFilter === 'all') return true;
  if (typeFilter === 'files') return !entry.is_directory;
  if (typeFilter === 'folders') return !!entry.is_directory;
  return true;
}

/** @param {ArchiveEntry} entry @param {string} extension */
export function matchesExtension(entry, extension) {
  const ext = normalizeExtension(extension);
  if (!ext) return true;
  // Directories have no extension; exclude them when an extension filter is active.
  if (entry.is_directory) return false;
  const name = entry.name || '';
  const idx = name.lastIndexOf('.');
  if (idx <= 0 || idx === name.length - 1) return false;
  return name.slice(idx + 1).toLowerCase() === ext;
}

/**
 * @param {{ query: string, typeFilter: string, extension: string }} p
 */
export function isArchiveQueryActive({ query, typeFilter, extension }) {
  const q = (query || '').trim();
  const ext = normalizeExtension(extension);
  return q.length > 0 || typeFilter !== 'all' || ext.length > 0;
}

/** @param {ArchiveEntry} entry */
export function entryRatio(entry) {
  if (entry.is_directory) return -1;
  const u = entry.uncompressed_size || 0;
  const c = entry.compressed_size;
  if (c === null || c === undefined || u <= 0) return -1;
  if (c >= u) return 0;
  return Math.round((1 - c / u) * 100);
}

/**
 * @param {ArchiveEntry} a
 * @param {ArchiveEntry} b
 * @param {string} sortKey
 * @param {'asc' | 'desc'} sortDir
 * @param {boolean} foldersFirst
 * @param {Map<ArchiveEntry, number> | null} [ratioByEntry] precomputed ratios for sortKey === 'ratio'
 */
export function compareEntries(a, b, sortKey, sortDir, foldersFirst, ratioByEntry = null) {
  if (foldersFirst && a.is_directory !== b.is_directory) {
    return a.is_directory ? -1 : 1;
  }
  let cmp = 0;
  switch (sortKey) {
    case 'size':
      cmp = (a.uncompressed_size || 0) - (b.uncompressed_size || 0);
      break;
    case 'compressed': {
      const ac = a.compressed_size == null ? -1 : a.compressed_size;
      const bc = b.compressed_size == null ? -1 : b.compressed_size;
      cmp = ac - bc;
      break;
    }
    case 'ratio':
      cmp =
        (ratioByEntry ? ratioByEntry.get(a) ?? entryRatio(a) : entryRatio(a)) -
        (ratioByEntry ? ratioByEntry.get(b) ?? entryRatio(b) : entryRatio(b));
      break;
    case 'modified': {
      const am = a.modified_at || '';
      const bm = b.modified_at || '';
      cmp = am < bm ? -1 : am > bm ? 1 : 0;
      break;
    }
    case 'name':
    default:
      cmp = namePathCollator.compare(a.name || '', b.name || '');
      break;
  }
  if (cmp === 0) {
    cmp = namePathCollator.compare(a.path || '', b.path || '');
  }
  return sortDir === 'desc' ? -cmp : cmp;
}

/**
 * Filter + sort archive entries for the current view.
 *
 * Folder mode (no active search/type/ext query): only direct children of
 * `currentInternalPath`. When `indexes.byParent` is provided, children are
 * taken via Map lookup (no full-list scan). Archive search mode still scans
 * all `entries` once.
 *
 * @param {{
 *   entries: ArchiveEntry[],
 *   indexes?: { byParent?: Map<string, ArchiveEntry[]>, byPath?: Map<string, ArchiveEntry> } | null,
 *   currentInternalPath: string,
 *   query: string,
 *   typeFilter: 'all' | 'files' | 'folders',
 *   extension: string,
 *   sortKey: string,
 *   sortDir: 'asc' | 'desc'
 * }} opts
 * @returns {ArchiveEntry[]}
 */
export function filterAndSortEntries(opts) {
  const {
    entries,
    indexes,
    currentInternalPath,
    query,
    typeFilter,
    extension,
    sortKey,
    sortDir
  } = opts;
  const archiveMode = isArchiveQueryActive({ query, typeFilter, extension });
  const parent = normalizeParentPath(currentInternalPath);

  /** @type {ArchiveEntry[]} */
  let source;
  let parentAlreadyScoped = false;
  if (!archiveMode && indexes?.byParent) {
    // Folder browse: O(1) children discovery — do not scan full n.
    source = indexes.byParent.get(parent) ?? [];
    // Defensive: if index missed root (key mismatch), fall back to full scan.
    if (source.length === 0 && (entries || []).length > 0) {
      source = entries || [];
      parentAlreadyScoped = false;
    } else {
      parentAlreadyScoped = true;
    }
  } else {
    source = entries || [];
  }

  // Normalize query once for the filter pass (avoids per-entry trim/toLowerCase).
  const q = (query || '').trim().toLowerCase();
  const filtered = source.filter((entry) => {
    if (
      !archiveMode &&
      !parentAlreadyScoped &&
      normalizeParentPath(entry.parent_path) !== parent
    ) {
      return false;
    }
    if (q && !entryNameLower(entry).includes(q) && !entryPathLower(entry).includes(q)) {
      return false;
    }
    if (!matchesType(entry, typeFilter)) return false;
    if (!matchesExtension(entry, extension)) return false;
    return true;
  });

  /** @type {Map<ArchiveEntry, number> | null} */
  let ratioByEntry = null;
  if (sortKey === 'ratio' && filtered.length > 1) {
    ratioByEntry = new Map();
    for (const e of filtered) {
      ratioByEntry.set(e, entryRatio(e));
    }
  }

  filtered.sort((a, b) => compareEntries(a, b, sortKey, sortDir, !archiveMode, ratioByEntry));
  return filtered;
}
