# Archive benchmarks

Latest listing/query comparison: [2026-11-16 results](../benchmarks/RESULTS.md),
including raw samples, reproduction commands, and measurement limitations.

## Initial codec measurement (historical)

Run from repository root:

```powershell
cargo run --offline --locked --release -p archi-worker --example archive_bench -- 5
```

Harness creates deterministic disposable corpora, then runs ZIP, TAR.GZ and 7z with
normal compression. One warmup and five measured runs per case. Uses mimalloc,
release profile, same locked codec versions as the legacy backend. Every extracted
file is compared byte-for-byte outside timed sections. Temporary data is removed.
Creation includes sync/publish logic. These historical extraction measurements were
taken **before** the later staging/sync/CRC publication changes; they are not a
baseline for current extraction throughput. Warm-cache local measurements, not
cold-start UI tests or power-loss durability guarantees.

Initial Windows x64 results, Intel Core i7-13700H (14 cores / 20 logical processors),
Rust 1.96.1, November 11 2026. Times are p50 milliseconds.

| Corpus | Format | Create before | Create after | Extract before | Extract after |
|---|---|---:|---:|---:|---:|
| 1000 × 1 KiB text | ZIP | 165.13 | 161.51 | 270.31 | 285.64 |
| 1000 × 1 KiB text | TAR.GZ | 714.43 | 754.29 | 1305.32 | 1350.42 |
| 1000 × 1 KiB text | 7z | 23694.48 | 20667.45 | 2774.03 | 2852.78 |
| 1 × 8 MiB text | ZIP | 23.67 | 23.77 | 9.81 | 10.27 |
| 1 × 8 MiB text | TAR.GZ | 31.26 | 28.88 | 9.92 | 7.87 |
| 1 × 8 MiB text | 7z | 1049.05 | 1065.46 | 26.24 | 26.34 |
| 1 × 8 MiB pseudorandom | ZIP | 532.59 | 524.41 | 22.96 | 23.64 |
| 1 × 8 MiB pseudorandom | TAR.GZ | 522.52 | 545.93 | 18.39 | 14.72 |
| 1 × 8 MiB pseudorandom | 7z | 4943.93 | 5114.98 | 24.50 | 20.90 |

The measured change in this table: use single-thread LZMA2 for a file smaller than the requested
2 MiB parallel block. Larger files retain previous options; compression level,
dictionary and safe archive publication remain unchanged. Streams stay non-solid
and independently editable; an inaccurate pre-existing solid-mode comment was fixed.

Small-file 7z creation improved about **1.15× / 12.8% less time** in this run. Baseline
samples were 23.14–23.92 seconds; changed samples 20.44–20.76 seconds. Resulting small
7z archive changed from 58461 to 58385 bytes; large-file archive sizes were unchanged.
Remaining cases show noise/regressions, not a general speedup or faster extraction.

Raw local measurements: `benchmark-results/javafx-core-baseline.json` and
`benchmark-results/javafx-core-after.json` (ignored). Repeat on representative user
archives before tuning more. Remaining small-file costs require profiling; no
speculative buffer, codec or unsafe in-place changes were made.

## Current ZIP edit measurement

Run from repository root:

```powershell
cargo run --offline --locked --release -p archi-worker --example edit_bench -- 10000 5
cargo run --offline --locked --release -p archi-worker --example edit_bench -- 10000 5 .\target\release\archi-worker.exe
```

The fixture contains 10,000 stored ZIP members under `tree/`. Each case has one
warmup and five measured runs. Archive correctness is checked outside timed sections;
the worker run measures one long-lived Rust worker plus NDJSON request/response. Both
runs use the same warm cache and local Windows x64 machine. Values below are p50 ms.

| Operation | Core | Worker + NDJSON |
|---|---:|---:|
| Open | 59.15 | 55.15 |
| Rename leaf | 120.35 | 102.48 |
| Rename tree | 118.02 | 100.64 |
| Move tree | 120.83 | 104.02 |
| Delete one | 130.39 | 106.22 |
| Compact | 115.31 | 99.99 |
| Test | 97.24 | 89.28 |
| Add one | 136.97 | 118.44 |
| Add 1,000 | 306.71 | 266.62 |
| Replace | 115.71 | 101.02 |
| Create folder | 135.54 | 116.92 |

Historical ZIP edit baseline (same 10,000-member shape, earlier implementation) was
`open 54.73`, `rename-leaf 220.68`, `rename-tree 876.08`, `move-tree 874.03`,
`delete 439.15`, `compact 797.19`, and `test 404.38` ms p50. Current values are
not a universal speedup claim: machine/cache noise, strategy selection and later
validation/sync safety changes affect results. The reproducible raw result is
`benchmark-results/rust-edit-after.json`; worker comparison is
`benchmark-results/rust-edit-worker.json` (local ignored artifacts).
