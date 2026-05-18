#!/usr/bin/env python3
"""Run cpuset/affinity matrix for the rinha-rust stack.

Each scenario defines cpuset + optional LB affinity + SO_INCOMING_CPU.
The script builds, starts the stack, runs k6, collects results, and tears down.

Usage:
    python3 scripts/run_cpuset_matrix.py [--scenarios A,B,C,D,E,F,G] [--repeats 1]
"""

import argparse
import csv
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# CPU topology for Mac Mini Late 2014 (2C/4T Haswell):
#   Physical core 0: logical CPUs 0,2
#   Physical core 1: logical CPUs 1,3
#
# Scenarios:
SCENARIOS = {
    "A": {
        "name": "baseline-floating",
        "api1_cpuset": "",
        "api2_cpuset": "",
        "lb_cpuset": "",
        "lb_workers": "2",
        "lb_affinity": "",
        "lb_incoming_cpu": "0",
        "mlock": "0",
        "pool_size": "32",
    },
    "B": {
        "name": "spread-logical",
        "api1_cpuset": "0",
        "api2_cpuset": "1",
        "lb_cpuset": "2",
        "lb_workers": "1",
        "lb_affinity": "2",
        "lb_incoming_cpu": "2",
        "mlock": "0",
        "pool_size": "32",
    },
    "C": {
        "name": "core-affinity",
        "api1_cpuset": "0,2",
        "api2_cpuset": "1,3",
        "lb_cpuset": "0-3",
        "lb_workers": "2",
        "lb_affinity": "2,3",
        "lb_incoming_cpu": "2,3",
        "mlock": "0",
        "pool_size": "32",
    },
    "D": {
        "name": "cross-smt",
        "api1_cpuset": "0",
        "api2_cpuset": "2",
        "lb_cpuset": "1,3",
        "lb_workers": "2",
        "lb_affinity": "1,3",
        "lb_incoming_cpu": "1,3",
        "mlock": "0",
        "pool_size": "32",
    },
    "E": {
        "name": "isolate-lb",
        "api1_cpuset": "0,2",
        "api2_cpuset": "1,3",
        "lb_cpuset": "0-3",
        "lb_workers": "2",
        "lb_affinity": "2,3",
        "lb_incoming_cpu": "2,3",
        "mlock": "0",
        "pool_size": "32",
    },
    "F": {
        "name": "best-plus-mlock",
        "api1_cpuset": "0,2",
        "api2_cpuset": "1,3",
        "lb_cpuset": "0-3",
        "lb_workers": "2",
        "lb_affinity": "2,3",
        "lb_incoming_cpu": "2,3",
        "mlock": "1",
        "pool_size": "32",
    },
    "G": {
        "name": "best-plus-mlock-smallpool",
        "api1_cpuset": "0,2",
        "api2_cpuset": "1,3",
        "lb_cpuset": "0-3",
        "lb_workers": "2",
        "lb_affinity": "2,3",
        "lb_incoming_cpu": "2,3",
        "mlock": "1",
        "pool_size": "16",
    },
}


def run_bench(env, results_dir):
    started = time.monotonic()
    proc = subprocess.run(
        ["make", "bench-local"], env=env, text=True, capture_output=True
    )
    elapsed = time.monotonic() - started
    results_path = results_dir / "results.json"
    result = {}
    if results_path.exists():
        result = json.loads(results_path.read_text(encoding="utf-8"))
    return proc.returncode, elapsed, proc.stdout, proc.stderr, result


