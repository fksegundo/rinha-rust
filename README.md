# Rinha de Backend 2026 - Rust Low-Latency Fraud Scoring

[![Rust CI](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--api-blue)](https://github.com/fksegundo/rinha-rust/pkgs/container/rinha-rust-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Rust implementation for the [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) challenge.

This repository is presented as a low-latency backend case study: build-time preprocessing, memory-mapped startup, an exact specialist kNN index, a minimal HTTP/1.1 path, fixed fraud-score responses, and Unix socket file descriptor passing through a companion custom load balancer.

Portuguese version: [docs/README.pt-BR.md](docs/README.pt-BR.md)

## Implementation

This is not a framework-based HTTP API. The implementation keeps the request path intentionally small so most of the CPU budget is spent on fraud-score lookup instead of generic request infrastructure.

Key implementation choices:

- build-time index generation from the official `references.json.gz`;
- runtime index loading with `mmap`;
- exact kNN scoring with `k = 5`;
- quantized vectors using a build-time scale;
- specialist partitioning and bounding-box pruning while preserving exactness;
- AVX2 distance scans in the production build;
- minimal HTTP parsing for the challenge endpoints;
- precomputed HTTP responses for all six possible fraud scores;
- Unix socket FD passing mode for the companion load balancer.

The API runtime expects FD passing. Each API process listens on `RINHA_FD_SOCKET` or, by default, `/sockets/${HOSTNAME}.sock`. The load balancer listens on `LB_PORT` and dispatches accepted TCP client descriptors to the comma-separated `API_SOCKETS` list.

## Architecture

```text
client
  |
  v
custom LB (:9999)
  |
  |  Unix socket FD passing (SCM_RIGHTS)
  v
api1 / api2
  |
  v
mmap specialist index
  |
  v
exact kNN fraud scoring
```

The Docker image builds two Rust binaries:

- `api`: serves `/ready` and `/fraud-score`;
- `lb`: accepts TCP traffic and forwards client file descriptors to API Unix sockets;
- `preprocess`: converts the official references file into the compact runtime index.

During the image build, `preprocess` downloads and converts the official references file into `/app/index/rinha-specialist.idx`. At runtime, the API maps that file, warms up a small set of synthetic queries, and serves requests from the mapped index.

## Correctness Model

The fraud score is derived from the labels of the five nearest reference vectors:

```text
fraud_score = fraud_count_among_5_nearest_neighbors / 5
```

The partitioned search is exact. The matching specialist partition is searched first, but other partitions are still visited when their lower bound can improve the current top-k set. Partitioning is used for ordering and pruning, not approximate lookup.

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

## Performance Notes

The current compose topology reserves the official `1.0 CPU / 350 MB` budget as:

| Component | CPU | Memory |
| --- | ---: | ---: |
| API 1 | `0.42` | `165M` |
| API 2 | `0.42` | `165M` |
| LB | `0.16` | `20M` |

The runtime defaults are tuned for the current implementation:

| Setting | Default |
| --- | --- |
| Vector scale | `10000` |
| Index leaf size | `56` in the Docker build |
| LB listen port | `9999` |
| LB upstreams | `/sockets/api1.sock,/sockets/api2.sock` |
| Warmup queries | `2048` |
| Index locking | enabled in the Docker runtime |


## Local Development

Run tests:

```bash
make test
```

Build the local API image:

```bash
make build
```

Validate the local compose file:

```bash
make config
```

Start the local stack:

```bash
make up
```

Stop the local stack:

```bash
make down
```

The local compose file expects `rinha-rust-api:local` and `rinha-rust-lb:local`. The LB is configured with `LB_PORT`, `LB_BACKLOG`, `LB_ACCEPT_BATCH`, and `API_SOCKETS`.

## Docker Image

The API image is published to GitHub Container Registry by the repository workflow:

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

The public compose file is available at [docker/docker-compose.yml](docker/docker-compose.yml).

## Project Layout

```text
src/bin/api.rs          API binary entrypoint
src/bin/preprocess.rs   build-time index generator entrypoint
src/api/                server startup, request routing and runtime options
src/fd_passing/         SCM_RIGHTS file descriptor receiving
src/http/               minimal HTTP/1.1 parser and fixed responses
src/index/              index format, builder, mmap loader and exact kNN search
src/vector/             challenge payload parsing and vector quantization
docker/Dockerfile       multi-stage API image build
docker/compose.local.yml
docker/compose.submission.yml
docs/                   architecture, performance and Portuguese README
info.json               challenge metadata
```

## Related Repositories

- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb) - companion custom load balancer used by this submission.
- [zanfranceschi/rinha-de-backend-2026](https://github.com/zanfranceschi/rinha-de-backend-2026) - official challenge repository.

## License

MIT
