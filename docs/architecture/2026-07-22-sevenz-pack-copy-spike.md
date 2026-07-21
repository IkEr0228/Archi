# SA-C1 / SA-C2: 7z pack-stream copy

**Date:** 2026-07-22  
**Branch:** `feat/incremental-edit`  
**Scope:** Research spike (SA-C1) **and** product wiring into `sevenz_edit::apply_planned` (SA-C2).

## Recommendation

# **GO** (product path wired)

Pack-stream byte-copy of unchanged non-solid 1:1 file↔block streams is feasible with a small vendor patch to `sevenz-rust2`, produces correct extractable archives, rejects solid/multi-substream layouts, preserves atomic temp publish semantics, and shows a clear speedup vs stream-rebuild on multi-megabyte members.

**SA-C2:** product `apply_planned` now tries pack-copy for eligible non-solid archives (delete / rename / move / mkdir / add / replace); falls back to `stream_rebuild` on non-cancel failure. Solid always `repack`.

## Goal proven

Byte-copy unchanged **non-solid** pack streams + re-encode only dirty members (new/replace files). Pure delete: zero re-encodes of kept members.

## Approach taken

**Option A (preferred):** local vendor of `sevenz-rust2` 0.21.3 under `vendor/sevenz-rust2` with:

| API | Purpose |
| --- | --- |
| `ArchiveWriter::push_packed_entry` | Write precompressed pack bytes + folder metadata (raw coder props) without LZMA |
| `Coder::properties` / stream counts | Read source folder coder properties for re-emit |
| `Block::num_unpack_sub_streams` / `packed_streams_count` / `unpack_sizes` | Eligibility + unpack size copy |

Archi module: `src-tauri/src/sevenz_pack_copy.rs`

- Eligibility gate (solid / multi-substream / multi-pack / multi-coder / non-LZMA2|COPY → refuse)
- `pack_stream_rebuild` — product executor (Copy + rename, NewDirectory, NewFile)
- `delete_entries_pack_copy` — spike delete helper (uses `pack_stream_rebuild`)
- Process-local `last_pack_copy_stats()` counters for S2 evidence
- **Wired** from `sevenz_edit::apply_planned` via `apply_nonsolid_planned`

## Product policy (`apply_planned`)

```
if solid → repack_edit
else if pack_copy eligible → try pack_stream_rebuild
         on non-cancel failure → stream_rebuild
else → stream_rebuild
```

| Strategy | When |
| --- | --- |
| `pack_copy` | Non-solid, eligible structure, pack path succeeds |
| `stream_rebuild` | Ineligible archive, or pack-copy failed (non-cancel) |
| `repack` | Solid archives |

Progress phase: `"pack_copy"` / `"rebuild"` / `"repack"` as appropriate.

PreferFast / PreferCompact: both may use pack-copy when eligible (pack-copy already drops deleted packs — true compact of pack region).

## Success criteria (SA-C1 spike)

| ID | Criterion | Result |
| --- | --- | --- |
| S1 | Delete 1 of 3 → remaining extract correct | **Pass** |
| S2 | Kept packs not re-LZMA’d (copy counter / timing) | **Pass** — `packs_copied=2`, `members_reencoded=0` |
| S3 | Output opens with `ArchiveReader`, `is_solid == false` | **Pass** |
| S4 | Temp publish atomic; original intact on failure | **Pass** — write-temp-only path leaves original bytes unchanged |
| S5 | Meaningful speedup if measurable | **Pass** — see timings |
| S6 | Eligibility rejects solid / multi-substream | **Pass** — solid fixture via `push_archive_entries` |

## Timings (local, dev profile opt-level 1)

Fixture: 3 × 1 MiB files, Fast (non-solid LZMA2), delete middle file.

| Strategy | Wall time (representative run) |
| --- | --- |
| `pack_copy` | **~17–24 ms** |
| `stream_rebuild` | **~136–140 ms** |

Approx. **6–8×** faster on this fixture. Pack-copy scales with compressed bytes + header write; stream-rebuild re-decodes and re-encodes every kept member.

Smaller fixtures (256 KiB × 3) also pass correctness; absolute timing delta shrinks but counters still prove copy.

## Files

| Path | Role |
| --- | --- |
| `vendor/sevenz-rust2/` | Apache-2.0 upstream 0.21.3 + Archi patch (`VENDOR.md`) |
| `src-tauri/Cargo.toml` | `path = "../vendor/sevenz-rust2"` dependency |
| `src-tauri/src/sevenz_pack_copy.rs` | Eligibility + pack-slot + `pack_stream_rebuild` |
| `src-tauri/src/sevenz_edit.rs` | Product `apply_planned` → pack-copy try / stream fallback |
| `src-tauri/tests/sevenz_pack_copy_spike.rs` | S1–S6 integration tests |
| `src-tauri/tests/sevenz_edit.rs` | Product edit tests expect `pack_copy` when eligible |

## License notes

- **Archi:** MIT  
- **sevenz-rust2 vendor:** Apache-2.0 — retain `vendor/sevenz-rust2/LICENSE` and `VENDOR.md`  
- Product binary continues to depend on sevenz-rust2; vendoring does not change license obligations. When redistributing, keep Apache-2.0 notice for the dependency.

## Eligibility rules

Refuse pack-copy when any of:

- `archive.is_solid` (any block `num_unpack_sub_streams > 1`)
- block `packed_streams_count != 1`
- block `coders.len() != 1` (multi-coder / filter chains out of scope)
- coder not LZMA2 or COPY
- AES / encrypted folders
- multi in/out stream coders

Archi create currently uses **non-solid** per-file streams for Store/Fast/Normal/Max (solid not used on create), so Archi-authored archives are typically eligible.

## Risks / follow-ups

1. **Vendor maintenance** — upgrade path requires re-applying `push_packed_entry` patch; consider upstream PR to sevenz-rust2.
2. **Metadata fidelity** — preserves size/CRC/name; timestamps/attributes from source entry are cloned but directory order may differ slightly from stream_rebuild.
3. **Dirty members** — hybrid: pack-copy keep + re-encode NewFile (add/replace).
4. **External archives** — multi-coder (BCJ+LZMA2), solid, or exotic methods correctly refused; fallback to stream_rebuild/repack.
5. **Header encryption** — sets `set_encrypt_header(false)` for simplicity.
6. **Pack CRC** — computed on copy; source pack CRCs not required to be present.
7. **Do not force corrupt paths** — on any eligibility miss, skip pack-copy (or return `pack_copy_ineligible` from spike APIs) and leave original untouched until a successful publish.

## Verify

```text
cargo test --manifest-path src-tauri/Cargo.toml --test sevenz_pack_copy_spike
cargo test --manifest-path src-tauri/Cargo.toml --test sevenz_edit
cargo test --manifest-path src-tauri/Cargo.toml
```
