# ArchiveModel benchmark

Reproducible baseline-vs-current query benchmark. It does not modify production
sources or retain captured baseline/current source copies. The runner obtains the
baseline from explicit Git revision `8799378` with `git show`, and reads current
`desktop-javafx/src/archi/ArchiveModel.java`.

Workload: deterministic 50,000 entries (500 folders + 49,500 files), warm JIT,
case-insensitive `.JaVa` extension search across the archive with name sorting,
and a folder query sorted by descending size. Every JVM asserts entry and query
counts before and after timing.

## Reproduce (PowerShell)

Run from the repository root:

```powershell
New-Item -ItemType Directory -Force .cache\javafx | Out-Null
Get-FileHash .cache\javafx\gson-2.13.2.jar -Algorithm SHA256
powershell -NoProfile -ExecutionPolicy Bypass -File desktop-javafx\benchmark\run.ps1
```

The runner requires the existing pinned Gson JAR at
`.cache/javafx/gson-2.13.2.jar`; it never downloads dependencies. The expected
SHA-256 is
`DD0CE1B55A3ED2080CB70F9C655850CDA86C206862310009DCB5E5C95265A5E0`.

It materializes the baseline source temporarily under ignored
`desktop-javafx/build/benchmark/sources`, compiles baseline and current into
separate ignored directories under `desktop-javafx/build/benchmark`, then
alternates baseline/current across 12 fresh `java -ea` JVMs (6 each), with 10
warmups and 20 measured samples per JVM. It creates `benchmark-results` if
needed and writes:

- `benchmark-results/java-archive-model-raw.json` — raw JSON array
- `benchmark-results/java-archive-model-raw.jsonl` — one raw record per JVM
- `benchmark-results/java-archive-model-summary.json` — p50 ranges and counts

The p50 is the conventional median: sorted values use the middle value for an
odd count and the average of the two middle values for an even count. The
summary records this definition and is not a speedup claim.
