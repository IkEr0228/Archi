# Listing and query measurements — 2026-11-16

Baseline: commit `8799378`; after: the cleanup/refactoring changes committed with
this report. Windows 11 10.0.26200, Intel64 Family 6 Model 186 Stepping 2, x64.
Rust/Cargo 1.96.1, Python 3.14.5; Java HotSpot 26.0.1 (sources target Java 21).

## Rust worker end-to-end listing

Separate release builds, same deterministic stored ZIP, long-lived workers,
warm filesystem cache, 5 warmups and 21 measured requests per variant/corpus.
Execution order alternates each round. Timing includes pipe transfer and Python
JSON decoding through the complete event, but excludes fixture validation.
Every listing's paths and count are checked outside timing.

| Members | Baseline p50 ms | After p50 ms | Change |
|---:|---:|---:|---:|
| 10,000 | 62.0379 | 60.6964 | -2.16% |
| 50,000 | 317.6239 | 317.7507 | +0.04% |

Ranges overlap: 10k baseline 59.4322–67.8264 ms, after 58.0962–64.8867 ms;
50k baseline 311.9498–332.2187 ms, after 307.1510–333.6180 ms.
This does not establish a general end-to-end speedup. The 50k case is effectively
unchanged. The benchmark does not isolate Rust CPU or allocation costs from the
Python receiver. An earlier exploratory run included validation in its timer;
those numbers are not used here.

Raw samples, binary hashes and environment: [Rust results](results/2026-11-16-rust-listing.json).
Harness and invocation: [README](README.md).

## Java model queries

Deterministic 50,000 entries: 500 folders and 49,500 files. Six fresh JVMs per
variant, alternating baseline/current; each performs 10 warmup and 20 measured
queries. Counts are checked outside timing. Values below are the range of the
six process medians (conventional median, averaging the middle pair).

| Query | Baseline process p50 range, ms | After process p50 range, ms |
|---|---:|---:|
| Case-insensitive extension filter + name sort, 12,375 matches | 1.31815–2.28720 | 0.89475–1.24055 |
| Folder query + descending-size sort, 99 matches | 0.03005–0.04320 | 0.02900–0.04195 |

Extension filtering was faster in this run; folder queries are effectively
unchanged. These are short synthetic JVM measurements, not UI frame latency or
a statistically established universal speedup. Ten warmup iterations do not
guarantee fully stabilized JIT compilation. Cancellation responsiveness, memory
usage, cold-cache behavior, compression and extraction were not benchmarked.

[Raw Java samples](results/2026-11-16-java-model.json),
[summary](results/2026-11-16-java-summary.json),
[reproduction instructions](../desktop-javafx/benchmark/README.md).
