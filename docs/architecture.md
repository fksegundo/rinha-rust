# Architecture deep dive

This repository is optimized for the Rinha de Backend 2026 fraud scoring workload. The important design decision is to move expensive work to image build time and keep the request path predictable.

## Request path

```text
client
  -> custom LB
  -> Unix socket FD passing
  -> api1 / api2
  -> mmap specialist index
  -> exact kNN
```

The load balancer accepts client connections on `:9999` and passes accepted file descriptors to API workers through `SCM_RIGHTS`. The API process receives the connected socket and handles the HTTP request directly. This avoids a second HTTP proxy hop between the LB and APIs.

## Why mmap

The reference set is large and read-only at runtime. The index is generated during the Docker build and copied into the final image. At startup, the API memory maps the index file instead of parsing JSON or allocating a large in-memory structure.

That gives two practical benefits:

- startup is mostly opening and mapping the index;
- the kernel can page the index efficiently and share mapped pages between API instances.

## Why a preprocessed index

The official reference data does not change while the image runs. Preprocessing lets the runtime avoid decompression, JSON parsing, normalization, partitioning, and tree construction. The Docker build bakes the chosen `scale` and `leaf_size` into the generated index.

The index header records the quantization scale. The runtime validates it against the binary scale on load so an old index cannot silently run with a new vector representation.

## Specialist partitioning

The search is still exact kNN. Partitioning and bounding boxes are used to reduce the number of scanned blocks without changing the result semantics.

The current default uses:

- quantization scale `10000`;
- leaf size `48`;
- `key-first` search mode.

`key-first` checks partitions with the same derived feature key first, then falls back to lower-bound ordering for the remaining partitions. The implementation keeps the exact kNN result by continuing to search any partition whose lower bound can still beat the current top-k distance.

## Trade-offs

- Higher quantization scale improves boundary behavior near `fraud_score=0.6`, but must stay within the safe arithmetic range of the SIMD distance path.
- Smaller leaves reduce scanned blocks at the cost of more tree nodes and a larger index.
- FD passing reduces proxy overhead but couples the API and LB around Linux/Unix socket behavior.
- The approach prioritizes local p99 and correctness for the official workload over being a general-purpose fraud scoring service.
