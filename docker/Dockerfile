FROM rust:bookworm AS build

ARG RUST_TARGET_CPU=haswell
ARG RINHA_NATIVE_SCALE=10000
ARG RINHA_CARGO_FEATURES=
ENV RUSTFLAGS="-C target-cpu=${RUST_TARGET_CPU} -C panic=abort -C link-arg=-Wl,--gc-sections"
ENV RINHA_NATIVE_SCALE=${RINHA_NATIVE_SCALE}

WORKDIR /app

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

RUN if [ -n "${RINHA_CARGO_FEATURES}" ]; then \
        cargo build --release --features "${RINHA_CARGO_FEATURES}" --bin api --bin preprocess; \
    else \
        cargo build --release --bin api --bin preprocess; \
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

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=preprocess /app/api /app/api
COPY --from=preprocess /app/index/rinha-specialist.idx /app/index/rinha-specialist.idx

ENV RINHA_INDEX_PATH=/app/index/rinha-specialist.idx
ENV BIND_ADDR=0.0.0.0:8080
ENV RINHA_SEARCH_MODE=key-first
ENV RINHA_MLOCK_INDEX=0

ENTRYPOINT ["/app/api"]
