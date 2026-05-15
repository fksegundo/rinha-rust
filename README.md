# Rinha de Backend 2026 - Rust low-latency fraud scoring

[![Rust CI](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--api-blue)](https://github.com/fksegundo/rinha-rust/pkgs/container/rinha-rust-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Rust implementation for the [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) challenge. The repository is structured as a low-latency architecture case study: image-time preprocessing, mmap startup, a specialist exact kNN index, a custom load balancer, and Unix-socket FD passing.

## Performance notes

Latest local benchmark, using the official k6 dataset and the compose topology from this repository:

| Metric | Result |
| --- | --- |
| p99 local | `0.61ms` |
| score | `6000` |
| false positives | `0` |
| false negatives | `0` |
| HTTP errors | `0` |
| environment | local Docker Compose, `900 rps`, `120s`, `250 max VUs` |
| resources | `2 x API: 0.42 CPU / 165M`, `LB: 0.16 CPU / 20M` |
| strategy | build-time specialist index, `mmap`, `scale=10000`, `leaf_size=48`, `key-first` exact kNN search, FD passing |

The benchmark command is intentionally based on the official/default dataset:

```bash
make bench-local
```

`make test-k6` fails fast if the standard dataset is missing at `../rinha-de-backend-2026-main/test/test-data.json`. Extended datasets exist only for local robustness checks and do not replace the official benchmark flow.

## Architecture deep dive

```text
client -> LB -> fd passing -> api1/api2 -> mmap index -> exact kNN
```

The API does exact kNN with `k=5` over quantized vectors. The Docker image builds the index ahead of time from the official references file, then the runtime process memory maps that index at startup. The custom LB accepts external HTTP traffic and forwards accepted sockets to API workers through Unix socket descriptor passing, keeping the hot path small.

More detail:

- [Architecture deep dive](docs/architecture.md)
- [Performance notes and benchmark matrix](docs/performance.md)

## Benchmark workflow

Requirements: Docker, Docker Compose, curl, Python 3, and access to the official challenge checkout at `../rinha-de-backend-2026-main`.

```bash
make build        # Build local API image with preprocess step
make build-lb     # Build the custom LB image from ../rinha-dotnetrust-lb
make up           # Start the full local topology on :9999
make test-k6      # Run the official k6 dataset against the running stack
make bench-local  # Build, start, run official k6, and tear down
make bench-diag   # Same benchmark plus API logs and docker stats
```

Tuning helpers:

```bash
python3 scripts/run_scale_leaf_matrix.py
python3 scripts/run_resource_matrix.py --scale 10000 --leaf-size 48 --phase cpu
```

The default build uses:

- `RINHA_NATIVE_SCALE=10000`
- `RINHA_NATIVE_LEAF_SIZE=48`
- `RINHA_SEARCH_MODE=key-first`
- `API_CPU_LIMIT=0.42`, `LB_CPU_LIMIT=0.16`
- `API_MEMORY_LIMIT=165M`, `LB_MEMORY_LIMIT=20M`

## Project layout

```text
bins/                 binary entrypoints: api, preprocess, verify
src/api/              HTTP server and request routing
src/fd_passing/       SCM_RIGHTS file descriptor passing
src/http/             minimal HTTP/1.1 parsing and fixed responses
src/index/            specialist index builder, mmap loader, exact kNN search
src/vector/           JSON payload to quantized vector parsing
submission/           Dockerfile and official compose topology
scripts/              benchmark and tuning helpers
docs/                 architecture and performance notes
```

## Submission branch

`main` keeps the implementation and documentation. The `submission` branch is reserved for the official challenge handoff shape, with the submission files at the repository root and public GHCR image references.

Published API image:

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

## License

MIT
