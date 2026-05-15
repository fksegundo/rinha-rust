#!/usr/bin/env python3
import argparse
import csv
import json
import os
import subprocess
import sys
import time
from pathlib import Path


CPU_PAIRS = [("0.36", "0.28"), ("0.38", "0.24"), ("0.40", "0.20"), ("0.42", "0.16"), ("0.44", "0.12"), ("0.46", "0.08")]
MEMORY_PAIRS = [("150M", "50M"), ("155M", "40M"), ("160M", "30M"), ("165M", "20M"), ("170M", "10M")]


def parse_pair_list(value):
    pairs = []
    for item in value.split(","):
        item = item.strip()
        if not item:
            continue
        left, right = item.split("/", 1)
        pairs.append((left, right))
    return pairs


def run_bench(env, results_dir):
    started = time.monotonic()
    proc = subprocess.run(["make", "bench-local"], env=env, text=True, capture_output=True)
    elapsed = time.monotonic() - started
    results_path = results_dir / "results.json"
    result = {}
    if results_path.exists():
        result = json.loads(results_path.read_text(encoding="utf-8"))
    return proc.returncode, elapsed, proc.stdout, proc.stderr, result


def main():
    parser = argparse.ArgumentParser(description="Run k6 resource matrix with fixed scale and leaf size.")
    parser.add_argument("--scale", required=True, type=int)
    parser.add_argument("--leaf-size", required=True, type=int)
    parser.add_argument("--phase", choices=["cpu", "memory", "all"], default="cpu")
    parser.add_argument("--cpu-pairs", default=",".join(f"{api}/{lb}" for api, lb in CPU_PAIRS))
    parser.add_argument("--memory-pairs", default=",".join(f"{api}/{lb}" for api, lb in MEMORY_PAIRS))
    parser.add_argument("--api-cpu", default="0.44")
    parser.add_argument("--lb-cpu", default="0.12")
    parser.add_argument("--api-memory", default="170M")
    parser.add_argument("--lb-memory", default="10M")
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--output-root", default="test/matrix-resources")
    args = parser.parse_args()

    cpu_pairs = parse_pair_list(args.cpu_pairs)
    memory_pairs = parse_pair_list(args.memory_pairs)
    output_root = Path(args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)

    cases = []
    if args.phase in ("cpu", "all"):
        for api_cpu, lb_cpu in cpu_pairs:
            cases.append(("cpu", api_cpu, lb_cpu, args.api_memory, args.lb_memory))
    if args.phase in ("memory", "all"):
        for api_mem, lb_mem in memory_pairs:
            cases.append(("memory", args.api_cpu, args.lb_cpu, api_mem, lb_mem))

    rows = []
    for phase, api_cpu, lb_cpu, api_mem, lb_mem in cases:
        for repeat in range(1, args.repeats + 1):
            case_name = f"{phase}-api{api_cpu}-lb{lb_cpu}-mem{api_mem}-{lb_mem}-r{repeat}".replace("/", "-")
            results_dir = output_root / case_name
            results_dir.mkdir(parents=True, exist_ok=True)

            env = os.environ.copy()
            env.update(
                {
                    "RINHA_NATIVE_SCALE": str(args.scale),
                    "RINHA_NATIVE_LEAF_SIZE": str(args.leaf_size),
                    "API_CPU_LIMIT": api_cpu,
                    "LB_CPU_LIMIT": lb_cpu,
                    "API_MEMORY_LIMIT": api_mem,
                    "LB_MEMORY_LIMIT": lb_mem,
                    "RESULTS_DIR": str(results_dir),
                }
            )
            code, elapsed, stdout, stderr, result = run_bench(env, results_dir)
            (results_dir / "bench.stdout.log").write_text(stdout, encoding="utf-8")
            (results_dir / "bench.stderr.log").write_text(stderr, encoding="utf-8")

            breakdown = result.get("scoring", {}).get("breakdown", {})
            row = {
                "phase": phase,
                "api_cpu": api_cpu,
                "lb_cpu": lb_cpu,
                "api_memory": api_mem,
                "lb_memory": lb_mem,
                "repeat": repeat,
                "status": "ok" if code == 0 else "failed",
                "bench_seconds": f"{elapsed:.3f}",
                "p99": result.get("p99", ""),
                "final_score": result.get("scoring", {}).get("final_score", ""),
                "false_positive_detections": breakdown.get("false_positive_detections", ""),
                "false_negative_detections": breakdown.get("false_negative_detections", ""),
                "http_errors": breakdown.get("http_errors", ""),
                "results_dir": str(results_dir),
            }
            rows.append(row)
            print(
                f"{case_name} status={row['status']} p99={row['p99']} "
                f"fp={row['false_positive_detections']} fn={row['false_negative_detections']} http={row['http_errors']}",
                flush=True,
            )

    fieldnames = [
        "phase",
        "api_cpu",
        "lb_cpu",
        "api_memory",
        "lb_memory",
        "repeat",
        "status",
        "bench_seconds",
        "p99",
        "final_score",
        "false_positive_detections",
        "false_negative_detections",
        "http_errors",
        "results_dir",
    ]
    with (output_root / "summary.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)

    return 0 if any(row["status"] == "ok" for row in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
