# Rinha de Backend 2026 - Fraud scoring de baixa latência em Rust

[![Rust CI](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--api-blue)](https://github.com/fksegundo/rinha-rust/pkgs/container/rinha-rust-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](../LICENSE)

Implementação em Rust para o desafio [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026).

Este repositório foi organizado como um estudo de arquitetura de baixa latência: pré-processamento no build da imagem, startup com `mmap`, índice especialista para kNN exato, parser HTTP mínimo, respostas fixas para os possíveis scores e repasse de file descriptors via Unix socket usando um load balancer customizado.

Versão principal em inglês: [../README.md](../README.md)

O load balancer usado pela submissão fica em um repositório separado:

- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb)

## Por que este projeto é interessante

Esta não é uma API HTTP baseada em framework.

O hot path foi mantido intencionalmente pequeno:

- geração do índice no build da imagem a partir do arquivo oficial de referências;
- carregamento do índice em runtime via `mmap`;
- busca kNN exata com `k = 5`;
- vetores quantizados para reduzir custo de distância;
- particionamento especialista com poda exact-safe;
- cálculo de distância com SIMD quando AVX2 está disponível;
- parser HTTP/1.1 mínimo;
- respostas HTTP pré-computadas para os seis possíveis scores de fraude;
- repasse de socket FD do LB para os processos da API.

O objetivo é manter o startup rápido, reduzir alocações em runtime e gastar a maior parte do orçamento de CPU no lookup do score de fraude.

## Notas de performance

Benchmark local reproduzível mais recente na minha máquina, usando o dataset oficial do k6 e a topologia Docker Compose deste repositório:

| Métrica | Resultado |
| --- | --- |
| p99 local | `0.61ms` |
| score | `6000` |
| falsos positivos | `0` |
| falsos negativos | `0` |
| erros HTTP | `0` |
| ambiente | Docker Compose local, `900 rps`, `120s`, `250 max VUs` |
| recursos | `2 x API: 0.42 CPU / 165M`, `LB: 0.16 CPU / 20M` |
| estratégia | índice especialista no build, `mmap`, `scale=10000`, `leaf_size=48`, kNN exato `key-first`, FD passing |

O comando de benchmark é intencionalmente baseado no dataset oficial/padrão:

```bash
make bench-local
```

O `make test-k6` falha rapidamente se o dataset padrão não existir em:

```text
../rinha-de-backend-2026-main/test/test-data.json
```

Datasets estendidos existem apenas para testes locais de robustez e experimentos de tuning. Eles não substituem o fluxo de benchmark oficial.

Notas mais detalhadas de benchmark e matrizes de tuning estão documentadas em:

- [Notas de performance e matriz de benchmark](performance.md)

## Arquitetura

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

Durante o build da imagem, o arquivo oficial `references.json.gz` é baixado e convertido em um índice compacto especialista.

Em runtime, cada processo da API faz o mapeamento em memória do índice pré-gerado e atende requisições em `/fraud-score` usando busca kNN exata.

O LB aceita tráfego HTTP externo na porta `9999` e repassa os sockets TCP aceitos para as instâncias da API usando Unix socket descriptor passing. A API recebe o FD do socket e passa a tratar a conexão HTTP diretamente.

Mais detalhes estão disponíveis em:

- [Detalhamento da arquitetura](architecture.md)

## Modelo de corretude

O caminho otimizado `key-first` continua sendo exato.

A partição especialista que corresponde à chave da query é pesquisada primeiro, mas as demais partições ainda são consideradas quando o menor limite delas pode melhorar o conjunto top-k atual. Em outras palavras, o particionamento é usado para priorizar e podar a busca, não para transformar a consulta em uma busca aproximada.

O score final de fraude é derivado dos rótulos dos cinco vetores de referência mais próximos:

```text
fraud_score = fraud_count_among_5_nearest_neighbors / 5
```

## Endpoints

| Método | Caminho | Descrição |
| --- | --- | --- |
| `GET` | `/ready` | Probe de readiness |
| `POST` | `/fraud-score` | Recebe o payload do desafio e retorna a decisão de aprovação com o score de fraude |

Exemplo de resposta:

```json
{
  "approved": true,
  "fraud_score": 0.2
}
```

## Fluxo de benchmark

Requisitos para o benchmark padrão:

- Docker
- Docker Compose
- curl
- acesso ao checkout oficial do desafio em `../rinha-de-backend-2026-main`

Python 3 é necessário apenas para os scripts auxiliares de benchmark e tuning.

