FROM rust:bookworm AS build

ARG RUST_TARGET_CPU=haswell
ARG RINHA_NATIVE_SCALE=10000
ENV RUSTFLAGS="-C target-cpu=${RUST_TARGET_CPU} -C panic=abort -C link-arg=-Wl,--gc-sections"
ENV RINHA_NATIVE_SCALE=${RINHA_NATIVE_SCALE}

WORKDIR /app

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

RUN if [ -n "${RINHA_CARGO_FEATURES}" ]; then \
        cargo build --release --features "${RINHA_CARGO_FEATURES}" --bin api --bin preprocess --bin lb; \
    else \
        cargo build --release --bin api --bin preprocess --bin lb; \
    fi

FROM rust:bookworm AS preprocess

WORKDIR /app
COPY --from=build /app/target/release/preprocess /app/preprocess
COPY --from=build /app/target/release/api /app/api

ARG REFERENCES_URL=https://github.com/zanfranceschi/rinha-de-backend-2026/raw/main/resources/references.json.gz
ARG RINHA_NATIVE_LEAF_SIZE=56
ARG RINHA_PARTITION_SCHEME=amt16_dow7
RUN curl -L -o /app/references.json.gz ${REFERENCES_URL}

RUN mkdir -p /app/index && \
    RINHA_LEAF_SIZE=${RINHA_NATIVE_LEAF_SIZE} \
    RINHA_PARTITION_SCHEME=${RINHA_PARTITION_SCHEME} \
    /app/preprocess /app/references.json.gz /app/index/rinha-specialist.idx

FROM debian:bookworm-slim AS api-runtime

RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=preprocess /app/api /app/api
COPY --from=preprocess /app/index/rinha-specialist.idx /app/index/rinha-specialist.idx

ENV RINHA_INDEX_PATH=/app/index/rinha-specialist.idx
ENV BIND_ADDR=0.0.0.0:8080
ENV RINHA_SEARCH_MODE=key-first
ENV RINHA_FD_EVENTED=1
ENV RINHA_CLIENT_FD_PRECONFIGURED=1
ENV RINHA_BUF_POOL_INIT=512
ENV RINHA_EPOLL_IDLE_US=60
ENV RINHA_ASSUME_KEEP_ALIVE=1
ENV RINHA_MLOCK_INDEX=1
ENV RINHA_PRETOUCH_INDEX=1
ENV RINHA_SPIN_BEFORE_BLOCK_US=0
ENV RINHA_BUSY_POLL_US=100
ENV RINHA_BUSY_POLL_BUDGET=8
ENV RINHA_PREFER_BUSY_POLL=1
ENV RINHA_PAYLOAD_WARMUP_REQUESTS=2048
ENV RINHA_WARMUP_QUERIES=2048

ENTRYPOINT ["/app/api"]

FROM debian:bookworm-slim AS lb-runtime

WORKDIR /app

COPY --from=build /app/target/release/lb /app/lb

ENV LB_PORT=9999
ENV LB_BACKLOG=65535
ENV LB_ACCEPT_BATCH=128
ENV API_SOCKETS=/sockets/api1.sock,/sockets/api2.sock

ENTRYPOINT ["/app/lb"]
