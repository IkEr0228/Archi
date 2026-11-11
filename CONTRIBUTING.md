# Contributing to Archi

Thanks for helping improve Archi. This document covers local setup, tests, and
how we like changes to look.

## Prerequisites (Windows)

- JDK 21+
- [Rust](https://rustup.rs/) stable (MSVC toolchain on Windows)
- Visual Studio Build Tools / C++ workload (for native crates)
- PowerShell

## Setup

```powershell
git clone https://github.com/IkEr0228/Archi.git
cd Archi
```

Fork on GitHub if you contribute from a personal copy, then clone your fork instead.

## Development

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-21.0.10'
.\desktop-javafx\build.ps1 -Run -Archive 'C:\archives\example.zip'
```

## Checks before a PR

Run what you can; CI will re-run on Windows:

```powershell
cargo fmt --all -- --check
cargo test --workspace --offline --locked --lib --bins --tests
.\desktop-javafx\build.ps1 -Test -Smoke
```

Release build (optional, slower):

```powershell
.\desktop-javafx\package.ps1 -Verify
```

## Project layout

| Path | Role |
| --- | --- |
| `desktop-javafx/` | JavaFX desktop shell, worker client, packaging, smoke tests |
| `crates/archi-core/` | Rust archive engine and regression tests |
| `crates/archi-worker/` | Long-lived NDJSON worker |
| `vendor/sevenz-rust2/` | Vendored 7z codec fork |
| `docs/STATUS.md` | Living product/dev status |
| `docs/architecture/` | Roadmap and design notes |
| `docs/DEVELOPMENT.md` | Coding and **security** conventions |

## Coding conventions

- **Java:** PascalCase classes and camelCase members
- **Rust:** snake_case
- Prefer small, focused modules over mega-files
- Do not add dependencies without a clear need
- Worker IPC: versioned NDJSON with bounded frames and typed Rust payloads
- Blocking archive I/O stays in the Rust worker, never on the JavaFX thread

## Security (required)

See `docs/DEVELOPMENT.md` and `SECURITY.md`. In particular:

- Path validation / ZIP Slip prevention
- No archive symlink extract; no reparse follow on extract/create sources
- No execute / auto-open of extracted files
- No `unwrap`/`expect` on user-controlled paths

PRs that weaken these rules will be rejected.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/):

```text
feat: add feature X
fix(extract): fix path validation edge case
docs: clarify create options
perf(ui): index archive entries by parent
```

Subject line ≤ 50 characters when practical.

## Pull requests

1. Fork and branch from `master` (or open a PR against `master`).
2. Keep the PR focused — one concern per PR when possible.
3. Add or update tests for behavior changes.
4. Do not commit: `node_modules/`, `target/`, `build/`, benchmark fixtures,
   generated archives, or local scratch files (see `.gitignore`).
5. Describe **what** and **why** in the PR body; link issues if any.

## Scope notes

- Archi targets **Windows** first (secure extract path is Windows-focused).
- Do not modify any separate “Niti” repository from this project.
- UI look (acrylic / fonts / transparency) is intentional product design;
  performance work should not silently strip it unless agreed in the PR.

## License

By contributing, you agree that your contributions are licensed under the MIT
License (see `LICENSE`).
