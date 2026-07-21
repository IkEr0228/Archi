// @ts-check

/**
 * @typedef {{ code: string, message: string }} ArchiveWarning
 */

/**
 * @param {{
 *   extractCapability: boolean,
 *   warnings: ArchiveWarning[],
 *   risksAcknowledged: boolean,
 *   busy: boolean
 * }} input
 * @returns {boolean}
 */
export function canExtractArchive({ extractCapability, warnings, risksAcknowledged, busy }) {
  if (!extractCapability || busy) return false;
  if (warnings.length > 0 && !risksAcknowledged) return false;
  return true;
}

/**
 * @param {ArchiveWarning[]} warnings
 * @param {boolean} risksAcknowledged
 * @returns {boolean}
 */
export function shouldShowRiskBanner(warnings, risksAcknowledged) {
  return warnings.length > 0 && !risksAcknowledged;
}

/**
 * @param {ArchiveWarning[]} warnings
 * @param {boolean} risksAcknowledged
 * @returns {string}
 */
export function extractButtonTitle(warnings, risksAcknowledged) {
  if (warnings.length > 0 && !risksAcknowledged) {
    return 'Confirm archive warnings before extracting';
  }
  return 'Extract All Files';
}

/**
 * @param {ArchiveWarning} warning
 * @returns {string}
 */
export function warningDisplayText(warning) {
  const message = typeof warning.message === 'string' ? warning.message.trim() : '';
  return message.length > 0 ? message : warning.code;
}
