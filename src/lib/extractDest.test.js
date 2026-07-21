// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  zipStem,
  parentDir,
  joinPath,
  resolveExtractDestination
} from './extractDest.js';

test('zipStem strips final extension', () => {
  assert.equal(zipStem('C:\\data\\pack.zip'), 'pack');
  assert.equal(zipStem('/tmp/pack.zip'), 'pack');
  assert.equal(zipStem('report.backup.zip'), 'report.backup');
});

test('parentDir returns parent', () => {
  assert.equal(parentDir('C:\\data\\pack.zip'), 'C:\\data');
  assert.equal(parentDir('/tmp/pack.zip'), '/tmp');
});

test('resolveExtractDestination modes', () => {
  assert.equal(
    resolveExtractDestination({
      mode: 'all',
      archivePath: 'C:\\a\\p.zip',
      chosenFolder: 'C:\\out'
    }),
    'C:\\out'
  );
  assert.equal(
    resolveExtractDestination({
      mode: 'selected',
      archivePath: 'C:\\a\\p.zip',
      chosenFolder: 'C:\\out'
    }),
    'C:\\out'
  );
  assert.equal(
    resolveExtractDestination({
      mode: 'here',
      archivePath: 'C:\\a\\p.zip',
      chosenFolder: null
    }),
    'C:\\a'
  );
  assert.equal(
    resolveExtractDestination({
      mode: 'named',
      archivePath: 'C:\\a\\p.zip',
      chosenFolder: 'C:\\out'
    }),
    joinPath('C:\\out', 'p')
  );
});
