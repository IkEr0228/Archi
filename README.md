# Archi

**Windows archive manager** — open, browse, extract, create, test, and edit archives with a focus on **safe extraction** and a modern desktop UI.

Built with [Tauri 2](https://v2.tauri.app/) (Rust backend) and [Svelte 5](https://svelte.dev/). Companion spirit to the Niti file manager; this repository is self-contained.

[![CI](https://github.com/IkEr0228/Archi/actions/workflows/ci.yml/badge.svg)](https://github.com/IkEr0228/Archi/actions/workflows/ci.yml)

## Features

- **Multi-format open/list/extract:** ZIP, TAR, TAR.GZ, GZIP, TAR.BZ2, BZIP2, TAR.XZ, XZ, 7z
- **Create:** ZIP, TAR family, and 7z (LZMA2), with shared compression presets
- **Edit:** ZIP stream rebuild; TAR family + 7z extract/repack (add, folder, rename, delete, replace)
- **Test:** all open formats — decompress/read integrity without writing user files
- **Browse UX:** virtual folders, whole-archive search, type/extension filters, column sort, virtualized table
- **Safe extract:** path validation, no archive symlink extract, no reparse traversal, Windows handle-relative writes
- **Conflicts:** overwrite / skip / rename / cancel (+ apply to all)
- **CLI + single-instance:** `archi.exe path\to\archive` opens in the running app
- **Opt-in Explorer associations** (per-user HKCU only)

## Format support

| Format | Open / list | Extract | Create | Test | Edit | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| **ZIP** | Yes | Yes | Yes | Yes | Yes | Stored + Deflate. Exotic methods may list but do not extract. Stream rebuild edit. |
| **7z** | Yes | Yes | Yes | Yes | Yes | LZMA/LZMA2; unencrypted open. Edit = extract/repack (needs free disk). |
| **TAR** | Yes | Yes | Yes | Yes | Yes | Create = store. Edit = extract/repack. |
| **TAR.GZ / TGZ** | Yes | Yes | Yes | Yes | Yes | Edit = extract/repack. |
| **TAR.BZ2 / TBZ2** | Yes | Yes | Yes | Yes | Yes | Edit = extract/repack. |
| **TAR.XZ / TXZ** | Yes | Yes | Yes | Yes | Yes | Edit = extract/repack. |
| **GZIP** (single) | Yes | Yes | No | Yes | No | Integrity stream test only. |
| **BZIP2** (single) | Yes | Yes | No | Yes | No | Integrity stream test only. |
| **XZ** (single) | Yes | Yes | No | Yes | No | Integrity stream test only. |

Capability flags from the backend drive the UI: unavailable actions stay disabled.

## Safety highlights

- Entry paths checked for traversal, absolute/drive/UNC forms, and unsafe Windows names
- Archive symlinks rejected; filesystem reparse points not followed on extract/create sources
- Extracted files are **never** executed or opened automatically
- Create rejects output paths that are a source or lie inside a selected source tree
- Long operations use operation IDs, cancellable progress, and cleanup of partial output
- Open-time risk assessment can gate extract behind an explicit **Continue** on suspicious metadata

Details: see **Extract conflict policy**, **Create options**, and **ZIP edit** sections below, plus [`SECURITY.md`](SECURITY.md).

## Requirements

- **Windows** 10/11 x64 (primary target)
- For building from source: Node.js 20+, Rust stable (MSVC), VS C++ build tools, [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)

## Build from source

```powershell
git clone https://github.com/IkEr0228/Archi.git
cd Archi
npm install
npm run tauri dev      # development
npm run tauri build    # release + NSIS installer
```

Release artifacts (after `npm run tauri build`):

| Artifact | Typical path |
| --- | --- |
| EXE | `src-tauri/target/release/archi_backend.exe` |
| Installer | `src-tauri/target/release/bundle/nsis/archi_0.1.0_x64-setup.exe` |

## Development checks

```powershell
npm run test:frontend
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
```

More detail: [`CONTRIBUTING.md`](CONTRIBUTING.md). Living status: [`docs/STATUS.md`](docs/STATUS.md).

## Command-line

```text
archi.exe path\to\archive.zip
```

Relative paths resolve against the process working directory. Archi is **single-instance**: a second launch forwards the path to the first process and exits.

## Extract conflict policy

When a destination **file already exists**:

| Choice | Behavior |
| --- | --- |
| **Overwrite** | Replace the regular file via secure temp + rename |
| **Skip** | Leave existing; count as skipped |
| **Rename** | Write as `stem (n).ext` |
| **Cancel** | Stop and clean partials |
| **Apply to all** | Remember Overwrite/Skip/Rename for this operation only |

Hard fails (no modal): destination symlink/reparse, file↔directory conflicts, duplicate plan destinations.

## Create archive options

| Option | Default | Behavior |
| --- | --- | --- |
| **Format** | ZIP (picker) | ZIP, TAR family, 7z |
| **Compression** | Normal | Store / Fast / Normal / Max (mapped per codec) |
| **Include root folder** | On | Directory sources keep their folder name at archive root |
| **Overwrite if exists** | Off | On: replace existing **regular file** only |

### Drag-and-drop

| Drop | Result |
| --- | --- |
| Exactly one path ending in `.zip` | Open that archive |
| Any other non-empty drop | Open Create with those paths as sources |

## Edit archive

- **ZIP:** stream rebuild into a temporary sibling, then atomic replace (Windows `MoveFileEx`).
- **TAR family + 7z:** extract to a temp work folder, apply the change, recreate the archive, replace the original. Needs free disk roughly the size of the unpacked tree.
- **Single-stream GZIP/BZIP2/XZ:** no multi-entry edit.

Cancel/error paths remove partial temps and leave the original intact when possible.

| Action | Behavior |
| --- | --- |
| **Add** | Files/folders under current virtual folder |
| **New Folder** | Empty directory entry |
| **Rename** | File or folder (prefix rewrite for folders) |
| **Delete** | Selection + recursive folder prefix |
| **Replace** | One file’s content from disk |

## File associations (opt-in)

Toolbar **Associations** registers Archi under **HKCU** only (not machine-wide, not installer-default). Reversible from the same dialog.

## Limitations

- No RAR, no encrypted 7z / passwords UI, no single-stream create for raw `.gz`/`.bz2`/`.xz`
- ZIP methods beyond Stored/Deflate are not decompressed
- ZIP edit rewrites the whole archive per operation
- No archive repair, multi-volume, or drag-into-open-archive yet
- Secure extract path is Windows-focused

## Documentation map

| Doc | Purpose |
| --- | --- |
| [`docs/STATUS.md`](docs/STATUS.md) | Phase / status snapshot |
| [`docs/architecture/`](docs/architecture/) | Roadmap / architecture |
| [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) | Coding and security conventions |
| [`SECURITY.md`](SECURITY.md) | Vulnerability reporting |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | How to contribute |

## License

[MIT](LICENSE) © 2026 [IKER](https://github.com/IkEr0228)

Third-party crates and npm packages remain under their own licenses (see `Cargo.lock` / `package-lock.json`).
