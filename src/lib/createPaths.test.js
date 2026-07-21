// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  defaultExtensionForCreateFormat,
  ensureCreateExtension,
  isArchivePath,
  isZipPath,
  stripCreateExtension,
  withCreateExtension,
} from './createPaths.js';

test('isZipPath detects zip extensions', () => {
  assert.equal(isZipPath('C:\\a\\b\\archive.zip'), true);
  assert.equal(isZipPath('C:/a/b/ARCHIVE.ZIP'), true);
  assert.equal(isZipPath('pack.Zip'), true);
  assert.equal(isZipPath('C:\\a\\b\\file.txt'), false);
  assert.equal(isZipPath('C:\\a\\b\\folder'), false);
  assert.equal(isZipPath(''), false);
  assert.equal(isZipPath('notzip'), false);
  assert.equal(isZipPath('something.zip.bak'), false);
});

test('isArchivePath detects supported open extensions', () => {
  assert.equal(isArchivePath('a.zip'), true);
  assert.equal(isArchivePath('a.tar'), true);
  assert.equal(isArchivePath('a.tar.gz'), true);
  assert.equal(isArchivePath('a.tgz'), true);
  assert.equal(isArchivePath('a.gz'), true);
  assert.equal(isArchivePath('a.tar.bz2'), true);
  assert.equal(isArchivePath('a.tbz2'), true);
  assert.equal(isArchivePath('a.bz2'), true);
  assert.equal(isArchivePath('a.tar.xz'), true);
  assert.equal(isArchivePath('a.txz'), true);
  assert.equal(isArchivePath('a.xz'), true);
  assert.equal(isArchivePath('a.7z'), true);
  assert.equal(isArchivePath('a.txt'), false);
  assert.equal(isArchivePath(''), false);
});

test('create format extensions', () => {
  assert.equal(defaultExtensionForCreateFormat('zip'), 'zip');
  assert.equal(defaultExtensionForCreateFormat('tarGz'), 'tar.gz');
  assert.equal(defaultExtensionForCreateFormat('tarXz'), 'tar.xz');
  assert.equal(defaultExtensionForCreateFormat('sevenZ'), '7z');
  assert.equal(stripCreateExtension('pack.tar.gz'), 'pack');
  assert.equal(stripCreateExtension('pack.ZIP'), 'pack');
  assert.equal(stripCreateExtension('pack.7z'), 'pack');
  assert.equal(withCreateExtension('C:\\out\\pack.zip', 'tarXz'), 'C:\\out\\pack.tar.xz');
  assert.equal(withCreateExtension('C:/out/pack', 'tarGz'), 'C:/out/pack.tar.gz');
  assert.equal(withCreateExtension('C:/out/pack', 'sevenZ'), 'C:/out/pack.7z');
  assert.equal(ensureCreateExtension('C:\\out\\pack.tar', 'tarGz'), 'C:\\out\\pack.tar.gz');
  assert.equal(ensureCreateExtension('C:\\out\\pack.tar.gz', 'tarGz'), 'C:\\out\\pack.tar.gz');
  assert.equal(ensureCreateExtension('C:\\out\\pack.7z', 'sevenZ'), 'C:\\out\\pack.7z');
});
