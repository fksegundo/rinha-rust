# Rinha de Backend 2026 — Rust Submission

Implementation for the [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) challenge.

---

## About

This is a fully **Rust** submission that detects fraud score vectors via **exact k-NN search** (`k = 5`, Euclidean distance) over a pre-built specialist-partitioned index. The index is built at Docker image build time and `mmap`-loaded at startup so the API reaches readiness immediately.

- **Language:** Rust
- **Algorithm:** Exact k-NN with specialist partitioning (exact-safe pruning)
- **Protocol:** HTTP/1.1 with keep-alive, plus Unix-socket FD passing via SCM_RIGHTS
- **Topology:** 1 load balancer (`rinha-dotnetrust-lb`) → 2 Rust API instances

---

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET`  | `/ready` | Readiness probe |
| `POST` | `/fraud-score` | Accepts a JSON payload and returns the fraud count (`0..5`) |

---

## Project Structure

```
.
├── bins/               # Binary entrypoints (api, preprocess, verify)
├── src/
│   ├── api/            # HTTP server and request routing
│   ├── fd_passing/     # SCM_RIGHTS file-descriptor passing
│   ├── http/           # Minimal HTTP/1.1 parser
│   ├── index/          # Specialist index builder and mmap loader
│   └── vector/         # JSON payload → normalized i16 vector
├── submission/
│   ├── Dockerfile      # Multi-stage build (compile → preprocess → runtime)
│   └── docker-compose.yml # Official topology
├── Makefile            # Local build, benchmark and diagnostic targets
└── info.json           # Participant metadata
```

---

## Local Build & Test

Requirements: Docker, Docker Compose, curl, k6 (for tests)

```bash
# Build the API image locally
make build

# Build the LB image (expects ../rinha-dotnetrust-lb)
make build-lb

# Start the full stack
make up

# Run official k6 workload
make test-k6

# Full benchmark (build, run, test, teardown)
make bench-local
```

---

## Docker Image

The image is published automatically to GitHub Container Registry via the included GitHub Action.

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

---

## License

MIT

---

---

# Rinha de Backend 2026 — Submissão em Rust

Implementação para o desafio [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026).

---

## Sobre

Esta é uma submissão 100% **Rust** que detecta vetores de fraude via **busca exata k-NN** (`k = 5`, distância Euclidiana) sobre um índice especialista pré-construído. O índice é gerado no build da imagem Docker e carregado via `mmap` no startup, para que a API fique pronta instantaneamente.

- **Linguagem:** Rust
- **Algoritmo:** k-NN exato com particionamento especialista (poda exact-safe)
- **Protocolo:** HTTP/1.1 com keep-alive, e repasse de FD via Unix sockets (SCM_RIGHTS)
- **Topologia:** 1 load balancer (`rinha-dotnetrust-lb`) → 2 instâncias Rust

---

## Endpoints

| Método | Caminho | Descrição |
|--------|---------|-----------|
| `GET`  | `/ready` | Probe de readiness |
| `POST` | `/fraud-score` | Recebe JSON e retorna a contagem de fraudes (`0..5`) |

---

## Estrutura do Projeto

```
.
├── bins/               # Entrypoints dos binários (api, preprocess, verify)
├── src/
│   ├── api/            # Servidor HTTP e roteamento
│   ├── fd_passing/     # Repasse de FD via SCM_RIGHTS
│   ├── http/           # Parser HTTP/1.1 mínimo
│   ├── index/          # Construtor e loader mmap do índice especialista
│   └── vector/         # Payload JSON → vetor i16 normalizado
├── submission/
│   ├── Dockerfile      # Build multi-stage (compila → preprocess → runtime)
│   └── docker-compose.yml # Topologia oficial
├── Makefile            # Targets de build local, benchmark e diagnóstico
└── info.json           # Metadados do participante
```

---

## Build e Teste Local

Requisitos: Docker, Docker Compose, curl, k6 (para testes)

```bash
# Build da imagem da API localmente
make build

# Build da imagem do LB (espera ../rinha-dotnetrust-lb)
make build-lb

# Iniciar a stack completa
make up

# Rodar workload oficial do k6
make test-k6

# Benchmark completo (build, run, test, teardown)
make bench-local
```

---

## Imagem Docker

A imagem é publicada automaticamente no GitHub Container Registry pela GitHub Action inclusa.

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

---

## Licença

MIT
