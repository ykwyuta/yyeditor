"""Portable end-to-end CLI timing; writes fixtures only to the specified workdir."""
import argparse
import json
from pathlib import Path
import statistics
import subprocess
import time

p = argparse.ArgumentParser()
p.add_argument("executable", type=Path)
p.add_argument("--workdir", type=Path, required=True)
args = p.parse_args()
exe = args.executable.resolve()
work = args.workdir.resolve()
work.mkdir(parents=True, exist_ok=True)
rows = [f"{i:06d}: configuration value for service endpoint; 日本語の比較データ\n" for i in range(100_000)]
base = "".join(rows).encode("utf-8")
(work / "base.txt").write_bytes(base)
sparse = rows.copy()
for i in range(5000, 100_000, 10_000):
    sparse[i] = sparse[i].replace("value", "changed value")
(work / "sparse.txt").write_bytes("".join(sparse).encode("utf-8"))
(work / "different.txt").write_bytes(base.replace(b"configuration", b"other-setting"))
records = []
for name, right, expected in [("identical", "base.txt", 0), ("ten_changed_lines", "sparse.txt", 1), ("bounded_incomplete", "different.txt", 2)]:
    samples = []
    for _ in range(5):
        start = time.perf_counter()
        result = subprocess.run([str(exe), "diff", str(work / "base.txt"), str(work / right)], capture_output=True, timeout=30)
        samples.append((time.perf_counter() - start) * 1000)
        if result.returncode != expected:
            raise RuntimeError(f"{name}: expected exit {expected}, got {result.returncode}: {result.stderr!r}")
        if expected == 2 and b"incomplete" not in result.stderr:
            raise RuntimeError("budget failure must be explicit")
    records.append({"case": name, "exit": expected, "median_ms": round(statistics.median(samples), 2), "max_ms": round(max(samples), 2)})
report = {"bytes_per_base": len(base), "lines_per_input": len(rows), "runs_per_case": 5, "scope": "whole CLI process: open, decode, compare and input revalidation; memory and cold disk not measured", "results": records}
(work / "timing.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps(report, ensure_ascii=False, indent=2))
