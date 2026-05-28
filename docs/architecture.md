# Arquitetura do Rinha Rust

## Visão Geral

Este projeto é uma implementação em Rust para o desafio Rinha de Backend 2026, focado em baixa latência para scoring de fraude. A arquitetura foi projetada para maximizar o desempenho através de decisões deliberadas de design que minimizam overhead e maximizam o uso eficiente de recursos.

## Componentes Principais

### 1. Binários

#### `api` (src/bin/api.rs)
Binário principal que serve as requisições HTTP. Responsável por:
- Carregar o índice via mmap
- Executar warmup do índice e do path de parsing
- Aceitar conexões TCP ou via Unix socket FD passing
- Processar requisições `/ready` e `/fraud-score`

#### `preprocess` (src/bin/preprocess.rs)
Binário de build-time que converte o arquivo oficial `references.json.gz` em um índice compacto e otimizado para runtime. Executado durante o build da imagem Docker.

### 2. Módulos do Sistema

#### `src/api/mod.rs` - Servidor e Runtime
Gerencia o ciclo de vida da aplicação:
- **Memory Locking**: Usa `mlockall` e `mlock` para evitar page faults
- **Pretouch**: Acessa todas as páginas do índice para pré-carregar na memória
- **Warmup**: Executa queries sintéticas para warmup do índice e do path de parsing
- **Thread Pool**: Usa threadpool com stack size de 64KB para minimizar uso de memória
- **Modos de Operação**: TCP mode ou FD passing mode (Unix socket)

#### `src/index/` - Índice Especialista kNN
Implementação de índice especialista para busca kNN exata:

**Arquivos:**
- `mod.rs`: Core do índice, busca kNN, mmap loading
- `build.rs`: Construção do índice a partir de referências
- `format.rs`: Formato on-disk do índice
- `layout.rs`: Layout de memória dos registros

**Estrutura do Índice:**
- **Particionamento**: Divide referências em até 256 partições baseadas em características do vetor
- **KD-Tree**: Cada partição tem sua própria KD-tree
- **Bounding Boxes**: Cada nó tem min/max para pruning eficiente
- **Busca Exata**: Garante resultados exatos através de busca em múltiplas partições quando necessário
- **k=5**: Sempre busca os 5 vizinhos mais próximos

**Algoritmo de Busca:**
1. Computa a partition key da query
2. Busca na partição correspondente (key-first)
3. Calcula lower bound das outras partições
4. Ordena partições por lower bound
5. Visita partições enquanto o bound pode melhorar o top-k atual
6. Usa AVX2 para cálculos de distância quando disponível

#### `src/vector/mod.rs` - Parsing e Quantização
Responsável por converter o payload JSON em vetor quantizado:

**Dual-Path Parsing:**
1. **Single-Pass Custom Parser**: Parser manual zero-allocation que faz uma única passagem
2. **Serde Fallback**: Usa serde_json como fallback se o parser custom falhar

**Extração de Features:**
- 14 dimensões extraídas do payload
- Quantização com escala configurável (default: 10000)
- Hash de merchant IDs para feature embedding
- Parsing de timestamps para features temporais
- Tratamento de campos opcionais (last_transaction)

#### `src/http/mod.rs` - HTTP Minimalista
Parser HTTP/1.1 minimalista focado apenas nos endpoints necessários:

**Características:**
- Parsing sem alocação de memória
- Suporte a keep-alive e pipelining
- Rejeição early de requests inválidos
- Respostas pré-computadas para todos os scores de fraude (0.0, 0.2, 0.4, 0.6, 0.8, 1.0)
- Buffer fixo de 2048 bytes

**Respostas Pré-Computadas:**
```rust
pub const FRAUD_RESPONSES: [&[u8]; 6] = [
    RESPONSE_FRAUD_0,  // {"approved":true,"fraud_score":0.0}
    RESPONSE_FRAUD_1,  // {"approved":true,"fraud_score":0.2}
    RESPONSE_FRAUD_2,  // {"approved":true,"fraud_score":0.4}
    RESPONSE_FRAUD_3,  // {"approved":false,"fraud_score":0.6}
    RESPONSE_FRAUD_4,  // {"approved":false,"fraud_score":0.8}
    RESPONSE_FRAUD_5,  // {"approved":false,"fraud_score":1.0}
];
```

#### `src/fd_passing/mod.rs` - Unix Socket FD Passing
Implementação de recebimento de file descriptors via Unix socket usando SCM_RIGHTS:

**Modos:**
- **Thread Pool Mode**: Cada FD recebido é processado em uma thread do pool
- **Evented Mode**: Usa epoll para event-driven processing (mais eficiente)

**Por que FD Passing?**
- Evita overhead de TCP entre load balancer e API
- Permite que o load balancer faça connection pooling
- Reduz latência eliminando handshake TCP

### 3. Configuração e Build

#### `build.rs` - Build Script
Gera constante de escala em tempo de compilação:
- Lê variável de ambiente `RINHA_NATIVE_SCALE`
- Valida escala (1-11000)
- Escreve `scale.rs` em `OUT_DIR`
- Permite otimizações baseadas na escala em compile-time