def read_throttle(container_name):
    """Read nr_throttled from cgroup cpu.stat inside a container."""
    try:
        out = subprocess.check_output(
            ["docker", "exec", container_name, "cat", "/sys/fs/cgroup/cpu.stat"],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
        for line in out.splitlines():
            if line.startswith("nr_throttled"):
                return int(line.split()[1])
            if line.startswith("throttled_time"):
                throttled_ns = int(line.split()[1])
                return throttled_ns // 1_000_000  # ns → ms
    except Exception:
        pass
    return None


def main():
    parser = argparse.ArgumentParser(
        description="Run cpuset/affinity matrix for rinha-rust"
    )
    parser.add_argument(
        "--scenarios",
        default="A,B,C,D,E,F,G",
        help="Comma-separated scenario keys (default: all)",
    )
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--output-root", default="test/matrix-cpuset")
    parser.add_argument("--scale", type=int, default=10000)
    parser.add_argument("--leaf-size", type=int, default=48)
    args = parser.parse_args()

    scenario_keys = [k.strip() for k in args.scenarios.split(",") if k.strip()]
    output_root = Path(args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)

    rows = []
    for key in scenario_keys:
        cfg = SCENARIOS.get(key)
        if cfg is None:
            print(f"WARNING: unknown scenario '{key}', skipping", flush=True)
            continue

        for repeat in range(1, args.repeats + 1):
            case_name = f"{key}-{cfg['name']}-r{repeat}"
            results_dir = output_root / case_name
            results_dir.mkdir(parents=True, exist_ok=True)

            env = os.environ.copy()
            env.update(
                {
                    "RINHA_NATIVE_SCALE": str(args.scale),
                    "RINHA_NATIVE_LEAF_SIZE": str(args.leaf_size),
                    "API1_CPUSET": cfg["api1_cpuset"],
                    "API2_CPUSET": cfg["api2_cpuset"],
                    "LB_CPUSET": cfg["lb_cpuset"],
                    "LB_WORKERS": cfg["lb_workers"],
                    "LB_CPU_AFFINITY": cfg["lb_affinity"],
                    "LB_SO_INCOMING_CPU": cfg["lb_incoming_cpu"],
                    "RINHA_MLOCK_INDEX": cfg["mlock"],
                    "API_THREAD_POOL_SIZE": cfg["pool_size"],
                    "API_CAP_IPC_LOCK": "IPC_LOCK" if cfg["mlock"] == "1" else "",
                    "API_MEMLOCK_ULIMIT": "-1" if cfg["mlock"] == "1" else "-1",
                    "RESULTS_DIR": str(results_dir),
                }
            )

            print(
                f"\n=== {case_name} ===\n"
                f"  api1_cpuset={cfg['api1_cpuset'] or 'float'} "
                f"api2_cpuset={cfg['api2_cpuset'] or 'float'} "
                f"lb_cpuset={cfg['lb_cpuset'] or 'float'}\n"
                f"  lb_workers={cfg['lb_workers']} "
                f"lb_affinity={cfg['lb_affinity'] or 'none'} "
                f"lb_incoming_cpu={cfg['lb_incoming_cpu']}\n"
                f"  mlock={cfg['mlock']} pool_size={cfg['pool_size']}",
                flush=True,
            )

            code, elapsed, stdout, stderr, result = run_bench(env, results_dir)
            (results_dir / "bench.stdout.log").write_text(stdout, encoding="utf-8")
            (results_dir / "bench.stderr.log").write_text(stderr, encoding="utf-8")

            # Try to read throttle stats (containers are down after bench-local)
            # We'll capture them from docker stats if available
            breakdown = result.get("scoring", {}).get("breakdown", {})
            row = {
                "scenario": key,
                "name": cfg["name"],
                "api1_cpuset": cfg["api1_cpuset"] or "float",
                "api2_cpuset": cfg["api2_cpuset"] or "float",
                "lb_cpuset": cfg["lb_cpuset"] or "float",
                "lb_workers": cfg["lb_workers"],
                "lb_affinity": cfg["lb_affinity"] or "none",
                "lb_incoming_cpu": cfg["lb_incoming_cpu"],
                "mlock": cfg["mlock"],
                "pool_size": cfg["pool_size"],
                "repeat": repeat,
                "status": "ok" if code == 0 else "failed",
                "bench_seconds": f"{elapsed:.1f}",
                "p50": result.get("latency", {}).get("p50", ""),
                "p95": result.get("latency", {}).get("p95", ""),
                "p99": result.get("p99", ""),
                "final_score": result.get("scoring", {}).get("final_score", ""),
                "fp": breakdown.get("false_positive_detections", ""),
                "fn": breakdown.get("false_negative_detections", ""),
                "http_errors": breakdown.get("http_errors", ""),
                "results_dir": str(results_dir),
            }
            rows.append(row)

            print(
                f"  status={row['status']} p99={row['p99']} "
                f"score={row['final_score']} "
                f"fp={row['fp']} fn={row['fn']} http={row['http_errors']}",
                flush=True,
            )

    fieldnames = [
        "scenario",
        "name",
        "api1_cpuset",
        "api2_cpuset",
        "lb_cpuset",
        "lb_workers",
        "lb_affinity",
        "lb_incoming_cpu",
        "mlock",
        "pool_size",
        "repeat",
        "status",
        "bench_seconds",
        "p50",
        "p95",
        "p99",
        "final_score",
        "fp",
        "fn",
        "http_errors",
        "results_dir",
    ]
    csv_path = output_root / "summary.csv"
    with csv_path.open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)

    print(f"\nSummary written to {csv_path}")
    return 0 if any(row["status"] == "ok" for row in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
