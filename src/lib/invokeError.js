// @ts-check

/**
 * Formats Tauri invoke failures for UI banners.
 * Handles CommandError `{ code, message, path? }`, nested shapes, and plain strings.
 *
 * @param {unknown} error
 * @returns {string}
 */
export function formatInvokeError(error) {
  if (error == null) return 'Unknown error.';
  if (typeof error === 'string') return error;

  if (typeof error === 'object') {
    /** @type {Record<string, unknown>} */
    const obj = /** @type {Record<string, unknown>} */ (error);
    const nested =
      obj.error && typeof obj.error === 'object'
        ? /** @type {Record<string, unknown>} */ (obj.error)
        : null;

    const code = obj.code ?? nested?.code;
    const message =
      (typeof obj.message === 'string' && obj.message) ||
      (typeof nested?.message === 'string' && nested.message) ||
      (typeof obj.error === 'string' && obj.error) ||
      null;
    const path = obj.path ?? nested?.path;

    if (typeof message === 'string' && message.length > 0) {
      let text =
        typeof code === 'string' && code.length > 0 ? `${code}: ${message}` : message;
      if (typeof path === 'string' && path.length > 0) {
        text += ` (${path})`;
      }
      return text;
    }

    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }

  return String(error);
}
