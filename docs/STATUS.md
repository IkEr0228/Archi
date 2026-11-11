# Archi status

**Active branch:** `Revive`
**Active desktop:** JavaFX + Rust worker
**License:** MIT

## Migration phases

| Phase | Status |
| --- | --- |
| Rust `archi-core` extraction of archive logic | Done |
| Long-lived NDJSON Rust worker and JavaFX vertical slice | Done |
| Transactional extraction and rollback | Done |
| HKCU associations, Explorer verbs, installer, `--extract-here` | Done |
| Password lifetime hardening | Done with documented transient-copy limits |
| JavaFX snap, accessibility, drag-in | Done |
| JavaFX multiple windows and staged external drag-out | Done |
| Root/release documentation rewrite | Done |
| Legacy Tauri/Svelte/WebView removal | Done |

## Current capabilities

- ZIP, 7z, RAR read/extract, TAR, TAR.GZ, TAR.BZ2, TAR.XZ, GZ, BZ2, and XZ
  open/list/extract/test paths as supported by each codec.
- ZIP/7z/TAR-family creation and capability-aware editing.
- Encrypted ZIP/7z reads and AES-256 creation. Encrypted ZIP editing is disabled;
  encrypted 7z editing is supported; RAR is read/extract-only.
- Selection, search, filtering, sorting, progress, cancellation, conflict
  resolution, warning acknowledgement, properties, and compact/rebuild actions.
- JavaFX file drop-in, staged file drag-out, multiple windows, edge snap, modal
  focus restoration, keyboard shortcuts, and accessibility metadata.
- Current-user Windows integration without administrator privileges.

## Known limits

- Transaction rollback is not crash/power-loss safe and cannot guard against
  concurrent external destination mutation.
- Non-Windows extraction does not provide the Windows handle-pinned no-reparse
  guarantee; parent symlink races remain a known limitation.
- Password zeroization covers owned operation/session boundaries, not every
  transient Java `String`, Gson JSON object, or codec-internal buffer.
- `cargo test --workspace --offline --locked` without target selection can hit an
  existing linker defect in `examples/edit_bench`; staged tests use
  `--lib --bins --tests`.
- The JavaFX package is the only supported desktop artifact; it has no WebView or
  JNI dependency.

## Verification commands

```powershell
cargo test --workspace --offline --locked --lib --bins --tests
.\desktop-javafx\build.ps1 -Test -Smoke
.\desktop-javafx\package.ps1 -Verify
```

No push is part of this plan. Releases require a clean worktree apart from
explicitly documented local files, a clean package verification, and a separate
release commit/tag decision.