```bash
make build        # Build da imagem local da API com etapa de preprocess
make build-lb     # Build da imagem do LB customizado a partir de ../rinha-dotnetrust-lb
make up           # Sobe a topologia local completa em :9999
make test-k6      # Executa o dataset oficial do k6 contra a stack em execução
make bench-local  # Build, start, k6 oficial e teardown
make bench-diag   # Mesmo benchmark, com logs da API e docker stats
```

Auxiliares de tuning:

```bash
python3 scripts/run_scale_leaf_matrix.py
python3 scripts/run_resource_matrix.py --scale 10000 --leaf-size 48 --phase cpu
```

O build local padrão usa:

- `RINHA_NATIVE_SCALE=10000`
- `RINHA_NATIVE_LEAF_SIZE=48`
- `RINHA_SEARCH_MODE=key-first`
- `API_CPU_LIMIT=0.42`
- `LB_CPU_LIMIT=0.16`
- `API_MEMORY_LIMIT=165M`
- `LB_MEMORY_LIMIT=20M`

## Desenvolvimento local

Build da imagem da API:

```bash
make build
```

Build da imagem do LB companion:

```bash
make build-lb
```

Subir a stack:

```bash
make up
```

Checar readiness:

```bash
curl -i http://localhost:9999/ready
```

Executar o benchmark oficial:

```bash
make test-k6
```

Parar a stack:

```bash
make down
```

## Imagem Docker

A imagem da API é publicada no GitHub Container Registry pelo workflow do repositório.

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

O branch oficial de submissão referencia imagens públicas do GHCR para que o runner do desafio consiga iniciar a stack sem fazer build local.

## Estrutura do projeto

```text
bins/
  api.rs                 entrypoint do binário da API
  preprocess.rs          entrypoint do gerador de índice em build-time
  verify.rs              utilitário de verificação

src/
  api/                   servidor HTTP e roteamento
  fd_passing/            recebimento de file descriptors via SCM_RIGHTS
  http/                  parser HTTP/1.1 mínimo e respostas fixas
  index/                 builder do índice, loader mmap e busca kNN exata
  vector/                parser do payload JSON para vetor quantizado

submission/
  Dockerfile             build multi-stage: compile -> preprocess -> runtime
  docker-compose.yml     topologia local em compose

scripts/
  run_scale_leaf_matrix.py
  run_resource_matrix.py
  generate_extended_test_data.py

docs/
  architecture.md
  performance.md
  README.pt-BR.md

Makefile                 targets de build, benchmark, diagnóstico e cleanup
info.json                metadados do desafio
```

## Destaques de implementação

### Pré-processamento no build

O Docker build baixa o arquivo oficial de referências e o converte em um índice compacto antes da imagem runtime ser produzida.

Isso mantém o startup em runtime simples: a API só precisa mapear o índice em memória e aquecer o caminho de lookup.

### Startup com mmap

O índice é carregado com `mmap`, evitando uma leitura completa com cópia para estruturas próprias no heap durante o startup. A região mapeada dá suporte às seções de vetores e labels usadas pela busca.

### Índice especialista com kNN exato

O índice agrupa referências em partições especialistas e armazena bounding boxes para poda. A execução da query prioriza as partições e nós mais promissores, mas preserva a exatidão ao continuar pesquisando qualquer partição cujo lower bound ainda possa melhorar o resultado top-k atual.

### Caminho HTTP mínimo

A API implementa apenas o comportamento HTTP necessário para o desafio:

- `GET /ready`
- `POST /fraud-score`
- HTTP/1.1 keep-alive
- respostas JSON fixas para `fraud_score` de `0.0` até `1.0`

Isso evita o overhead de frameworks genéricos no hot path.

### FD passing

O LB companion customizado aceita a conexão externa e passa o file descriptor do socket aceito para uma das APIs por meio de um Unix socket usando `SCM_RIGHTS`.

Isso permite que a API trate diretamente a conexão do cliente depois do balanceamento, mantendo o handoff leve.

## Branch de submissão

O `main` mantém implementação, documentação e fluxo de desenvolvimento local.

O branch `submission` é reservado para o formato oficial de entrega do desafio, com os arquivos de submissão na raiz do repositório e referências para imagens públicas do GHCR.

## Repositórios relacionados

- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb) — load balancer companion usado por esta submissão.
- [zanfranceschi/rinha-de-backend-2026](https://github.com/zanfranceschi/rinha-de-backend-2026) — repositório oficial do desafio.

## Licença

MIT
