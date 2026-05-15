#!/usr/bin/env python3
import argparse
import csv
import json
import os
import subprocess
import sys
import time
from pathlib import Path


DEFAULT_SCALES = [8192, 9000, 9500, 10000, 10240, 10500, 11000]
DEFAULT_LEAF_SIZES = [32, 48, 64, 80, 96, 128, 160, 192, 256, 384]


def parse_int_list(value):
    return [int(item.strip()) for item in value.split(",") if item.strip()]


def run(cmd, env=None):
    started = time.monotonic()
    proc = subprocess.run(cmd, env=env, text=True, capture_output=True)
    elapsed = time.monotonic() - started
    return proc.returncode, elapsed, proc.stdout, proc.stderr


def metric(summary, name, field, default=0):
    return summary.get("diagnostics", {}).get(name, {}).get(field, default)


def write_text(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description="Run scale x leaf-size verification matrix.")
    parser.add_argument("--scales", default=",".join(map(str, DEFAULT_SCALES)))
    parser.add_argument("--leaf-sizes", default=",".join(map(str, DEFAULT_LEAF_SIZES)))
    parser.add_argument("--references", default="../rinha-de-backend-2026-main/resources/references.json.gz")
    parser.add_argument("--test-data", default="../rinha-de-backend-2026-main/test/test-data.json")
    parser.add_argument("--search-mode", default="key-first")
    parser.add_argument("--output-root", default="test/matrix-scale-leaf")
    parser.add_argument("--keep-indexes", action="store_true")
    args = parser.parse_args()

    scales = parse_int_list(args.scales)
    leaf_sizes = parse_int_list(args.leaf_sizes)
    output_root = Path(args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)
    rows = []

    for scale in scales:
        scale_dir = output_root / f"scale-{scale}"
        scale_dir.mkdir(parents=True, exist_ok=True)

        env = os.environ.copy()
        env["RINHA_NATIVE_SCALE"] = str(scale)
        code, elapsed, stdout, stderr = run(["cargo", "build", "--release", "--bins"], env=env)
        write_text(scale_dir / "cargo-build.stdout.log", stdout)
        write_text(scale_dir / "cargo-build.stderr.log", stderr)
        if code != 0:
            rows.append({"scale": scale, "status": "build_failed", "build_seconds": f"{elapsed:.3f}"})
            continue

        for leaf_size in leaf_sizes:
            case_dir = scale_dir / f"leaf-{leaf_size}"
            case_dir.mkdir(parents=True, exist_ok=True)
            index_path = case_dir / "rinha-specialist.idx"
            summary_path = case_dir / "verify-summary.json"

            env = os.environ.copy()
            env["RINHA_NATIVE_SCALE"] = str(scale)
            env["RINHA_LEAF_SIZE"] = str(leaf_size)
            code, preprocess_elapsed, stdout, stderr = run(
                [
                    "target/release/preprocess",
                    args.references,
                    str(index_path),
                ],
                env=env,
            )
            write_text(case_dir / "preprocess.stdout.log", stdout)
            write_text(case_dir / "preprocess.stderr.log", stderr)
            if code != 0:
                rows.append(
                    {
                        "scale": scale,
                        "leaf_size": leaf_size,
                        "status": "preprocess_failed",
                        "preprocess_seconds": f"{preprocess_elapsed:.3f}",
                    }
                )
                continue

            env = os.environ.copy()
            env["RINHA_NATIVE_SCALE"] = str(scale)
            env["RINHA_SEARCH_MODE"] = args.search_mode
            code, verify_elapsed, stdout, stderr = run(
                [
                    "target/release/verify",
                    str(index_path),
                    args.test_data,
                    "--diag",
                    "--approval-summary-json",
                    str(summary_path),
                ],
                env=env,
            )
            write_text(case_dir / "verify.stdout.log", stdout)
            write_text(case_dir / "verify.stderr.log", stderr)

            summary = {}
            if summary_path.exists():
                summary = json.loads(summary_path.read_text(encoding="utf-8"))

            row = {
                "scale": scale,
                "leaf_size": leaf_size,
                "status": "ok" if code == 0 else "verify_failed",
                "preprocess_seconds": f"{preprocess_elapsed:.3f}",
                "verify_seconds": f"{verify_elapsed:.3f}",
                "verify_elapsed_ms": f"{summary.get('elapsed_ms', 0):.3f}",
                "score_mismatches": summary.get("score_mismatches", ""),
                "false_positive_detections": summary.get("false_positive_detections", ""),
                "false_negative_detections": summary.get("false_negative_detections", ""),
                "parse_errors": summary.get("parse_errors", ""),
                "partitions_p99": metric(summary, "partitions_visited", "p99", ""),
                "nodes_p99": metric(summary, "nodes_visited", "p99", ""),
                "leaves_p99": metric(summary, "leaves_scanned", "p99", ""),
                "blocks_p99": metric(summary, "blocks_scanned", "p99", ""),
                "node_count": summary.get("metadata", {}).get("node_count", ""),
                "block_count": summary.get("metadata", {}).get("block_count", ""),
                "index_bytes": index_path.stat().st_size if index_path.exists() else "",
            }
            rows.append(row)
            if not args.keep_indexes and index_path.exists():
                index_path.unlink()
            print(
                f"scale={scale} leaf={leaf_size} status={row['status']} "
                f"fp={row['false_positive_detections']} fn={row['false_negative_detections']} "
                f"score_mismatches={row['score_mismatches']} blocks_p99={row['blocks_p99']}",
                flush=True,
            )

    fieldnames = [
        "scale",
        "leaf_size",
        "status",
        "preprocess_seconds",
        "verify_seconds",
        "verify_elapsed_ms",
        "score_mismatches",
        "false_positive_detections",
        "false_negative_detections",
        "parse_errors",
        "partitions_p99",
        "nodes_p99",
        "leaves_p99",
        "blocks_p99",
        "node_count",
        "block_count",
        "index_bytes",
        "build_seconds",
    ]
    with (output_root / "summary.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)

    eligible = [
        row
        for row in rows
        if row.get("status") == "ok"
        and row.get("false_positive_detections") == 0
        and row.get("false_negative_detections") == 0
        and row.get("parse_errors") == 0
    ]
    eligible.sort(
        key=lambda row: (
            int(row.get("blocks_p99") or 10**9),
            int(row.get("leaves_p99") or 10**9),
            float(row.get("verify_seconds") or 10**9),
            int(row.get("score_mismatches") or 10**9),
        )
    )
    with (output_root / "ranked.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(eligible)

    print(f"wrote {output_root / 'summary.csv'}")
    print(f"wrote {output_root / 'ranked.csv'}")
    if eligible:
        best = eligible[0]
        print(f"best_verify_candidate scale={best['scale']} leaf={best['leaf_size']}")
    return 0 if eligible else 1


if __name__ == "__main__":
    sys.exit(main())
