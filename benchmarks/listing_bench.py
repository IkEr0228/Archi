#!/usr/bin/env python3
"""Benchmark long-lived worker ZIP listing; validation is excluded from timing."""
import argparse, hashlib, json, platform, queue, statistics, subprocess, sys, tempfile, threading, time, zipfile
from pathlib import Path

WARMUPS = 5
MEASURED = 21
COUNTS = (10_000, 50_000)
IO_TIMEOUT_SECONDS = 30
SHUTDOWN_TIMEOUT_SECONDS = 5


def make_zip(path, count):
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as z:
        for i in range(count):
            z.writestr(f"tree/item-{i:06d}.txt", f"payload-{i}\n")


class Worker:
    def __init__(self, binary):
        self.proc = subprocess.Popen(
            [str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1,
        )
        self.lines = queue.Queue()
        self.reader = threading.Thread(target=self._read_lines, daemon=True)
        self.reader.start()

    def _read_lines(self):
        try:
            for line in self.proc.stdout:
                self.lines.put(("line", line))
        except Exception as exc:
            self.lines.put(("error", exc))
        finally:
            self.lines.put(("eof", None))

    def request(self, archive):
        request = {"version": 1, "id": "bench", "command": "open", "path": str(archive)}
        self.proc.stdin.write(json.dumps(request, separators=(",", ":")) + "\n")
        self.proc.stdin.flush()
        events = []
        while True:
            try:
                kind, value = self.lines.get(timeout=IO_TIMEOUT_SECONDS)
            except queue.Empty as exc:
                raise TimeoutError("worker response timed out") from exc
            if kind == "eof":
                stderr = self.proc.stderr.read()
                raise RuntimeError(f"worker exited while listing: {stderr}")
            if kind == "error":
                raise RuntimeError(f"worker output reader failed: {value}")
            event = json.loads(value)
            events.append(event)
            if event.get("event") == "error":
                raise RuntimeError(f"worker error: {event}")
            if event.get("event") == "complete":
                # Stop here: JSON receive/parse is timed, fixture validation is not.
                return time.perf_counter_ns(), events

    def stop(self):
        if self.proc.poll() is not None:
            return
        try:
            self.proc.stdin.write(json.dumps({"version": 1, "id": "shutdown", "command": "shutdown"}) + "\n")
            self.proc.stdin.flush()
            self.proc.wait(timeout=SHUTDOWN_TIMEOUT_SECONDS)
        except (BrokenPipeError, subprocess.TimeoutExpired):
            self.proc.kill()
            self.proc.wait(timeout=SHUTDOWN_TIMEOUT_SECONDS)


def validate(events, expected):
    listed = [entry for event in events if event.get("event") == "entries" for entry in event["data"]]
    expected_entries = expected + 1  # virtual tree directory plus files
    if len(listed) != expected_entries:
        raise AssertionError(f"listed {len(listed)} entries, expected {expected_entries}")
    names = {entry["path"] for entry in listed}
    expected_names = {"tree"} | {f"tree/item-{i:06d}.txt" for i in range(expected)}
    if names != expected_names:
        raise AssertionError("listing names differ from deterministic fixture")
    archive = next((event for event in events if event.get("event") == "archive"), None)
    complete = events[-1]
    if archive is None or complete.get("data", {}).get("entry_count") != expected_entries:
        raise AssertionError("missing archive event or incorrect complete count")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--after", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    baseline, after, output = map(lambda value: Path(value).resolve(), (args.baseline, args.after, args.output))
    started = time.time()
    results = []
    with tempfile.TemporaryDirectory(prefix="archi-listing-bench-") as directory:
        for count in COUNTS:
            archive = Path(directory) / f"fixture-{count}.zip"
            make_zip(archive, count)
            workers = {"baseline": Worker(baseline), "after": Worker(after)}
            samples = {"baseline": [], "after": []}
            try:
                for round_no in range(WARMUPS + MEASURED):
                    order = ("baseline", "after") if round_no % 2 == 0 else ("after", "baseline")
                    for label in order:
                        start = time.perf_counter_ns()
                        end, events = workers[label].request(archive)
                        elapsed = (end - start) / 1_000_000
                        validate(events, count)
                        if round_no >= WARMUPS:
                            samples[label].append(elapsed)
            finally:
                for worker in workers.values():
                    worker.stop()
            for label in ("baseline", "after"):
                values = samples[label]
                results.append({"members": count, "variant": label, "warmups": WARMUPS,
                                "samples_ms": values, "p50_ms": statistics.median(values),
                                "min_ms": min(values), "max_ms": max(values),
                                "validated_entries": count + 1})
    payload = {
        "schema": 2, "benchmark": "long-lived worker NDJSON ZIP open/list",
        "timing": "request write/flush through complete event; validation excluded",
        "fixture": {"compression": "stored", "members": list(COUNTS),
                    "path_pattern": "tree/item-{index:06d}.txt", "payload": "payload-{index}\\n"},
        "warmups": WARMUPS, "measured_rounds": MEASURED,
        "order": "alternating baseline/after per round", "io_timeout_seconds": IO_TIMEOUT_SECONDS,
        "shutdown_timeout_seconds": SHUTDOWN_TIMEOUT_SECONDS, "results": results,
        "elapsed_seconds": time.time() - started,
        "environment": {
            "os": platform.platform(), "machine": platform.machine(), "processor": platform.processor(),
            "python": platform.python_version(), "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "cargo": subprocess.check_output(["cargo", "--version"], text=True).strip(),
            "baseline_binary_sha256": sha256(baseline), "after_binary_sha256": sha256(after),
        },
    }
    output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload, indent=2))


if __name__ == "__main__":
    main()
