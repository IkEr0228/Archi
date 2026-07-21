// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  canExtractArchive,
  shouldShowRiskBanner,
  extractButtonTitle,
  warningDisplayText
} from './archiveRisks.js';

const warning = { code: 'entry_count', message: 'Too many entries.' };

test('allows extract when capability true and no warnings', () => {
  assert.equal(
    canExtractArchive({
      extractCapability: true,
      warnings: [],
      risksAcknowledged: true,
      busy: false
    }),
    true
  );
});

test('denies extract when warnings unacknowledged', () => {
  assert.equal(
    canExtractArchive({
      extractCapability: true,
      warnings: [warning],
      risksAcknowledged: false,
      busy: false
    }),
    false
  );
});

test('allows extract after warnings acknowledged', () => {
  assert.equal(
    canExtractArchive({
      extractCapability: true,
      warnings: [warning],
      risksAcknowledged: true,
      busy: false
    }),
    true
  );
});

test('denies extract without capability or when busy', () => {
  assert.equal(
    canExtractArchive({
      extractCapability: false,
      warnings: [],
      risksAcknowledged: true,
      busy: false
    }),
    false
  );
  assert.equal(
    canExtractArchive({
      extractCapability: true,
      warnings: [],
      risksAcknowledged: true,
      busy: true
    }),
    false
  );
});

test('shows risk banner only for unacked warnings', () => {
  assert.equal(shouldShowRiskBanner([], false), false);
  assert.equal(shouldShowRiskBanner([warning], false), true);
  assert.equal(shouldShowRiskBanner([warning], true), false);
});

test('extract button title reflects unacked warnings only', () => {
  assert.equal(extractButtonTitle([], false), 'Extract All Files');
  assert.equal(
    extractButtonTitle([warning], false),
    'Confirm archive warnings before extracting'
  );
  assert.equal(extractButtonTitle([warning], true), 'Extract All Files');
});

test('warning display text falls back to code', () => {
  assert.equal(warningDisplayText(warning), 'Too many entries.');
  assert.equal(warningDisplayText({ code: 'path_depth', message: '' }), 'path_depth');
  assert.equal(warningDisplayText({ code: 'path_depth', message: '   ' }), 'path_depth');
});
