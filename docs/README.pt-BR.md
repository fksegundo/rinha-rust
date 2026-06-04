# Rinha de Backend 2026 - Fraud Scoring de Baixa Latencia em Rust

[![Rust CI](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--api-blue)](https://github.com/fksegundo/rinha-rust/pkgs/container/rinha-rust-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](../LICENSE)

Implementacao em Rust para o desafio [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026).

Este repositorio esta organizado como um estudo de arquitetura backend de baixa latencia: preprocessamento no build da imagem, startup com `mmap`, indice especialista de kNN exato, caminho HTTP/1.1 minimo, respostas fixas de score de fraude e repasse de file descriptors via Unix socket usando um load balancer customizado.

English version: [../README.md](../README.md)

## Por Que Este Projeto E Interessante

Esta nao e uma API HTTP baseada em framework. A implementacao mantem o caminho de requisicao pequeno para gastar a maior parte do orcamento de CPU no lookup do score de fraude, nao em infraestrutura generica de requisicao.

Escolhas principais:

- geracao do indice no build a partir do `references.json.gz` oficial;
- carregamento do indice em runtime com `mmap`;
- score kNN exato com `k = 5`;
- vetores quantizados usando uma escala definida no build;
- particionamento especialista e poda por bounding boxes preservando exatidao;
- calculo de distancia com AVX2 no build de producao;
- parser HTTP minimo para os endpoints do desafio;
- respostas HTTP precomputadas para os seis scores de fraude possiveis;
- modo de FD passing via Unix socket para o load balancer companion.

A API em runtime espera FD passing. Cada processo de API escuta em `RINHA_FD_SOCKET` ou, por padrao, em `/sockets/${HOSTNAME}.sock`. O load balancer escuta em `LB_PORT` e distribui os file descriptors TCP aceitos para a lista `API_SOCKETS`, separada por virgulas.

## Arquitetura

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

A imagem Docker compila dois binarios Rust:

- `api`: atende `/ready` e `/fraud-score`;
- `lb`: aceita trafego TCP e repassa file descriptors para os Unix sockets das APIs;
- `preprocess`: converte o arquivo oficial de referencias para o indice compacto de runtime.

Durante o build da imagem, o `preprocess` baixa e converte o arquivo oficial de referencias para `/app/index/rinha-specialist.idx`. Em runtime, a API mapeia esse arquivo em memoria, aquece um pequeno conjunto de queries sinteticas e atende requisicoes a partir do indice mapeado.

Mais detalhes: [Detalhamento da arquitetura](architecture.md)

## Modelo de Corretude

O score de fraude e derivado dos rotulos dos cinco vetores de referencia mais proximos:

```text
fraud_score = fraud_count_among_5_nearest_neighbors / 5
```

A busca particionada continua sendo exata. A particao especialista correspondente a query e pesquisada primeiro, mas outras particoes ainda sao visitadas quando o lower bound delas pode melhorar o top-k atual. O particionamento serve para ordenar e podar a busca, nao para uma consulta aproximada.

## Endpoints

| Metodo | Caminho | Descricao |
| --- | --- | --- |
| `GET` | `/ready` | Probe de readiness |
| `POST` | `/fraud-score` | Recebe o payload do desafio e retorna a decisao de aprovacao com o score de fraude |

Exemplo de resposta:

```json
{
  "approved": true,
  "fraud_score": 0.2
}
```

## Notas de Performance

A topologia compose atual reserva o orcamento oficial de `1.0 CPU / 350 MB` assim:

| Componente | CPU | Memoria |
| --- | ---: | ---: |
| API 1 | `0.42` | `165M` |
| API 2 | `0.42` | `165M` |
| LB | `0.16` | `20M` |

Os defaults de runtime estao ajustados para a implementacao atual:

| Configuracao | Default |
| --- | --- |
| Escala dos vetores | `10000` |
| Leaf size do indice | `56` no build Docker |
| Porta do LB | `9999` |
| Upstreams do LB | `/sockets/api1.sock,/sockets/api2.sock` |
| Queries de warmup | `2048` |
| Travamento do indice em memoria | ativado no runtime Docker |

Notas detalhadas de implementacao e tuning: [Notas de performance](performance.md)

## Desenvolvimento Local

Executar testes:

```bash
make test
```

Build da imagem local da API:

```bash
make build
```

Validar o compose local:

```bash
make config
```

Subir a stack local:

```bash
make up
```

Parar a stack local:

```bash
make down
```

O compose local espera as imagens `rinha-rust-api:local` e `rinha-rust-lb:local`. O LB e configurado com `LB_PORT`, `LB_BACKLOG`, `LB_ACCEPT_BATCH` e `API_SOCKETS`.

## Imagem Docker

A imagem da API e publicada no GitHub Container Registry pelo workflow do repositorio:

```bash
docker pull ghcr.io/fksegundo/rinha-rust-api:latest
```

O compose publico esta em [docker/docker-compose.yml](../docker/docker-compose.yml).

## Estrutura do Projeto

```text
src/bin/api.rs          entrypoint do binario da API
src/bin/preprocess.rs   entrypoint do gerador de indice em build-time
src/api/                startup do servidor, roteamento e opcoes de runtime
src/fd_passing/         recebimento de file descriptors via SCM_RIGHTS
src/http/               parser HTTP/1.1 minimo e respostas fixas
src/index/              formato, builder, loader mmap e busca kNN exata
src/vector/             parser do payload do desafio e quantizacao do vetor
docker/Dockerfile       build multi-stage da imagem da API
docker/compose.local.yml
docker/compose.submission.yml
docs/                   arquitetura, performance e README em portugues
info.json               metadados do desafio
```

## Repositorios Relacionados

- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb) - load balancer customizado usado por esta submissao.
- [zanfranceschi/rinha-de-backend-2026](https://github.com/zanfranceschi/rinha-de-backend-2026) - repositorio oficial do desafio.

## Licenca

MIT
