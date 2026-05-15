# Rinha de Backend 2026 - Rust low-latency fraud scoring

[![Rust CI](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--api-blue)](https://github.com/fksegundo/rinha-rust/pkgs/container/rinha-rust-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Rust implementation for the [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) challenge.

This repository is structured as a low-latency architecture case study: build-time preprocessing, mmap-based startup, a specialist exact kNN index, minimal HTTP parsing, fixed fraud-score responses, and Unix-socket FD passing through a companion custom load balancer.

Portuguese version: [docs/README.pt-BR.md](docs/README.pt-BR.md)

The companion load balancer lives in a separate repository:

- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb)

## Why this is interesting

This is not a framework-based HTTP API.

The hot path is intentionally small:

- build-time index generation from the official references file;
- memory-mapped index loading at runtime;
- exact kNN search with `k = 5`;
- quantized vectors for compact distance computation;
- specialist partitioning with exact-safe pruning;
- SIMD-assisted distance scans when AVX2 is available;
- minimal HTTP/1.1 parsing;
- precomputed fixed HTTP responses for the six possible fraud scores;
- Unix socket FD passing from the LB to the API processes.

The goal is to keep startup fast, reduce runtime allocation, and spend most of the CPU budget on the actual fraud-score lookup.

## Performance notes

Latest reproducible local benchmark on my machine, using the official k6 dataset and the compose topology from this repository:

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

`make test-k6` fails fast if the standard dataset is missing at:

```text
../rinha-de-backend-2026-main/test/test-data.json
```

Extended datasets exist only for local robustness checks and tuning experiments. They do not replace the official benchmark flow.

More detailed benchmark notes and tuning matrices are documented in:

- [Performance notes and benchmark matrix](docs/performance.md)

## Architecture

```text
client
  |
  v
custom LB
  |
  |  Unix socket FD passing
  v
api1 / api2
  |
  v
mmap specialist index
  |
  v
exact kNN fraud scoring
```

At image build time, the official `references.json.gz` file is downloaded and converted into a compact specialist index.

At runtime, each API process maps the prebuilt index into memory and serves `/fraud-score` requests using exact kNN search.

The LB accepts external HTTP traffic on port `9999` and forwards accepted TCP sockets to the API instances through Unix socket descriptor passing. The API receives the socket FD and handles the HTTP connection directly.

More details are available in:

- [Architecture deep dive](docs/architecture.md)

## Correctness model

The optimized `key-first` path is still exact.

The specialist partition that matches the query key is searched first, but other partitions are still considered when their lower bound can improve the current top-k set. In other words, partitioning is used to prioritize and prune the search, not to turn the query into an approximate lookup.

The final fraud score is derived from the labels of the five nearest reference vectors:

```text
fraud_score = fraud_count_among_5_nearest_neighbors / 5
```

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/ready` | Readiness probe |
| `POST` | `/fraud-score` | Receives the challenge payload and returns the approval decision plus fraud score |

Example response:

```json
{
  "approved": true,
  "fraud_score": 0.2
}
```

## Benchmark workflow

Requirements for the default benchmark:

- Docker
- Docker Compose
- curl
- access to the official challenge checkout at `../rinha-de-backend-2026-main`

Python 3 is required only for benchmark and tuning helper scripts.

```bash
make build        # Build local API image with the preprocess step
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

The default local build uses:

- `RINHA_NATIVE_SCALE=10000`
- `RINHA_NATIVE_LEAF_SIZE=48`
- `RINHA_SEARCH_MODE=key-first`
- `API_CPU_LIMIT=0.42`
- `LB_CPU_LIMIT=0.16`
- `API_MEMORY_LIMIT=165M`
- `LB_MEMORY_LIMIT=20M`

## Local development

Build the API image:

```bash
make build
```

Build the companion LB image:

```bash
make build-lb
```

Start the stack:

```bash
make up
```

Check readiness:

```bash
curl -i http://localhost:9999/ready
```

Run the official benchmark:

```bash
make test-k6
```

Stop the stack:

```bash
make down
```

## Docker image

The API image is published to GitHub Container Registry by the repository workflow.

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

The official submission branch references public GHCR images so the challenge runner can start the stack without building locally.

## Project layout

```text
bins/
  api.rs                 API binary entrypoint
  preprocess.rs          build-time index generator entrypoint
  verify.rs              verification utility

src/
  api/                   HTTP server and request routing
  fd_passing/            SCM_RIGHTS file descriptor receiving
  http/                  minimal HTTP/1.1 parser and fixed responses
  index/                 specialist index builder, mmap loader, exact kNN search
  vector/                JSON payload to quantized vector parsing

submission/
  Dockerfile             multi-stage image build: compile -> preprocess -> runtime
  docker-compose.yml     local compose topology

scripts/
  run_scale_leaf_matrix.py
  run_resource_matrix.py
  generate_extended_test_data.py

docs/
  architecture.md
  performance.md
  README.pt-BR.md

Makefile                 local build, benchmark, diagnostics and cleanup targets
info.json                challenge metadata
```

## Implementation highlights

### Build-time preprocessing

The Docker build downloads the official references file and converts it into a compact index before the runtime image is produced.

This keeps runtime startup simple: the API only needs to memory map the generated index and warm up the lookup path.

### mmap startup

The index is loaded with `mmap`, avoiding a full read/copy into owned heap structures during startup. The mapped region backs the vector and label sections used by the search path.

### Specialist exact kNN index

The index groups references into specialist partitions and stores bounding boxes for pruning. Query execution prioritizes the most promising partitions and tree nodes, but preserves exactness by continuing to search any partition whose lower bound can still improve the current top-k result.

### Minimal HTTP path

The API implements only the HTTP behavior needed by the challenge:

- `GET /ready`
- `POST /fraud-score`
- HTTP/1.1 keep-alive
- fixed JSON responses for `fraud_score` values from `0.0` to `1.0`

This avoids general-purpose framework overhead in the hot path.

### FD passing

The custom companion LB accepts the external connection and passes the accepted socket file descriptor to one of the API processes over a Unix socket using `SCM_RIGHTS`.

This allows the API process to handle the client connection directly after balancing, keeping the handoff lightweight.

## Submission branch

`main` keeps the implementation, documentation and local development workflow.

The `submission` branch is reserved for the official challenge handoff shape, with the submission files at the repository root and public GHCR image references.

## Related repositories

- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb) — companion custom load balancer used by this submission.
- [zanfranceschi/rinha-de-backend-2026](https://github.com/zanfranceschi/rinha-de-backend-2026) — official challenge repository.

## License

MIT