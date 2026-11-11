# Archi release checklist

No push is performed by this plan. Publishing requires an explicit separate
request after all gates pass.

## Pre-release gate

1. Confirm intended commit/tag and inspect `git status --short`.
2. Ensure no game/archive fixtures, secrets, local paths, `target/`, `build/`,
   `.cache/`, or unrelated untracked files are staged.
3. Run the staged Rust suite:

   ```powershell
   cargo test --workspace --offline --locked --lib --bins --tests
   ```

4. Run JavaFX/frontend checks:

   ```powershell
   .\desktop-javafx\build.ps1 -Test -Smoke
   ```

5. Build and verify the portable app image:

   ```powershell
   .\desktop-javafx\package.ps1 -Verify
   ```

6. Run `git diff --check`, inspect the final diff, and verify the packaged worker
   starts, shuts down, opens the fixture, and leaves no orphan process.

## Package and installer

`desktop-javafx/package.ps1` creates a portable JavaFX app image. Use `-Installer`
to include the current-user installer. `install.ps1` registers archive
associations and Explorer “Extract here” verbs under HKCU; administrator rights
are not required. Verify WhatIf, install, association status, extract-here, and
uninstall in a disposable user profile or test machine.

Copy the complete app image directory. Do not distribute only `Archi.exe`; the
private Java/JavaFX runtime and Rust worker are required.

## Release contents

- portable JavaFX app image
- optional per-user installer script/package
- source commit and tag
- README install/run instructions
- format and security limitations
- third-party/license notices
- test evidence for the exact package

The JavaFX app image and Rust worker are the supported product artifacts.
