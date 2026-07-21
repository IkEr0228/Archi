// @ts-nocheck
import test from 'node:test';
import assert from 'node:assert/strict';
import { getVirtualRange } from './virtualization.js';

test('limits rendered rows with overscan', () => {
  assert.deepEqual(getVirtualRange(10_000, 3500, 350, 35, 2), {
    start: 98,
    end: 112,
    top: 3430,
    bottom: 346080
  });
});

test('clamps empty and final windows', () => {
  assert.deepEqual(getVirtualRange(0, 0, 400, 35, 8), { start: 0, end: 0, top: 0, bottom: 0 });
  assert.deepEqual(getVirtualRange(5, 9999, 400, 35, 8), { start: 0, end: 5, top: 0, bottom: 0 });
});
