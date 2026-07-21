# Archi Archive Roadmap Design

## Goal

Turn the current ZIP-only prototype into a fast, safe, honest Windows archive manager without rewriting the Tauri/Svelte application or changing its Niti-style interface.

## Current State

**As of 2026-07-21** (see also `docs/STATUS.md`):

| Branch | Contents |
| --- | --- |
| `master` | **Phase 1–3 complete:** ZIP full workflows, multi-format open/extract/create, ZIP edit, CLI, associations. |
| `phase3` | Historical feature branch; merged into `master` (may be deleted locally). |

Living status: **`docs/STATUS.md`**. User-facing capabilities: **`README.md`**.

Release build uses Rust `[profile.release]` (LTO, strip, opt-level 3, panic=abort) in `src-tauri/Cargo.toml`. Acrylic UI is unchanged; performance work targets backend and list runtime paths.

## Feature Matrix

| Capability | State | Notes |
| --- | --- | --- |
| ZIP open/list | **Working** | Typed IPC; content-first open. |
| Folder navigation | **Working** | Loaded metadata; breadcrumbs. |
| ZIP extraction | **Working** | Secure Windows path, cancel, progress, modes, conflicts. |
| ZIP creation | **Working** | Options (compression, include root, overwrite), atomic temp. |
| Large-list rendering | **Working** | Row virtualization + rAF scroll. |
| Search/column sorting | **Working** | Whole-archive query + column sort. |
| Selected extraction/conflicts | **Working** | Selected + overwrite/skip/rename/apply-all. |
| Test Archive/properties | **Working** | ZIP test CRC stream; properties modal. |
| Archive editing | **Working** | ZIP rebuild add/delete/rename/folder/replace. |
| TAR/TAR.GZ/GZIP/TAR.BZ2/BZIP2/TAR.XZ/XZ | **Working** | Open/list/extract; create for TAR family (+ ZIP). |
| CLI open + single-instance | **Working** | First path arg; second process forwards. |
| 7z | **Working** | Open/list/extract unencrypted; create LZMA2 (Max = level 9). Encrypted 7z deferred. |
| File associations | **Working** | Opt-in HKCU only. |
| RAR creation | Not appropriate | Proprietary; never advertise or implement creation. |
| RAR reading/extraction | Deferred | Requires documented, licensed backend. |

## Chosen Approach

Use incremental, foundation-first delivery. Preserve existing module boundaries where useful, add only shared types or operation state used by multiple commands, and verify each phase before continuing.

Rejected approaches:

- Feature-first breadth: exposes more workflows through unsafe blocking and cleanup paths.
- Full rewrite: adds migration risk without evidence that current separation prevents safe optimization.

## Architecture

### Backend

Rust owns format detection, archive I/O, path validation, operation lifecycle, progress, cancellation, capability declarations, and structured error summaries.

Blocking archive work runs outside Tauri-sensitive async execution. File contents remain streamed through fixed-size buffers. An operation registry contains only active operation IDs and `AtomicBool` cancellation flags; entries are removed on completion. Progress events are rate-limited and always end with one exact final state.

Archive edits rebuild into a temporary sibling file, flush and close it, then replace the original only after success. Cancellation and error paths remove partial files. Extraction writes each incoming file through a temporary sibling and renames it after success so failed files are not presented as complete.

### Frontend

Svelte owns one archive metadata collection, current folder, search/filter/sort state, selection, focused row, and current operation view. Opening another archive replaces the collection and rejects stale responses.

Navigation, search, and sorting use loaded metadata. A small fixed-row virtualizer renders only the visible window plus overscan. Stable entry paths remain row keys and selection identifiers. Keyboard focus is scrolled into view.

### IPC

Commands return serializable Rust structs directly; no JSON string inside Tauri JSON. Normal archives use one metadata response. Batching is added only if baseline measurements show a single response is a material bottleneck.

Long operations use a stable operation ID. Progress payloads contain operation ID, phase, processed entries, total entries, processed bytes, total bytes when known, current path, and percentage. Cancellation targets the operation ID. Passwords, if later supported, are command inputs only and never enter progress events or logs.

### Format Capabilities

Each supported format declares open, list, extract, create, edit, encrypt, and test flags. UI actions derive from these flags. Unsupported actions return the detected format and a clear reason.

Content signatures are preferred over extensions where practical. Extensions remain hints for save dialogs and ambiguous single-stream formats.

## Delivery Phases

