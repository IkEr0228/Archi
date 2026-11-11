# Archi

Windows archive manager with a JavaFX desktop shell, a Rust archive engine, and
a long-lived Rust NDJSON worker.

> **Desktop status:** JavaFX is the active and only supported desktop target.

## Features

- Open, browse, search, sort, test, and inspect ZIP, 7z, RAR, TAR, TAR.GZ,
  TAR.BZ2, TAR.XZ, GZ, BZ2, and XZ archives.
- Create ZIP, 7z, TAR, TAR.GZ, TAR.BZ2, and TAR.XZ archives.
- Edit ZIP, 7z, and TAR-family archives where the format capability allows it:
  add, delete, rename, move, replace, create folders, and compact.
- AES-256 password support for ZIP/7z creation and encrypted archive reads.
  Encrypted ZIP editing is intentionally disabled; RAR remains read/extract-only.
- Transactional extraction with path validation, conflict decisions, cancellation,
  progress, CRC/integrity checks, and rollback on operation failure.
- Per-user HKCU archive associations and Explorer “Extract here” integration.
- JavaFX drag-in, staged external drag-out, multiple windows, native edge snap,
  keyboard navigation, and accessibility labels/roles.

## Architecture

```text
desktop-javafx/       JavaFX controls and window lifecycle; no JNI
crates/archi-core/    format-independent Rust archive engine
crates/archi-worker/  long-lived version-1 NDJSON process
vendor/sevenz-rust2/  local 7z codec fork and pack-copy support
```

Worker protocol limits: 1 MiB frames, 256-entry listing batches, and one archive
operation at a time. JavaFX communicates with the worker over stdin/stdout; Rust
owns archive logic, validation, extraction policy, encryption policy, and archive
data handling.

## Windows build and run

Prerequisites:

- Windows x64
- JDK 21+
- Rust stable with the MSVC toolchain and Windows SDK
- PowerShell

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
.\desktop-javafx\build.ps1 -Run -Archive 'C:\archives\example.zip'
```

The first JavaFX build downloads checksum-pinned JavaFX and Gson JARs into
`.cache/javafx`. Maven and Gradle are not required.

Build a portable app image:

```powershell
.\desktop-javafx\package.ps1 -Verify
```

Copy the complete generated `Archi` directory. `package.ps1 -Installer` also
includes the current-user installer; it does not require administrator rights.

## Verification

```powershell
cargo test --workspace --offline --locked --lib --bins --tests
.\desktop-javafx\build.ps1 -Test -Smoke
git diff --check
```

The unconstrained workspace test command may try to link benchmark examples with
an existing MSVC/LLVM anonymous-symbol defect; use the staged command above.
Linux cross-checks require native `bzip2` and `unrar` toolchains.

## Security and operational limits

- Windows extraction uses handle-pinned, no-reparse staging/publication.
- Other platforms use same-directory staging, sync, and atomic rename; a parent
  symlink race remains outside that guarantee.
- Rollback protects normal operation errors, cancellation, CRC failures, and
  conflict failures. It is not crash/power-loss safe and does not protect against
  concurrent external mutation of the destination.
- Password ownership is zeroized at Rust operation boundaries and JavaFX session
  secrets use `char[]`; transient JSON/String and codec buffers cannot all be
  guaranteed zeroized by the current APIs.
- Use disposable destinations and backups when testing archive mutation or
  extraction behavior.

## Documentation

- [`desktop-javafx/README.md`](desktop-javafx/README.md) — JavaFX build, package,
  smoke, and feature details.
- [`docs/STATUS.md`](docs/STATUS.md) — migration status and known limits.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) — architecture and security rules.
- [`docs/PUBLISHING.md`](docs/PUBLISHING.md) — release checklist.

## License

Archi is MIT licensed. Third-party crates, the vendored 7z fork, and JavaFX/Gson
artifacts retain their own licenses.
