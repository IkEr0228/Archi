// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import { buildArchiveIndexes } from './archiveIndex.js';

/** Multi-level tree fixture:
 *  / (root)
 *    docs/
 *      a.txt
 *      sub/
 *        deep.txt
 *    readme.txt
 *    empty/
 */
const tree = [
  { path: 'docs', name: 'docs', parent_path: '/', is_directory: true },
  { path: 'docs/a.txt', name: 'a.txt', parent_path: 'docs', is_directory: false },
  { path: 'docs/sub', name: 'sub', parent_path: 'docs', is_directory: true },
  {
    path: 'docs/sub/deep.txt',
    name: 'deep.txt',
    parent_path: 'docs/sub',
    is_directory: false
  },
  { path: 'readme.txt', name: 'readme.txt', parent_path: '/', is_directory: false },
  { path: 'empty', name: 'empty', parent_path: '/', is_directory: true }
];

test('byPath provides O(1) lookup for every entry', () => {
  const { byPath } = buildArchiveIndexes(tree);
  assert.equal(byPath.size, tree.length);
  for (const entry of tree) {
    assert.equal(byPath.get(entry.path), entry);
  }
  assert.equal(byPath.get('missing'), undefined);
});

test('byParent returns only direct children at root', () => {
  const { byParent } = buildArchiveIndexes(tree);
  const rootKids = byParent.get('/') ?? [];
  assert.deepEqual(
    rootKids.map((e) => e.path).sort(),
    ['docs', 'empty', 'readme.txt']
  );
  assert.ok(rootKids.every((e) => e.parent_path === '/'));
});

test('byParent returns only direct children of nested folder', () => {
  const { byParent } = buildArchiveIndexes(tree);
  const docsKids = byParent.get('docs') ?? [];
  assert.deepEqual(
    docsKids.map((e) => e.path).sort(),
    ['docs/a.txt', 'docs/sub']
  );
  // Deep file is not a direct child of docs
  assert.ok(!docsKids.some((e) => e.path === 'docs/sub/deep.txt'));
});

test('byParent for deepest folder and empty folder', () => {
  const { byParent } = buildArchiveIndexes(tree);
  const subKids = byParent.get('docs/sub') ?? [];
  assert.equal(subKids.length, 1);
  assert.equal(subKids[0].path, 'docs/sub/deep.txt');

  // empty/ has no children — key may be absent
  assert.equal(byParent.get('empty'), undefined);
});

test('null/empty parent_path normalizes to root key /', () => {
  const entries = [
    { path: 'orphan', name: 'orphan', parent_path: null },
    { path: 'also', name: 'also', parent_path: undefined },
    { path: 'empty-parent', name: 'empty-parent', parent_path: '' }
  ];
  const { byParent, byPath } = buildArchiveIndexes(entries);
  assert.equal(byPath.get('orphan').path, 'orphan');
  const rootKids = byParent.get('/') ?? [];
  assert.equal(rootKids.length, 3);
  assert.deepEqual(
    rootKids.map((e) => e.path).sort(),
    ['also', 'empty-parent', 'orphan']
  );
});

test('empty or null entries yield empty indexes', () => {
  for (const input of [[], null, undefined]) {
    const { byParent, byPath } = buildArchiveIndexes(input);
    assert.equal(byParent.size, 0);
    assert.equal(byPath.size, 0);
  }
});

test('preserves insertion order of children under a parent', () => {
  const entries = [
    { path: 'b', name: 'b', parent_path: '/' },
    { path: 'a', name: 'a', parent_path: '/' },
    { path: 'c', name: 'c', parent_path: '/' }
  ];
  const { byParent } = buildArchiveIndexes(entries);
  assert.deepEqual(
    byParent.get('/').map((e) => e.path),
    ['b', 'a', 'c']
  );
});
