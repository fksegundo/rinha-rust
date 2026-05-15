# Performance notes

These numbers are local benchmark results from the repository's Docker Compose topology, using the official k6 dataset from `../rinha-de-backend-2026-main/test/test-data.json`.

## Current default

| Setting | Value |
| --- | --- |
| scale | `10000` |
| leaf size | `48` |
| search mode | `key-first` |
| API resources | `0.42 CPU / 165M` per API container |
| LB resources | `0.16 CPU / 20M` |
| k6 load | `900 rps`, `120s`, `250 max VUs` |

Latest selected run:

| Metric | Result |
| --- | --- |
| p99 | `0.61ms` |
| final score | `6000` |
| false positives | `0` |
| false negatives | `0` |
| HTTP errors | `0` |

## Scale and leaf matrix

The scale/leaf sweep first used `verify --diag` to reject candidates with approval errors before running k6. Tested scales:

```text
8192, 9000, 9500, 10000, 10240, 10500, 11000
```

Only `scale=10000` produced zero false positives and zero false negatives across the verified leaf sizes. The k6 finalist runs were:

| scale | leaf | p99 | score | FP | FN | HTTP |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| 10000 | 32 | `0.63ms` | 6000 | 0 | 0 | 0 |
| 10000 | 48 | `0.61ms` | 6000 | 0 | 0 | 0 |
| 10000 | 64 | `0.62ms` | 6000 | 0 | 0 | 0 |
| 10000 | 80 | `0.62ms` | 6000 | 0 | 0 | 0 |
| 10000 | 96 | `0.62ms` | 6000 | 0 | 0 | 0 |

## Resource matrix

With `scale=10000` and `leaf_size=48`, CPU was swept while keeping the total CPU budget at `1.0`.

| API CPU | LB CPU | p99 | FP | FN | HTTP |
| ---: | ---: | --- | ---: | ---: | ---: |
| 0.36 | 0.28 | `0.61ms` | 0 | 0 | 0 |
| 0.38 | 0.24 | `0.62ms` | 0 | 0 | 0 |
| 0.40 | 0.20 | `0.62ms` | 0 | 0 | 0 |
| 0.42 | 0.16 | `0.61ms` | 0 | 0 | 0 |
| 0.44 | 0.12 | `0.62ms` | 0 | 0 | 0 |
| 0.46 | 0.08 | `0.62ms` | 0 | 0 | 0 |

Memory was swept with CPU fixed at `API=0.42`, `LB=0.16`.

| API memory | LB memory | p99 | FP | FN | HTTP |
| ---: | ---: | --- | ---: | ---: | ---: |
| 150M | 50M | `0.62ms` | 0 | 0 | 0 |
| 155M | 40M | `0.62ms` | 0 | 0 | 0 |
| 160M | 30M | `0.62ms` | 0 | 0 | 0 |
| 165M | 20M | `0.61ms` | 0 | 0 | 0 |
| 170M | 10M | `0.62ms` | 0 | 0 | 0 |

## Reproducing

Use the official benchmark path:

```bash
make bench-local
```

For diagnostics:

```bash
make bench-diag
python3 scripts/run_scale_leaf_matrix.py
python3 scripts/run_resource_matrix.py --scale 10000 --leaf-size 48 --phase cpu
```

Extended datasets can help catch parser/order assumptions, but they are not the acceptance benchmark for this repository.
