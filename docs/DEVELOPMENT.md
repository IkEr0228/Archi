# Development guidelines

## Architecture

Archi has three application layers:

- `crates/archi-core`: Rust archive model, format dispatch, validation,
  extraction, creation, editing, conflicts, and security policy.
- `crates/archi-worker`: one long-lived Rust process using version-1 NDJSON over
  stdin/stdout. Frames are bounded to 1 MiB; listing responses batch at 256
  entries; one archive operation runs at a time.
- `desktop-javafx`: JavaFX controls and window lifecycle. It communicates with
  the worker over pipes and does not use JNI or WebView.

New product behavior belongs in the Rust core/worker and JavaFX shell.

## Commands

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
cargo test --workspace --offline --locked --lib --bins --tests
.\desktop-javafx\build.ps1 -Test -Smoke
.\desktop-javafx\package.ps1 -Verify
```

Use `cargo fmt -p archi-core -p archi-worker -- --check` for touched Rust
packages. Vendor formatting differences are tracked separately and should not be
mass-reformatted during feature work.

## Security rules

1. Validate every archive member path before filesystem use; reject traversal,
   absolute paths, drive paths, UNC paths, device names, and invalid components.
2. Never execute archived files or automatically open extracted content.
3. Do not follow symlinks or Windows reparse points during extraction or source
   traversal. Preserve the Windows handle-pinned path where applicable.
4. Use same-directory staging, synchronization, and atomic publication for
   replace operations. Keep rollback semantics explicit: normal-failure safe,
   not crash-safe.
5. Keep passwords owned at operation boundaries in zeroizing buffers where the
   API permits. Do not log passwords or persist them in settings.
6. Preserve archive capability policy: encrypted ZIP editing stays disabled,
   encrypted 7z editing stays supported, RAR stays read/extract-only, and TAR
   encryption stays disabled.
7. Treat user-controlled paths and worker JSON as untrusted input. Bound frame
   sizes, batch sizes, operation IDs, and concurrent operations.

## JavaFX rules

- Keep blocking archive work in Rust worker operations; never run codecs on the
  FX thread.
- Keep modal focus restoration, keyboard navigation, accessible roles/text, and
  disabled capability controls intact.
- External drag-out must stage files through Rust extraction, clean temporary
  files after bounded retention, and support cancellation/password retry.
- A new window owns its worker, executor, password session, and staging state;
  closing one window must not terminate siblings.

## Change discipline

- Follow surrounding module boundaries; avoid new dependencies unless required.
- Add a focused assertion/test for non-trivial branches and security behavior.
- Run the relevant staged suite after each phase.
- Commit each completed phase separately using Conventional Commits.
- Never include unrelated untracked files, build output, archives, or generated
  assets in a phase commit.
