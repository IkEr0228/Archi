# Development guidelines

Coding and security conventions for Archi contributors.

## Project structure

- **Goal:** Windows archive manager with a Tauri/Svelte UI.
- Keep UI components modular under `src/components`.
- Keep format and I/O logic modular under `src-tauri/src`.

## Commands

```powershell
npm run tauri dev
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
npm run test:frontend
```

## Naming

- **Svelte components:** PascalCase (e.g. `ArchiveTable.svelte`)
- **TypeScript / JavaScript:** camelCase
- **Rust:** snake_case
- **CSS:** two-space indentation

## Security (required)

1. **Path validation:** Validate all archive entry paths from untrusted input; prevent ZIP Slip and `../` traversal.
2. **Absolute paths:** Reject absolute archive entry paths.
3. **Execution:** Never execute archived files or automatically open extracted contents.
4. **Errors:** Do not use `unwrap()` / `expect()` on user-controlled Rust input; return readable errors.
5. **No link traversal:** Do not follow archive symlinks, filesystem symlinks, or Windows reparse points while extracting or reading creation sources.
6. **Creation containment:** Reject output archives that equal a source or sit inside a source directory, including temporary outputs.

## Archive IPC and operations

- Return typed, serializable Rust structs from Tauri commands (not JSON strings for IPC).
- Run blocking archive I/O only inside `tauri::async_runtime::spawn_blocking` at the command boundary.
- Long operations use a unique operation ID, rate-limited progress events, and one final state.
- Register an operation before work starts and remove it on every completion path so cancellation state cannot leak.
- Treat backend capability flags as the UI source of truth; unavailable actions stay disabled with a clear reason.

## Coding standards

- Prefer small modules over single-file “god” logic.
- Avoid unnecessary dependencies.
- Prefer Svelte 5 runes (`$state`, `$derived`, `$effect`) where they fit.
- Use Conventional Commits (e.g. `feat: …`, `fix(extract): …`); subject ≤ 50 characters when practical.

## What not to commit

- Generated archives, benchmark output, build output (`target/`, `build/`, `node_modules/`)
- Local scratch notes and temporary tooling output (see `.gitignore`)
