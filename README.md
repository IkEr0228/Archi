# Archi

Archi is an archive manager with a JavaFX desktop application and a Rust archive
engine. Development is active on the `Revive` default branch. The repository is
no longer archived.

## Status

- **Windows x64:** supported desktop platform.
- **Linux x64:** preview build on Debian 13 with OpenJDK 21. Linux packaging is
  available for evaluation, not a claim of full platform support.
- **Releases:** no new release has been published. Legacy v0.5.0 Tauri assets may
  still exist in repository history or hosting, but they are not current Archi
  downloads.

## Features

- Browse, search, filter, sort, test, create, and edit common ZIP, 7z, RAR, TAR,
  GZ, BZ2, and XZ archives where each format supports the operation.
- Extract with path validation, conflict handling, progress, cancellation, CRC
  checks, and normal-failure rollback.
- Password-protected archive reads and ZIP/7z creation. Encrypted ZIP editing is
  disabled; RAR is read/extract-only; TAR encryption is disabled.
- JavaFX drag-in, staged drag-out, multiple windows, keyboard navigation,
  accessibility metadata, and current-user Windows Explorer integration.

## Architecture

```text
desktop-javafx/       JavaFX UI and window lifecycle
crates/archi-core/    Rust archive model, codecs, validation, and operations
crates/archi-worker/  Long-lived Rust worker using bounded v1 NDJSON over pipes
```

The UI does not use WebView or JNI. Archive logic and security policy stay in
Rust; the worker permits one archive operation at a time and bounds protocol
frames and listing batches.

## Build and run

### Windows

Requirements: Windows x64, OpenJDK 21+, Rust stable with the MSVC toolchain, the
Windows SDK, and PowerShell.

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
.\desktop-javafx\build.ps1 -Run
.\desktop-javafx\package.ps1 -Destination dist-release/windows-x64 -Verify -Installer
```

The package is a portable app image. Copy the complete `Archi` directory, not
only the executable. The optional installer is per-user and does not require
administrator rights.

### Linux preview

The preview build is built on Debian 13 x64 with OpenJDK 21. It needs a POSIX
shell, `curl`, `sha256sum`, Rust, the JDK tools, a C/C++ compiler, `pkg-config`,
and native bzip2 development headers. Running the UI requires GTK 3 and an
X11-compatible display. The locally built worker requires glibc 2.34+.

On Debian 13, install the native prerequisites with:

```sh
sudo apt-get install openjdk-21-jdk build-essential pkg-config libbz2-dev libgtk-3-0 curl
```

```sh
./desktop-javafx/build.sh --test
./desktop-javafx/build.sh --run
./desktop-javafx/build.sh --release
./desktop-javafx/package.sh dist-release/linux-x64
```

The packaged application starts at
`dist-release/linux-x64/Archi/bin/Archi`. ZIP extraction currently returns
`unsupported_platform` on Linux. Do not infer equivalent support for other
archive formats from that preview; their behavior is format- and operation-
dependent and remains under evaluation.

## Security caveats

Archive member paths are validated and extracted files are not executed
automatically. Windows extraction has handle-pinned no-reparse traversal;
non-Windows extraction does not provide that guarantee. Rollback covers normal
operation failures, cancellation, CRC, password, and conflict failures, but not
crash/power-loss recovery or concurrent external destination changes. Passwords
are not logged or persisted; transient Java/Rust, JSON, and codec copies cannot
all be guaranteed to be zeroized.

Use disposable destinations and backups while evaluating archive mutation and
extraction behavior. See [`SECURITY.md`](SECURITY.md) for the project policy.

## Documentation

- [`desktop-javafx/README.md`](desktop-javafx/README.md) — desktop build, run, and
  package details.
- [`docs/STATUS.md`](docs/STATUS.md) — current platform status and validation
  boundaries.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) — architecture and development
  rules.
- [`docs/PUBLISHING.md`](docs/PUBLISHING.md) — release gates and artifact policy.
- [`desktop-javafx/PERFORMANCE.md`](desktop-javafx/PERFORMANCE.md) — benchmarks
  and their limitations.

## License

Archi is licensed under the [MIT License](LICENSE). Third-party dependencies and
the vendored 7z fork retain their own licenses.
