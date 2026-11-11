# JavaFX desktop

Windows x64, JDK 21+, Rust/MSVC toolchain. JavaFX uses `javafx.controls`, not WebView.
Archive operations remain in Rust. JavaFX is the active desktop target; the old
Tauri/Svelte application remains only as a temporary compatibility surface.

## Run

From the repository root in PowerShell:

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
.\desktop-javafx\build.ps1 -Run -Archive 'C:\archives\example.zip'
```

Omit `-Archive` for an empty window. Add `-Release` for an optimized worker.
First build downloads checksum-pinned JavaFX 21.0.10 Windows and Gson 2.13.2 JARs
from Maven Central into `.cache/javafx`. Java classes and smoke screenshots live in
`desktop-javafx/build/` (ignored). Maven/Gradle are not required.

## Portable Windows application

```powershell
.\desktop-javafx\package.ps1 -Verify
```

Builds a release worker and `dist-release/javafx/Archi/Archi.exe` with a private
trimmed Java/JavaFX runtime. Copy the **whole Archi directory**, not only its EXE.
Installed Java is not required. Existing output is never removed; use
`-Destination 'dist-release/javafx-next'` for another build. `-SkipBuild` reuses
compiled classes/release worker. `-Verify` opens a fixture from an unrelated working
directory, checks the bundled worker and closes the application without orphans.
This is an app image. `-Installer` also copies `install.ps1`, which installs the app
image per-user and registers archive associations plus Explorer “Extract here”
commands under HKCU. `install.ps1 -Uninstall -Package <package>` removes those
entries and (unless `-KeepFiles` is used) the installed app. Signing remains pending.

## Verify

```powershell
cargo test --offline --locked --workspace
cargo test --offline --locked --manifest-path src-tauri/Cargo.toml
cargo fmt -p archi-core -p archi-worker -- --check
.\desktop-javafx\build.ps1 -Test -Smoke
npm run test:frontend
```

`-Test`: Java model/query/framing assertions plus real Java → Rust worker integration.
`-Smoke`: launches the real UI twice; verifies listing/search/modal blur, create,
folder/rename/add/delete/move/compact, refresh preserving query/selection/folder,
modal bounds at 800×600 and 720×480, and encrypted RAR header password focus,
wrong-password retry, then successful opening. Also covers all resize directions,
encrypted 7z mutation password retry/session memory and disabled encrypted ZIP edits.
Produces `smoke-*.png`. This is not exhaustive UI
automation or pixel-perfect visual acceptance.

## Implemented

- Independent `crates/archi-core`; legacy adapter re-exports its modules.
- Long-lived `archi-worker`, version 1 NDJSON stdin/stdout. Frames bounded to 1 MiB,
  listing batches up to 256 entries, one archive operation at a time. Cancel,
  conflict decisions and shutdown remain available during operations.
- Open/list, indexed folder navigation, archive-wide search/type/extension filters,
  sorting, multiple selection, file drop to open, properties.
- Password prompt/retry and optional session-only remembering; password spaces preserved.
- Extract selected/all, warning acknowledgement, progress, cancellation,
  overwrite/skip/rename/cancel conflict choices and apply-to-all; archive testing.
- Dark theme, dotted icons/borders, sharp modal over disabled/blurred background.
- Create ZIP/7z/TAR/TAR.GZ/TAR.BZ2/TAR.XZ, compression and include-root options,
  explicit overwrite confirmation, ZIP/7z passwords; TAR encryption is rejected.
- Add files/folders, new folder, rename, delete, move, replace and compact; edit
  strategy selection, mutation progress/cancel and automatic listing refresh.
- Fixed modal header/actions with content-only scrolling; long source paths wrap.
- Resize from all four edges and corners, preserving opposite-edge anchoring.
- Worker process regressions exercise all create formats and edit actions,
  encrypted 7z edit, encrypted ZIP rejection, and original-byte/temp cleanup checks
  for invalid mutations and immediate cancellation.

## Known limits

Multiple JavaFX windows, staged external drag-out, native edge snap, drag-in, and
accessibility roles/text are implemented. Cross-window archive-to-archive moves
are still copy/extract workflows, not a single shared in-memory model.

The legacy Tauri/Svelte tree remains until the final parity gate and clean release
verification. It is not the active release UI.

Encrypted ZIP edits are deliberately disabled: the engine's ZIP editor does not
preserve encryption. Encrypted 7z edits remain supported with password retry.

Windows extraction now uses handle-pinned, no-reparse writes. Single-stream codecs
and RAR decode into staging, check cancellation/CRC, sync, then publish without
deleting the old file first. RAR streams native callbacks into those handles;
the native library receives no destination path and does not buffer whole files.
TAR/7z defer replacement of existing files until decoding finishes; TAR codecs are
drained past TAR end blocks to validate their trailers. Duplicate replacements keep
last-entry-wins behavior. Extraction now snapshots destination and restores it on
codec, CRC, password, cancellation, or conflict failure. This is rollback-safe,
not crash/power-loss safe; concurrent external destination mutation is unsupported.

These guarantees have Windows regressions, plus non-Windows staging/sync/publish
coverage. Non-Windows paths use same-directory temporary files and atomic rename for
overwrite, so codec/CRC/cancellation failures do not destroy the old regular file.
They do not have Windows' handle-pinned no-reparse traversal; a parent symlink race
between preflight and publish remains outside this verification scope. Use backups
and disposable destinations while evaluating this migration; it is not yet a
production replacement. Listing construction inside the core is not interruptible;
cancel is checked while delivering listing batches. Codec calls can delay shutdown;
the Java client terminates an unresponsive worker after its shutdown timeout.

Passwords are not logged or written to settings. Optional session memory uses JVM
strings; complete zeroization of all JVM/Rust copies is not guaranteed.

## Performance

See [`PERFORMANCE.md`](PERFORMANCE.md). No general speedup claim. Shared edit planners,
indexed collisions, buffered ZIP rebuilds and reuse of already-open ZIP metadata
reduce specific edit costs; the worker no longer performs a redundant full listing
before editing. Validation, encrypted ZIP rejection, CRC and sync remain enabled.
Engine dependencies match the legacy
lockfile baseline. The architecture graph in `graphify-out/` describes the pre-move
snapshot and is not the current file map.
