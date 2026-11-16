# Development guide

Archi is a JavaFX desktop application backed by a Rust archive engine and worker.
The active development line is the unarchived `Revive` branch.

## Architecture

- `crates/archi-core` contains archive format dispatch, validation, extraction,
  creation, editing, conflict handling, and security policy.
- `crates/archi-worker` is a long-lived Rust process using version-1 NDJSON over
  stdin/stdout. Frames are bounded to 1 MiB, listing responses are batched, and
  one archive operation runs at a time.
- `desktop-javafx` contains JavaFX controls and window lifecycle. It communicates
  with the worker over pipes and does not use JNI or WebView.

New product behavior belongs in the Rust core/worker and JavaFX shell rather than
in a second desktop stack.

## Build and validation commands

### Windows

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
cargo test --workspace --offline --locked --lib --bins --tests
.\desktop-javafx\build.ps1 -Test -Smoke
.\desktop-javafx\package.ps1 -Destination dist-release/windows-x64 -Verify -Installer
```

### Linux preview

The documented Linux environment is Debian 13 x64 with OpenJDK 21:

```sh
./desktop-javafx/build.sh --test
./desktop-javafx/build.sh --run
./desktop-javafx/build.sh --release
./desktop-javafx/package.sh dist-release/linux-x64
```

The packaged application is `dist-release/linux-x64/Archi/bin/Archi`. If a
headless display is needed, bounded package-startup validation may use Xvfb. Do
not report this as full Linux validation: Rust extraction tests include Windows
assumptions, and ZIP extraction intentionally returns `unsupported_platform` on
Linux. Do not generalize that limitation to other formats without testing them.

Use `cargo fmt -p archi-core -p archi-worker -- --check` for touched Rust
packages. Vendor formatting differences are tracked separately and should not be
mass-reformatted during feature work.

## Security rules

1. Validate every archive member path before filesystem use. Reject traversal,
   absolute paths, drive paths, UNC paths, device names, and invalid components.
2. Never execute archived files or automatically open extracted content.
3. Do not follow symlinks or Windows reparse points during extraction or source
   traversal. Preserve the Windows handle-pinned path where applicable.
4. Use same-directory staging, synchronization, and atomic publication for
   replacement. Rollback is normal-failure safe, not crash/power-loss safe.
5. Do not log or persist passwords. Keep owned password data within operation
   boundaries and use zeroizing buffers where the API permits; current Java/Rust,
   JSON, and codec APIs may create transient copies that cannot all be zeroized.
6. Preserve format capability policy: encrypted ZIP editing disabled, encrypted
   7z editing supported, RAR read/extract-only, and TAR encryption disabled.
7. Treat user paths and worker JSON as untrusted input. Bound frame sizes, batch
   sizes, operation IDs, and concurrent operations.

## JavaFX rules

- Keep blocking archive work in Rust worker operations; never run codecs on the FX
  thread.
- Preserve modal focus restoration, keyboard navigation, accessible roles/text,
  and disabled capability controls.
- Stage external drag-out through Rust extraction, clean temporary files after
  bounded retention, and support cancellation and password retry.
- Each window owns its worker, executor, password session, and staging state.
  Closing one window must not terminate siblings.

## Change discipline

- Follow existing module boundaries and avoid dependencies unless required.
- Add focused tests or assertions for non-trivial branches and security behavior.
- Run the narrowest relevant staged suite after each phase.
- Keep generated output, archives, fixtures, secrets, local paths, and unrelated
  files out of commits.
- Keep historical design documents when they provide context; update current
  status and operating instructions instead of rewriting history.
