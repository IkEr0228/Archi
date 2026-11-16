# Project status

**Active branch:** `Revive`

**Default branch:** `Revive`

**Desktop:** JavaFX UI with a Rust worker

**License:** MIT

The repository is unarchived, with active development on `Revive` as the default
branch. No new release has been published. Legacy v0.5.0
Tauri assets are historical and should not be presented as current downloads.

## Platform status

| Platform | Status | Notes |
| --- | --- | --- |
| Windows x64 | Supported | Primary desktop target; OpenJDK 21 development and portable packaging are supported. |
| Linux x64 | Preview | Built on Debian 13 x64 with OpenJDK 21. Package startup is bounded validation, not full Linux support. |

The Linux preview is intentionally narrower than Windows. ZIP extraction returns
`unsupported_platform` on Linux. Other formats and operations must not be assumed
to have the same support level merely because the application packages or starts.

## Current capabilities

- Browse, search, filter, sort, inspect, and test supported archive formats.
- Create ZIP, 7z, TAR, TAR.GZ, TAR.BZ2, and TAR.XZ archives.
- Edit ZIP, 7z, and TAR-family archives where format capabilities allow it.
- Password-protected reads and ZIP/7z creation; encrypted ZIP editing is disabled,
  RAR is read/extract-only, and TAR encryption is disabled.
- Transactional extraction with path validation, progress, cancellation, conflict
  decisions, integrity checks, and rollback for normal operation failures.
- JavaFX drag-in, staged external drag-out, multiple windows, accessibility
  metadata, keyboard navigation, and current-user Windows integration.

## Validation boundaries

The repository has targeted validation, not a blanket claim that every platform
suite passes:

- Rust tests cover the core and worker, but some extraction tests assume Windows
  behavior and must not be represented as Linux support evidence.
- Java model tests and Java-to-Rust worker integration tests cover the desktop
  protocol and application model.
- JavaFX smoke coverage exercises the UI on a real display environment. On Linux,
  package startup can be checked with bounded Xvfb-based validation; this is not
  exhaustive UI automation or full Linux test coverage.
- Packaging checks confirm that the bundled worker and runtime can start and
  shut down in the tested environment. They do not prove support for every codec,
  archive, filesystem, or desktop environment.

Generated files under `dist-release/`, `target/`, `build/`, and `.cache/` are local
build output, not release artifacts or commit input.

## Architecture and security summary

`crates/archi-core` owns archive operations and security policy. `crates/archi-worker`
is a long-lived version-1 NDJSON process with bounded frames and listing batches.
`desktop-javafx` provides the JavaFX shell and communicates with the worker over
pipes without WebView or JNI.

Windows extraction uses handle-pinned no-reparse traversal. Non-Windows extraction
does not provide that guarantee. Rollback is safe for normal operation errors but
not crash/power-loss safe and cannot protect against concurrent external mutation.
Passwords are not logged or persisted; transient Java/Rust, JSON, and codec
buffers cannot all be guaranteed to be zeroized.

See [`DEVELOPMENT.md`](DEVELOPMENT.md) for change and security rules and
[`PUBLISHING.md`](PUBLISHING.md) for release policy.
