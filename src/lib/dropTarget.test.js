// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import { isStagedDragPath, isValidMoveDest } from './dropTarget.js';

test('isStagedDragPath detects Archi temporary staging directories', () => {
  assert.equal(
    isStagedDragPath('C:\\Users\\User\\AppData\\Local\\Temp\\archi-dnd-123-456\\file.txt'),
    true
  );
  assert.equal(
    isStagedDragPath('C:/Temp/ARCHI-DND-TEST/sub/folder'),
    true
  );
  assert.equal(
    isStagedDragPath('C:\\Users\\User\\Documents\\my-file.txt'),
    false
  );
  assert.equal(isStagedDragPath(''), false);
  assert.equal(isStagedDragPath(null), false);
});

test('isValidMoveDest correctly validates move targets', () => {
  // Root is always valid
  assert.equal(isValidMoveDest(['sub/a.txt'], '/'), true);
  assert.equal(isValidMoveDest(['sub/a.txt'], ''), true);

  // Valid move into another folder
  assert.equal(isValidMoveDest(['a.txt'], 'sub'), true);
  assert.equal(isValidMoveDest(['folder1'], 'folder2'), true);

  // Cannot move into itself
  assert.equal(isValidMoveDest(['folder1'], 'folder1'), false);
  assert.equal(isValidMoveDest(['folder1/sub'], 'folder1/sub'), false);

  // Cannot move into subfolder of itself
  assert.equal(isValidMoveDest(['folder1'], 'folder1/child'), false);
  assert.equal(isValidMoveDest(['folder1'], 'folder1/child/deep'), false);

  // Empty sources
  assert.equal(isValidMoveDest([], 'folder'), false);
});
