# Archi development status

**Last updated:** 2026-07-22  
**Branch:** `master`  
**Latest release:** **v0.2.0**  
**License:** MIT (see root `LICENSE`)  
**Repository:** https://github.com/IkEr0228/Archi

## Phase overview

| Phase | Theme | Status |
| --- | --- | --- |
| **Phase 1** | ZIP safety, cancel/progress, risk gate, virtualization | **Done** |
| **Phase 2** | Extract modes, conflicts, search/filter/sort, test/properties, create options + DnD | **Done** |
| **Phase 3** | Formats, ZIP edit, CLI, multi-format create | **Done** |
| **Phase 3.5** | File associations / Explorer (opt-in) | **Done** |
| **Phase 4** | Incremental edit (ZIP append/logical delete, 7z pack-copy, TAR stream), in-archive DnD, Edit mode UI | **Done (v0.2.0)** |
| **Later** | RAR (read only if licensed), passwords UI, encrypted 7z, ZPAQ | Deferred |

## What works today

- **ZIP:** open, list, search/filter/sort, extract (modes + conflicts), create, test, properties, edit (append/logical delete/rebuild), in-archive DnD, Explorer drop-to-folder. Codecs: Stored + Deflate.
- **Create formats (dropdown order):** ZIP, **7z**, TAR, TAR.GZ, TAR.BZ2, TAR.XZ (7z defaults to Max / LZMA2-9).
- **TAR family + 7z:** open, list, extract, **test**, **edit** (stream rebuild / pack-copy / solid repack fallback). Single-stream gz/bz2/xz: open/list/extract/**test** only.
- **Edit mode:** Auto / Fast / Compact toolbar control + Compact archive action.
- **CLI:** `archi.exe path\to\archive`; single-instance forwards a second launch to the first process.
- **File associations:** opt-in per-user (HKCU) via toolbar **Associations**.
- **Not enabled:** encrypted 7z, RAR, passwords UI, single-stream create, ZPAQ.

## Docs map

| File | Role |
| --- | --- |
| **`docs/STATUS.md`** | This status snapshot |
| `docs/architecture/` | Roadmap and architecture notes |
| `docs/DEVELOPMENT.md` | Coding and security conventions |
| `README.md` | User-facing overview and capability matrix |
| `CONTRIBUTING.md` | How to contribute |
| `SECURITY.md` | Vulnerability reporting |

## Build

```powershell
npm install
npm run tauri build
```

Release Rust profile: `src-tauri/Cargo.toml` → `[profile.release]` (LTO, strip, opt-level 3, panic=abort).  
Frontend: Vite production minify.

**Last measured release build (2026-07-22, v0.2.0):**

| Artifact | Path | Size |
| --- | --- | --- |
| EXE | `src-tauri/target/release/archi_backend.exe` | **6.40 MiB** (6 715 392 bytes) |
| Installer (NSIS) | `src-tauri/target/release/bundle/nsis/archi_0.2.0_x64-setup.exe` | **1.97 MiB** (2 067 809 bytes) |

## Performance notes (UI look unchanged)

Acrylic / fonts / transparent window are preserved. Runtime work targets backend I/O and list data paths.

### Backend

| Area | Notes |
| --- | --- |
| **Selection** | Fast membership index for ZIP/TAR/7z selected extract |
| **TAR select** | Validate selection before any write |
| **open_root** | One Windows destination root handle per extract (ZIP/TAR/7z) |
| **7z dirs** | Empty directories materialize via `ensure_path` |
| **Security path** | Pre-canonical destination root |
| **Streaming** | TAR and single-stream formats avoid full payload buffers |
| **GZIP open** | Trailer ISIZE for list size when available |
| **ZIP test** | Single-pass CRC/decompress test |
| **Progress** | ~100 ms throttle on long operations |
| **I/O** | 128 KiB buffers (`io_perf`) |

### Frontend

| Area | Notes |
| --- | --- |
| **Indexes** | Parent/path maps for folder browse |
| **Query** | Single filter pipeline + match count |
| **Table** | Virtualization, low overscan, rAF scroll |
| **Progress** | Coalesced UI updates; prefer backend stats |

### Binary size

| Choice | Effect |
| --- | --- |
| Release profile | LTO, strip, opt-level 3, panic=abort |
| **zip** crate | Deflate only (no bzip2/zstd/aes-crypto features) |
| **sevenz-rust2** | Vendored path (pack-copy API); compress + util |

### Edit performance (v0.2.0)

| Format | Fast path | Notes |
| --- | --- | --- |
| ZIP add/mkdir | Append | PreferCompact → full rebuild |
| ZIP delete | Logical CD delete (PreferFast/Auto small) | Compact reclaims orphans |
| 7z non-solid | Pack-stream byte-copy | Solid → repack fallback; edit default Normal not Max |
| TAR* | Stream rebuild | No full work-tree extract |

## Possible next work

1. Encrypted 7z / passwords UI  
2. Solid 7z packing (trade-offs with cancel)  
3. Single-stream gz/bz2/xz create  
4. RAR read (licensing) / ZPAQ  

