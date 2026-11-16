# Rust worker listing benchmark

Measures optimized `archi-worker` ZIP listing for deterministic stored ZIPs with
10,000 and 50,000 members. Each worker stays alive for all requests. Every case
uses 5 warmups and 21 measured rounds, alternating baseline/after order.

Timing starts immediately before writing the `open` request and ends when the
`complete` event has been received and decoded. Listing-name/count validation is
performed afterward and is excluded from the timer. Worker response reads have
a 30-second watchdog; shutdown has a 5-second bounded wait and kills the worker
if needed.

## Usage

Build each revision into a separate release target, then run:

```text
python benchmarks/listing_bench.py \
  --baseline .cache/benchmark-baseline-target/release/archi-worker.exe \
  --after .cache/benchmark-after-target/release/archi-worker.exe \
  --output benchmark-results/rust-listing-YYYY-MM-DD-v2.json
```

The output JSON contains raw samples, p50/min/max values, validation counts,
timing methodology, toolchain/environment metadata, and binary SHA-256 hashes.
The benchmark creates fixtures in a temporary directory and removes them when
finished.
