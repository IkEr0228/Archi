# SA-C1: 7z pack-stream copy research spike

**Date:** 2026-07-22  
**Branch:** `feat/incremental-edit`  
**Scope:** Research spike only — **not** wired into product `apply_planned` / `sevenz_edit` paths.

## Recommendation

# **GO**

Pack-stream byte-copy of unchanged non-solid 1:1 file↔block streams is feasible with a small vendor patch to `sevenz-rust2`, produces correct extractable archives, rejects solid/multi-substream layouts, preserves atomic temp publish semantics, and shows a clear speedup vs stream-rebuild on multi-megabyte members.

## Goal proven

Byte-copy unchanged **non-solid** pack streams + re-encode only dirty members (this spike exercises pure delete: zero re-encodes of kept members).

## Approach taken

**Option A (preferred):** local vendor of `sevenz-rust2` 0.21.3 under `vendor/sevenz-rust2` with:

| API | Purpose |
| --- | --- |
| `ArchiveWriter::push_packed_entry` | Write precompressed pack bytes + folder metadata (raw coder props) without LZMA |
| `Coder::properties` / stream counts | Read source folder coder properties for re-emit |
| `Block::num_unpack_sub_streams` / `packed_streams_count` / `unpack_sizes` | Eligibility + unpack size copy |

Archi spike module: `src-tauri/src/sevenz_pack_copy.rs`

- Eligibility gate (solid / multi-substream / multi-pack / multi-coder / non-LZMA2|COPY → refuse)
- `delete_entries_pack_copy` — spike delete using pack copy
- Process-local `last_pack_copy_stats()` counters for S2 evidence
- **Not** called from `sevenz_edit` product functions

## Success criteria

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
| `src-tauri/src/sevenz_pack_copy.rs` | Spike eligibility + pack-copy delete |
| `src-tauri/tests/sevenz_pack_copy_spike.rs` | S1–S6 integration tests |

## License notes

- **Archi:** MIT  
- **sevenz-rust2 vendor:** Apache-2.0 — retain `vendor/sevenz-rust2/LICENSE` and `VENDOR.md`  
- Product binary continues to depend on sevenz-rust2; vendoring does not change license obligations. When redistributing, keep Apache-2.0 notice for the dependency.

## Eligibility rules (spike)

Refuse pack-copy when any of:

- `archive.is_solid` (any block `num_unpack_sub_streams > 1`)
- block `packed_streams_count != 1`
- block `coders.len() != 1` (multi-coder / filter chains out of spike scope)
- coder not LZMA2 or COPY
- AES / encrypted folders
- multi in/out stream coders

Archi create currently uses **non-solid** per-file streams for Store/Fast/Normal/Max (solid not used on create), so Archi-authored archives are typically eligible.

## Risks / follow-ups before product wiring

1. **Vendor maintenance** — upgrade path requires re-applying `push_packed_entry` patch; consider upstream PR to sevenz-rust2.
2. **Metadata fidelity** — spike preserves size/CRC/name; timestamps/attributes from source entry are cloned but directory order may differ slightly from stream_rebuild.
3. **Dirty members** — pure delete only; add/replace still need re-encode path; product should hybridize: pack-copy keep + re-encode dirty.
4. **External archives** — multi-coder (BCJ+LZMA2), solid, or exotic methods correctly refused; fallback to stream_rebuild/repack.
5. **Header encryption** — spike sets `set_encrypt_header(false)` for simplicity; product may prefer matching source policy.
6. **Pack CRC** — computed on copy; source pack CRCs not required to be present.
7. **Do not force corrupt paths** — on any eligibility miss, return `pack_copy_ineligible` and leave original untouched.

## GO product path (suggested)

1. Keep vendor (or land upstream `push_packed_entry`).
2. In `sevenz_edit::apply_planned` (or delete/rename hot path): try pack-copy when eligibility + all kept members are whole-block copies with unchanged content; else stream_rebuild.
3. Rename that only changes names (same pack payload) can pack-copy with rewritten `ArchiveEntry::name`.
4. Feature-flag or `EditStrategyPref::PreferFast` gate until soak tests pass.
5. Expand tests: rename, partial folder delete, mixed add+keep.

## NO-GO would have been if

- Unable to re-emit valid folder properties without re-encode  
- Solid-only API surface forced full rewrite  
- Extract mismatch or corrupt archives under normal Fast fixtures  

None of those occurred.

## Verify

```text
cargo test --manifest-path src-tauri/Cargo.toml --test sevenz_pack_copy_spike
cargo test --manifest-path src-tauri/Cargo.toml --test sevenz_edit
```
