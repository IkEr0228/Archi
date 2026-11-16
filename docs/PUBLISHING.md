# Publishing policy

No new Archi release has been published. The repository is active on the
unarchived `Revive` branch, which is planned to be pushed and made the default
branch. Publishing requires a separate explicit decision; documentation work
does not publish, push, tag, or create a release.

Legacy v0.5.0 Tauri assets may remain in historical locations. Do not link them
as current Archi downloads. The JavaFX app image and its matching Rust worker are
the supported product artifacts for a future release.

## Release gate

Before publishing a release:

1. Confirm the intended commit/tag and inspect `git status --short`.
2. Keep secrets, local paths, fixtures, generated archives, `target/`, `build/`,
   `.cache/`, `dist-release/`, and unrelated untracked files out of the release.
3. Run the Rust suite in its documented staged form on the target platform:

   ```powershell
   cargo test --workspace --offline --locked --lib --bins --tests
   ```

   Do not describe Windows-specific extraction tests as Linux support evidence.
4. Run Java model and worker integration checks, and the relevant JavaFX smoke
   checks. On Linux, bounded Xvfb package-startup validation is useful but is not
   full Linux test coverage.
5. Build the exact artifact intended for distribution:

   ```powershell
   .\desktop-javafx\package.ps1 -Destination dist-release/windows-x64 -Verify -Installer
   ```

   On Debian 13 x64 with OpenJDK 21, use:

   ```sh
   ./desktop-javafx/package.sh dist-release/linux-x64
   ```
6. Run `git diff --check`, inspect the final diff, and record the platform,
   JDK, Rust toolchain, package path, and validation scope.

## Artifacts

### Windows

The PowerShell package creates a portable JavaFX app image. `-Installer` adds the
per-user installer script. The installer registers current-user archive
associations and Explorer “Extract here” commands; administrator rights are not
required. Distribute the complete `Archi` directory, including its private Java
runtime and Rust worker, rather than only `Archi.exe`.

### Linux preview

`package.sh` creates the app image under the supplied destination, for example:

```text
dist-release/linux-x64/Archi/bin/Archi
```

This is a Debian 13 x64 / OpenJDK 21 preview artifact. It must not be presented
as evidence of general Linux support. ZIP extraction returns
`unsupported_platform` on Linux, and other archive formats must be assessed
individually. Do not claim an installer or a universal package format unless one
has been explicitly built and verified.

## Release contents and notes

A release record should include the supported platform(s), exact artifact paths,
source commit/tag, build environment, validation commands and limits, format
capabilities, security caveats, and third-party/license notices. State clearly
when an artifact is a preview.

Generated package output remains local under `dist-release/` until an explicit
release process selects and publishes it.
