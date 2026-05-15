#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def reorder_request(request, offset):
    if offset % 2 == 0:
        return request
    order = ["customer", "id", "last_transaction", "merchant", "terminal", "transaction"]
    return {key: request[key] for key in order if key in request}


def main():
    parser = argparse.ArgumentParser(description="Generate an extended local k6 dataset.")
    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--factor", type=int, default=2)
    parser.add_argument("--mode", choices=["neutral", "reordered"], default="neutral")
    args = parser.parse_args()

    if args.factor < 1:
        raise SystemExit("--factor must be >= 1")

    source = json.loads(Path(args.input).read_text(encoding="utf-8"))
    entries = source["entries"]
    expanded = []
    for copy_idx in range(args.factor):
        for entry_idx, entry in enumerate(entries):
            cloned = json.loads(json.dumps(entry))
            cloned["request"]["id"] = f"{cloned['request']['id']}-x{copy_idx}"
            if args.mode == "reordered":
                cloned["request"] = reorder_request(cloned["request"], copy_idx + entry_idx)
            expanded.append(cloned)

    stats = dict(source.get("stats", {}))
    stats["total"] = len(expanded)
    stats["fraud_count"] = sum(1 for entry in expanded if not entry["expected_approved"])
    stats["legit_count"] = stats["total"] - stats["fraud_count"]
    if stats["total"]:
        stats["fraud_rate"] = round(stats["fraud_count"] / stats["total"], 4)
        stats["legit_rate"] = round(stats["legit_count"] / stats["total"], 4)

    output = {
        "references_checksum_sha256": source.get("references_checksum_sha256"),
        "stats": stats,
        "entries": expanded,
    }
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(output, separators=(",", ":")), encoding="utf-8")


if __name__ == "__main__":
    main()
