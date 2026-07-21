# Security Policy

Archi is an archive manager. Path traversal, symlink abuse, and untrusted
archive contents are first-class concerns. Please report vulnerabilities
responsibly.

## Supported versions

| Version | Supported |
| --- | --- |
| `0.1.x` (current `master`) | Yes |
| Older unpublished snapshots | No |

## Reporting a vulnerability

**Do not open a public GitHub issue for security problems.**

1. Prefer a **private vulnerability report** on the GitHub repository
   (Security → Report a vulnerability), if enabled.
2. Or email the maintainer listed in the repository profile / commit history
   with a clear subject line such as `Archi security: <short title>`.

Please include:

- Archi version or commit hash
- OS version (Windows build)
- Steps to reproduce (minimal archive or script when possible)
- Impact (path escape, overwrite outside destination, crash, etc.)
- Whether a public PoC already exists

You should receive an acknowledgement when the report is seen. Coordinated
disclosure is preferred: please allow time to fix before publishing details.

## Security expectations (design)

Contributors should treat these as non-negotiable (also in `docs/DEVELOPMENT.md`):

- Validate archive entry paths (no `..`, absolute, drive, or UNC escapes)
- Do not extract archive symlinks; do not follow filesystem reparse points
  while extracting or reading create sources
- Windows extract uses handle-relative creates under a pinned destination root
- Never execute archived files or auto-open extracted contents
- Reject create output paths that equal a source or sit inside a source tree
- Handle user-controlled input without panicking (`unwrap` / `expect` on
  untrusted paths is a bug)

## Out of scope (for now)

- Encrypted / password-protected archives (no password UI yet)
- RAR and other deferred formats
- Social engineering via archive contents after a *correct* extract