#### `Cargo.toml` - Dependências e Profile
**Dependências Mínimas:**
- `libc`: Para syscalls Linux (mmap, mlock, recvmsg)
- `flate2`: Para descompressão do references.json.gz
- `serde_json`: Parsing JSON fallback
- `serde`: Derive macros
- `threadpool`: Thread pool simples
- `mimalloc`: Allocator alternativo (mais rápido que system allocator)

**Profile Release Agressivo:**
```toml
[profile.release]
opt-level = 3          # Máxima otimização
lto = "fat"            # Link-time optimization agressivo
codegen-units = 1      # Melhor otimização, build mais lento
panic = "abort"        # Remove overhead de unwinding
strip = true           # Remove símbolos de debug
debug = 0
overflow-checks = false
```

#### `docker/Dockerfile` - Multi-stage Build
**Stage 1: Build**
- Compila `api` e `preprocess` com otimizações
- Usa `target-cpu=haswell` para AVX2
- Passa escala via `RINHA_NATIVE_SCALE`

**Stage 2: Preprocess**
- Baixa references.json.gz oficial
- Executa preprocess para gerar índice
- Configura leaf size via `RINHA_NATIVE_LEAF_SIZE`

**Stage 3: Runtime**
- Imagem minimal debian:bookworm-slim
- Copia apenas binário e índice
- Configura environment variables

### 4. Orquestração

#### `docker-compose.local.yml` - Stack Local
**Serviços:**
- `api1`: API instance 1 (CPU 0, 170MB)
- `api2`: API instance 2 (CPU 1, 170MB)
- `lb`: Load balancer custom (CPU 2,3, 10MB)

**Configurações de Performance:**
- CPU pinning via `cpuset`
- Memory limits tight
- Unix socket volume (tmpfs 10MB)
- ulimits elevados (nofile: 65535, memlock: unlimited)
- Logging desabilitado para reduzir overhead

**Variáveis de Ambiente:**
- `RINHA_FD_SOCKET`: Path do Unix socket para FD passing
- `RINHA_PRETOUCH_INDEX`: Habilita pretouch do índice
- `RINHA_MLOCK_INDEX`: Habilita mlock do índice

## Fluxo de Requisição

### 1. Build Time
```
references.json.gz → preprocess → rinha-specialist.idx
```

### 2. Startup
```
API binary → mmap index → pretouch → warmup → ready
```

### 3. Request (FD Passing Mode)
```
Client → LB (TCP) → Unix socket → FD passing → API → kNN search → response
```

### 4. Request (TCP Mode)
```
Client → API (TCP) → HTTP parse → vector extraction → kNN search → response
```

## Estrutura de Dados

### Formato do Índice (RNSPCST2)
```
Header (8 bytes magic + metadata)
  - Magic: "RNSPCST2"
  - Scale: i32
  - Packed dims: i32
  - Reference count: i32
  - Partition count: i32
  - Node count: i32
  - Block count: i32
  - Partition cuts v0: [i16; 7]

Partitions (partition_count * 80 bytes)
  - Key: u32
  - Root: i32
  - Start: i32 (unused)
  - Len: i32 (unused)
  - Min: [i16; 16]
  - Max: [i16; 16]

Nodes (node_count * 80 bytes)
  - Left: i32
  - Right: i32
  - Start: i32
  - Len: i32
  - Min: [i16; 16]
  - Max: [i16; 16]

Vectors (total_blocks * 14 * 8 * 2 bytes)
  - Block layout: 8 vectors de 14 dimensões cada
  - Organizado para AVX2 scanning

Labels (total_blocks * 8 bytes)
  - 1 byte por vector (0 = legit, 1 = fraud)
```

### Partition Key (8 bits)
```
Bit 0: vector[9] > 0 (is_online)
Bit 1: vector[10] > 0 (card_present)
Bit 2: vector[11] > 0 (derived feature)
Bit 3: vector[8] > 2048 (tx_count_24h threshold)
Bit 4: vector[2] > 4096 (amount threshold)
Bits 5-7: bucket8_equifreq_v0(vector[0])
```

## Performance Optimizations

### Memory
- **mmap**: Carrega índice sem cópia
- **mlock**: Evita page faults
- **pretouch**: Pré-carrega páginas
- **mimalloc**: Allocator mais rápido
- **hugepages**: Usa hugepages quando disponível

### CPU
- **AVX2**: SIMD para cálculos de distância
- **Compile-time scale**: Constantes em tempo de compilação
- **LTO**: Link-time optimization
- **Single codegen unit**: Melhor inlining
- **CPU pinning**: Evita cache thrashing

### I/O
- **FD passing**: Evita TCP overhead
- **Unix socket**: Faster que TCP local
- **No logging**: Remove I/O de logging
- **Precomputed responses**: Zero allocation de response

### Algorithm
- **Specialist partitioning**: Reduz espaço de busca
- **Bounding box pruning**: Corta branches não promissores
- **Key-first search**: Busca partição mais provável primeiro
- **Early exit**: Para busca quando threshold é atingido

## Corretude

### Modelo de Scoring
```
fraud_score = fraud_count_among_5_nearest_neighbors / 5
approved = fraud_score < 0.5
```

### Garantias de Exatidão
- Busca é exata, não aproximada
- Partitioning é usado para ordenação, não para aproximação
- Todas as partições são visitadas quando necessário
- Lower bound pruning nunca descarta resultados corretos
