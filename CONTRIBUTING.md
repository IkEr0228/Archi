# Contributing to Archi

Thanks for helping improve Archi. This document covers local setup, tests, and
how we like changes to look.

## Prerequisites (Windows)

- [Node.js](https://nodejs.org/) 20+ and npm
- [Rust](https://rustup.rs/) stable (MSVC toolchain on Windows)
- Visual Studio Build Tools / C++ workload (for native crates)
- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for Windows

## Setup

```powershell
git clone https://github.com/IkEr0228/Archi.git
cd Archi
npm install
```

Fork on GitHub if you contribute from a personal copy, then clone your fork instead.

## Development

```powershell
# Desktop app (frontend + Rust backend)
npm run tauri dev
```

## Checks before a PR

Run what you can; CI will re-run on Windows:

```powershell
npm run test:frontend
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
```

Release build (optional, slower):

```powershell
npm run tauri build
```

## Project layout

| Path | Role |
| --- | --- |
| `src/` | Svelte 5 UI |
| `src-tauri/src/` | Rust backend (formats, extract, create, security) |
| `src-tauri/tests/` | Integration tests |
| `docs/STATUS.md` | Living product/dev status |
| `docs/architecture/` | Roadmap and design notes |
| `docs/DEVELOPMENT.md` | Coding and **security** conventions |

## Coding conventions

- **Svelte components:** PascalCase file names
- **TypeScript/JS:** camelCase
- **Rust:** snake_case
- **CSS:** two-space indent
- Prefer small, focused modules over mega-files
- Do not add dependencies without a clear need
- Frontend state: Svelte 5 runes (`$state`, `$derived`, `$effect`) where appropriate
- IPC: typed Rust structs (not JSON strings); blocking archive I/O only inside
  `spawn_blocking` at the command boundary

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
