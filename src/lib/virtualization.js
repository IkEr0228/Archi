// @ts-check
/**
 * @param {number} itemCount
 * @param {number} scrollTop
 * @param {number} viewportHeight
 * @param {number} rowHeight
 * @param {number} overscan
 * @returns {{ start: number, end: number, top: number, bottom: number }}
 */
export function getVirtualRange(itemCount, scrollTop, viewportHeight, rowHeight, overscan) {
  if (itemCount <= 0 || rowHeight <= 0) return { start: 0, end: 0, top: 0, bottom: 0 };
  const visible = Math.ceil(Math.max(0, viewportHeight) / rowHeight);
  const maxStart = Math.max(0, itemCount - visible);
  const rawStart = Math.max(0, Math.floor(Math.max(0, scrollTop) / rowHeight) - overscan);
  const start = Math.min(rawStart, maxStart);
  const end = Math.min(itemCount, start + visible + overscan * 2);
  return { start, end, top: start * rowHeight, bottom: Math.max(0, (itemCount - end) * rowHeight) };
}