### Phase 1: ZIP Safety and Performance Foundation

- Establish local benchmark fixtures and baseline measurements.
- Harden path validation against absolute, drive, UNC, mixed-separator, traversal, symlink, hard-link, deep-path, and archive-self-overwrite cases.
- Move blocking ZIP open/extract/create work off async command execution.
- Return typed metadata directly.
- Add active-operation cancellation and bounded progress emission.
- Make extraction and creation cleanup safe and atomic where possible.
- Add archive-bomb metadata assessment without allocating from untrusted declared sizes.
- Virtualize large folder rows while preserving selection, focus, navigation, and accessibility.
- Re-run identical measurements and commit verified results only as documentation, never machine-specific raw output.

### Phase 2: Core Archive Workflows

- Add search, file/folder and extension filters, and sortable metadata columns.
- Add Extract Selected, Extract Here, and Extract to archive-name folder.
- Add overwrite, skip, rename-incoming, and apply-to-all conflict decisions.
- Add Test Archive and archive information views.
- Add creation options that the backend honors: output, ZIP compression level, include-root behavior, overwrite decision, and bounded worker setting where useful.
- Complete file/folder drag-and-drop workflows supported reliably by Tauri.

### Phase 3: Formats, Editing, and Integration

- Add full Priority 1 TAR, TAR.GZ/TGZ, and single-stream GZIP support.
- Add safe ZIP add/delete/rename/create-folder/replace operations through archive rebuilds.
- Evaluate 7z, TAR.BZ2, TAR.XZ, BZIP2, and XZ against library stability, licensing, binary size, and test coverage; expose only verified capabilities.
- Add command-line archive opening and single-instance forwarding if Tauri support remains small and reliable.
- File associations and Explorer integration are opt-in, reversible (HKCU), and outside core archive permissions (**P3.5 done on `phase3`**).

## Security Rules

- Validate every frontend path in Rust.
- Reject parent traversal, rooted paths, Windows drive prefixes, UNC paths, alternate separators that change meaning, malformed empty names, and paths exceeding conservative depth limits.
- Canonicalize the destination root and verify every existing parent component remains inside it. Do not follow archive-provided symlinks or hard links during supported extraction.
- Never execute or automatically open extracted content.
- Prevent output archives and temporary outputs from being read as source content.
- Track declared entry count, total uncompressed size, individual size, path depth, and expansion ratio. Suspicious values produce a warning/confirmation boundary; they do not trigger large allocations.
- Preserve minimal Tauri permissions.

## Error Model

User-facing operation results contain status, successful count, failed count, skipped count, first meaningful error, and optional item details. Errors distinguish invalid path, corrupt archive, unsupported format/method, inaccessible path, disk full, conflict, cancellation, partial cleanup failure, and atomic replacement failure.

Rust internals remain in development logs only. No password or archived file contents are logged.

## Testing and Measurement

Production behavior changes follow test-first development. Backend tests programmatically generate small archives and temporary directories; no binary fixtures are committed.

Required automated coverage includes:

- Path safety variants and symlink escape.
- Empty, many-entry, Unicode, virtual-folder, missing-time, and malformed listings.
- Full/selected extraction, conflicts, cancellation, corruption, and cleanup.
- Creation/editing with files, folders, duplicate names, Unicode, empty folders, replacement, deletion, rename, cancellation, and atomic replacement.
- Format capability truthfulness.
- Frontend search/sort/selection/virtual-window calculations as pure functions.

Performance fixtures are generated locally:

- 10,000 nested small entries.
- Several large files totaling 1-5 GB when disk space permits; otherwise the actual smaller size is recorded.
- Mixed Unicode, empty, medium, and deep-path entries.

Before/after reports use identical fixtures and commands. Unavailable measurements are labeled unavailable; expected improvements are never presented as measured.

## Validation and Commits

Each logical group runs focused tests, `git diff --check`, then a Conventional Commit with a subject no longer than 50 characters. Full validation before final completion follows the project-requested command list, including frontend checks, Rust format/check/test, Tauri information, debug build, release build, and a real `tauri dev` launch when the environment permits.

Generated archives, benchmarks, installers, logs, and local scratch files remain uncommitted (see `.gitignore`).

## Non-Goals

- RAR creation.
- WinRAR recovery records or proprietary repair.
- Exact WinRAR UI cloning.
- Self-extracting executables.
- Cloud, accounts, telemetry, uploads, or unrelated file-manager behavior.
