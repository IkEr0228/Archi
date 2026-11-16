# Archi JavaFX desktop

The active desktop is a JavaFX shell backed by the Rust `archi-worker`. Windows
x64 is supported. Linux x64 is a preview build tested on Debian 13 with OpenJDK
21. The desktop has no WebView or JNI dependency.

## Requirements

### Windows

- Windows x64
- OpenJDK 21+
- Rust stable with the MSVC toolchain and Windows SDK
- PowerShell

### Linux preview

- Debian 13 x64
- OpenJDK 21 with `javac`, `java`, `jar`, `jlink`, and `jpackage`
- Rust and a POSIX shell
- `curl` and `sha256sum`

The first build downloads checksum-pinned JavaFX 21.0.10 and Gson 2.13.2
artifacts into the repository `.cache/javafx` directory. Maven and Gradle are not
required.

## Build and run

From the repository root:

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
.\desktop-javafx\build.ps1 -Run
.\desktop-javafx\build.ps1 -Test -Smoke
```

Use `-Archive 'C:\path\to\archive.zip'` with `-Run` to open an archive, and
`-Release` to build the release worker.

On Debian 13/Linux:

```sh
./desktop-javafx/build.sh --test
./desktop-javafx/build.sh --run
./desktop-javafx/build.sh --release
```

The shell script accepts `--archive FILE` as well as `--test`, `--smoke`, `--run`,
and `--release`.

## Packaging

Windows portable app image and optional per-user installer:

```powershell
.\desktop-javafx\package.ps1 -Destination dist-release/windows-x64 -Verify -Installer
```

The complete app image is required: do not copy only `Archi.exe`. The private
Java/JavaFX runtime and Rust worker are bundled. Existing output is not removed;
choose a new destination for another package. `-Verify` performs bounded package
startup, fixture open, worker discovery, and clean shutdown checks. `-Installer`
also copies the current-user installer script. Signing is not implied.

Linux preview package:

```sh
./desktop-javafx/package.sh dist-release/linux-x64
```

The application starts at:

```text
dist-release/linux-x64/Archi/bin/Archi
```

`package.sh` creates a Linux app image and includes a runtime. It is not a claim
that Linux is fully supported, nor does it imply a deb, rpm, Flatpak, or other
package. ZIP extraction currently returns `unsupported_platform` on Linux. Do
not assume the same result for other formats or operations without testing them.

## Validation scope

The Java model tests and Java-to-Rust worker integration tests are useful on both
platforms where their prerequisites are available. JavaFX smoke checks exercise
real UI behavior; on Linux, bounded Xvfb-based package-startup checks may be used
but are not exhaustive UI automation.

Rust extraction tests include Windows extraction assumptions. Therefore neither
`cargo test` nor a successful Linux package startup should be described as full
Linux validation. Record the exact platform and test scope with any artifact.

## Architecture

- `crates/archi-core` owns format dispatch, archive operations, validation,
  extraction, creation, editing, and security policy.
- `crates/archi-worker` is a long-lived version-1 NDJSON process over stdin/stdout.
  Frames are bounded to 1 MiB, listing responses are batched, and only one
  archive operation runs at a time.
- `desktop-javafx` owns controls, windows, keyboard/accessibility behavior, and
  worker lifecycle.

## Capabilities and limits

The application can browse, search, filter, sort, test, create, and edit supported
ZIP, 7z, RAR, TAR, GZ, BZ2, and XZ archives where the format permits the action.
It supports progress, cancellation, conflict choices, path validation, integrity
checks, and normal-failure rollback.

Encrypted ZIP editing is disabled because the editor cannot preserve encryption;
encrypted 7z editing remains supported. RAR is read/extract-only and TAR
  encryption is disabled. Passwords are not logged or persisted, but transient
Java/Rust, JSON, and codec copies cannot all be guaranteed to be zeroized.

Windows extraction uses handle-pinned no-reparse traversal. Non-Windows paths do
not have that guarantee. Rollback is not crash/power-loss safe and cannot protect
against concurrent external destination mutation. Use disposable destinations and
backups while evaluating the migration.

See [`../docs/DEVELOPMENT.md`](../docs/DEVELOPMENT.md) for architecture and
security rules, [`../docs/STATUS.md`](../docs/STATUS.md) for current platform
status, and [`../desktop-javafx/PERFORMANCE.md`](PERFORMANCE.md) for benchmark
results and limitations.
