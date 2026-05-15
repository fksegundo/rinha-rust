# Stage 1: Build
FROM rust:bookworm AS build

ARG RUST_TARGET_CPU=haswell
ENV RUSTFLAGS="-C target-cpu=${RUST_TARGET_CPU} -C panic=abort -C link-arg=-Wl,--gc-sections"

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY bins ./bins

RUN cargo build --release --bins

# Stage 2: Preprocess
FROM rust:bookworm AS preprocess

WORKDIR /app
COPY --from=build /app/target/release/preprocess /app/preprocess
COPY --from=build /app/target/release/api /app/api

ARG REFERENCES_URL=https://github.com/zanfranceschi/rinha-de-backend-2026/raw/main/resources/references.json.gz
ARG RINHA_NATIVE_LEAF_SIZE=64
RUN curl -L -o /app/references.json.gz ${REFERENCES_URL}

RUN mkdir -p /app/index && \
    RINHA_LEAF_SIZE=${RINHA_NATIVE_LEAF_SIZE} \
    /app/preprocess /app/references.json.gz /app/index/rinha-specialist.idx

# Stage 3: Runtime
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=preprocess /app/api /app/api
COPY --from=preprocess /app/index/rinha-specialist.idx /app/index/rinha-specialist.idx

ENV RINHA_INDEX_PATH=/app/index/rinha-specialist.idx
ENV BIND_ADDR=0.0.0.0:8080
ENV RINHA_SEARCH_MODE=key-first

ENTRYPOINT ["/app/api"]
