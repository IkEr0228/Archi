// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeExtension,
  matchesSearch,
  matchesType,
  matchesExtension,
  isArchiveQueryActive,
  filterAndSortEntries
} from './archiveQuery.js';
import { buildArchiveIndexes } from './archiveIndex.js';

const entries = [
  { path: 'docs', name: 'docs', parent_path: '/', is_directory: true, uncompressed_size: 0, compressed_size: null, modified_at: '2020-01-01' },
  { path: 'docs/a.txt', name: 'a.txt', parent_path: 'docs', is_directory: false, uncompressed_size: 100, compressed_size: 40, modified_at: '2020-02-01' },
  { path: 'docs/pic.PNG', name: 'pic.PNG', parent_path: 'docs', is_directory: false, uncompressed_size: 200, compressed_size: 50, modified_at: '2020-03-01' },
  { path: 'readme.txt', name: 'readme.txt', parent_path: '/', is_directory: false, uncompressed_size: 50, compressed_size: 50, modified_at: null },
  { path: 'deep/nested/file.txt', name: 'file.txt', parent_path: 'deep/nested', is_directory: false, uncompressed_size: 10, compressed_size: 5, modified_at: '2021-01-01' }
];

const indexes = buildArchiveIndexes(entries);

test('normalizeExtension', () => {
  assert.equal(normalizeExtension('PNG'), 'png');
  assert.equal(normalizeExtension('.png'), 'png');
  assert.equal(normalizeExtension('  .Png '), 'png');
  assert.equal(normalizeExtension(''), '');
  assert.equal(normalizeExtension('   '), '');
});

test('folder mode only current children', () => {
  const rows = filterAndSortEntries({
    entries,
    currentInternalPath: '/',
    query: '',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.deepEqual(rows.map((e) => e.path), ['docs', 'readme.txt']);
});

test('search finds nested path', () => {
  const rows = filterAndSortEntries({
    entries,
    currentInternalPath: '/',
    query: 'nested',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].path, 'deep/nested/file.txt');
});

test('type filter files and folders', () => {
  const files = filterAndSortEntries({
    entries,
    currentInternalPath: '/',
    query: '',
    typeFilter: 'files',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.ok(files.every((e) => !e.is_directory));
  assert.ok(files.some((e) => e.path === 'deep/nested/file.txt'));

  const folders = filterAndSortEntries({
    entries,
    currentInternalPath: '/',
    query: '',
    typeFilter: 'folders',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.ok(folders.every((e) => e.is_directory));
  assert.equal(folders.length, 1);
  assert.equal(folders[0].path, 'docs');
});

test('extension filter', () => {
  const rows = filterAndSortEntries({
    entries,
    currentInternalPath: '/',
    query: '',
    typeFilter: 'all',
    extension: '.png',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].name, 'pic.PNG');
});

test('sort by size desc', () => {
  const rows = filterAndSortEntries({
    entries,
    currentInternalPath: 'docs',
    query: '',
    typeFilter: 'all',
    extension: '',
    sortKey: 'size',
    sortDir: 'desc'
  });
  assert.deepEqual(rows.map((e) => e.name), ['pic.PNG', 'a.txt']);
});

test('isArchiveQueryActive matrix', () => {
  assert.equal(isArchiveQueryActive({ query: '', typeFilter: 'all', extension: '' }), false);
  assert.equal(isArchiveQueryActive({ query: 'x', typeFilter: 'all', extension: '' }), true);
  assert.equal(isArchiveQueryActive({ query: '', typeFilter: 'files', extension: '' }), true);
  assert.equal(isArchiveQueryActive({ query: '', typeFilter: 'all', extension: 'txt' }), true);
});

test('matchesSearch name and path', () => {
  const e = entries[4];
  assert.equal(matchesSearch(e, 'FILE'), true);
  assert.equal(matchesSearch(e, 'deep/nested'), true);
  assert.equal(matchesSearch(e, 'zzz'), false);
  assert.equal(matchesSearch(e, ''), true);
});

test('folder mode with indexes matches full scan (root)', () => {
  const opts = {
    entries,
    currentInternalPath: '/',
    query: '',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  };
  const without = filterAndSortEntries(opts);
  const withIdx = filterAndSortEntries({ ...opts, indexes });
  assert.deepEqual(
    withIdx.map((e) => e.path),
    without.map((e) => e.path)
  );
  assert.deepEqual(withIdx.map((e) => e.path), ['docs', 'readme.txt']);
});

test('folder mode with indexes matches full scan (nested)', () => {
  const opts = {
    entries,
    currentInternalPath: 'docs',
    query: '',
    typeFilter: 'all',
    extension: '',
    sortKey: 'size',
    sortDir: 'desc'
  };
  const without = filterAndSortEntries(opts);
  const withIdx = filterAndSortEntries({ ...opts, indexes });
  assert.deepEqual(
    withIdx.map((e) => e.path),
    without.map((e) => e.path)
  );
  assert.deepEqual(withIdx.map((e) => e.name), ['pic.PNG', 'a.txt']);
});

test('folder mode with indexes does not mutate byParent lists', () => {
  const local = buildArchiveIndexes(entries);
  const before = (local.byParent.get('/') ?? []).map((e) => e.path);
  filterAndSortEntries({
    entries,
    indexes: local,
    currentInternalPath: '/',
    query: '',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'desc'
  });
  assert.deepEqual(
    (local.byParent.get('/') ?? []).map((e) => e.path),
    before
  );
});

test('archive search mode ignores byParent and still full-scans', () => {
  const rows = filterAndSortEntries({
    entries,
    indexes,
    currentInternalPath: '/',
    query: 'nested',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].path, 'deep/nested/file.txt');
});

test('empty parent slice yields empty folder view', () => {
  const rows = filterAndSortEntries({
    entries,
    indexes,
    currentInternalPath: 'missing-folder',
    query: '',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  assert.deepEqual(rows, []);
});

test('matchCount is visible length (single pipeline contract)', () => {
  const visible = filterAndSortEntries({
    entries,
    indexes,
    currentInternalPath: '/',
    query: 'txt',
    typeFilter: 'all',
    extension: '',
    sortKey: 'name',
    sortDir: 'asc'
  });
  // Same list drives table rows and matchCount — no second filter.
  const matchCount = visible.length;
  assert.ok(matchCount >= 2);
  assert.equal(matchCount, visible.length);
  assert.ok(visible.every((e) => matchesSearch(e, 'txt')));
});
