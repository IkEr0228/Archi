// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import { formatInvokeError } from './invokeError.js';

test('passes through strings', () => {
  assert.equal(formatInvokeError('plain failure'), 'plain failure');
});

test('formats CommandError code and message', () => {
  assert.equal(
    formatInvokeError({ code: 'invalid_entry', message: 'Bad path.' }),
    'invalid_entry: Bad path.'
  );
});

test('includes path when present', () => {
  assert.equal(
    formatInvokeError({
      code: 'write_failed',
      message: 'Disk full.',
      path: 'C:\\out\\a.bin'
    }),
    'write_failed: Disk full. (C:\\out\\a.bin)'
  );
});

test('handles nested error object', () => {
  assert.equal(
    formatInvokeError({
      error: { code: 'conflict', message: 'Already exists.', path: 'x.txt' }
    }),
    'conflict: Already exists. (x.txt)'
  );
});

test('null and empty object fallbacks', () => {
  assert.equal(formatInvokeError(null), 'Unknown error.');
  assert.equal(formatInvokeError({}), '{}');
});
