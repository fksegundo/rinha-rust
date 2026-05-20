# Rinha de Backend 2026 - Rust submission

This repository contains my Rust submission for [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026).

The challenge is to build a backend service that receives transaction payloads and returns a fraud score response under a constrained Docker Compose environment. The final score depends on correctness, latency and resource usage during the official benchmark.

The public Docker image is published by the repository workflow:

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

The challenge submission uses this API image together with a companion load balancer image.

## Repository

```text
src/                  Rust source code
docker/               Dockerfile and local compose topology
info.json             challenge metadata
```

## Local checks

```bash
cargo test
docker build -f docker/Dockerfile -t rinha-rust-api:local .
docker compose -f docker/docker-compose.yml config -q
```

## License

MIT



# Rinha de Backend 2026 - Submissao em Rust

English version: [README.md](README.md)

Este repositorio contem minha submissao em Rust para a [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026).

O desafio consiste em criar um backend que recebe payloads de transacao e retorna uma resposta com score de fraude dentro de um ambiente Docker Compose com recursos limitados. A pontuacao final depende de corretude, latencia e uso de recursos durante o benchmark oficial.

A imagem publica da API e publicada pelo workflow do repositorio:

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

A submissao do desafio usa esta imagem da API junto com uma imagem companion de load balancer.

## Repositorio

```text
src/                  codigo Rust
docker/               Dockerfile e topologia compose local
info.json             metadados do desafio
```

## Checagens locais

```bash
cargo test
docker build -f docker/Dockerfile -t rinha-rust-api:local .
docker compose -f docker/docker-compose.yml config -q
```

## Licenca

MIT
