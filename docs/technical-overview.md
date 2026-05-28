# Visão Técnica - Rinha Rust

## Resumo Executivo

Este projeto é uma implementação de alta performance em Rust para o desafio Rinha de Backend 2026, focado em scoring de fraude com latência mínima. A arquitetura foi projetada do zero para maximizar throughput e minimizar latência através de decisões deliberadas em cada camada da stack.

## Objetivos de Design

1. **Latência Mínima**: Cada decisão foi tomada para reduzir latência
2. **Throughput Máximo**: Otimizado para alto volume de requisições
3. **Determinístico**: Comportamento previsível sem spikes
4. **Corretude**: Resultados exatos, não aproximados
5. **Eficiência de Recursos**: Opera dentro de limites estritos (1 CPU / 350MB)

## Stack Tecnológica

### Linguagem
- **Rust 2024 Edition**: Últimas features do Rust
- **No_std parcial**: Usa libc para syscalls diretos

### Dependências
- `libc`: Syscalls Linux (mmap, mlock, recvmsg, epoll)
- `flate2`: Descompressão gzip
- `serde_json`: Parsing JSON (fallback)
- `threadpool`: Thread pool simples
- `mimalloc`: Allocator de alta performance

### Infraestrutura
- **Docker multi-stage build**: Separa build, preprocess e runtime
- **Docker Compose**: Orquestração de múltiplas instâncias
- **Unix socket**: Comunicação inter-process via FD passing
- **Linux syscalls**: mmap, mlock, madvise, recvmsg, epoll

## Arquitetura em Camadas

```
┌─────────────────────────────────────────┐
│         Cliente HTTP                    │
└──────────────┬──────────────────────────┘
               │ TCP
┌──────────────▼──────────────────────────┐
│      Load Balancer Custom               │
│    (fksegundo/rinha-api-lb)            │
└──────────────┬──────────────────────────┘
               │ Unix Socket (SCM_RIGHTS)
┌──────────────▼──────────────────────────┐
│         API Instance                    │
│  ┌──────────────────────────────────┐  │
│  │  HTTP Parser (Minimalista)       │  │
│  └──────────────┬───────────────────┘  │
│                 │                       │
│  ┌──────────────▼───────────────────┐  │
│  │  Vector Parser (Dual-path)      │  │
│  └──────────────┬───────────────────┘  │
│                 │                       │
│  ┌──────────────▼───────────────────┐  │
│  │  Specialist kNN Index           │  │
│  │  - Partitioning                  │  │
│  │  - KD-tree per partition        │  │
│  │  - AVX2 distance calc           │  │
│  └──────────────┬───────────────────┘  │
│                 │                       │
│  ┌──────────────▼───────────────────┐  │
│  │  Precomputed Responses          │  │
│  └──────────────────────────────────┘  │
└─────────────────────────────────────────┘
               │ mmap
┌──────────────▼──────────────────────────┐
│    Index File (rinha-specialist.idx)    │
│    - Build-time generated               │
│    - Memory-mapped                     │
│    - Compact binary format             │
└─────────────────────────────────────────┘
```

## Fluxo de Dados Detalhado

### 1. Build Time (Docker Build)

```
references.json.gz (oficial)
    ↓
preprocess binary
    ↓
1. Download references.json.gz
2. Parse JSON
3. Quantizar vetores (f64 → i16)
4. Computar partition cuts
5. Particionar referências (até 256 partições)
6. Construir KD-tree por partição
7. Serializar em formato binário
    ↓
rinha-specialist.idx
    ↓
Docker image (API binary + index)
```

### 2. Runtime Startup

```
API binary start
    ↓
1. mmap index file
    ↓
2. madvise (MADV_WILLNEED, MADV_HUGEPAGE)
    ↓
3. mlock index (se habilitado)
    ↓
4. pretouch index (ler todas as páginas)
    ↓
5. Warmup queries (256 queries sintéticas)
    ↓
6. Warmup payload path (256 requests)
    ↓
7. Set ready flag = true
    ↓
8. Aceitar conexões
```

### 3. Request Processing (FD Passing Mode)

```
Client → LB (TCP)
    ↓
LB aceita conexão TCP
    ↓
LB parse HTTP
    ↓
LB envia FD via Unix socket (SCM_RIGHTS)
    ↓
API recebe FD (recvmsg)
    ↓
API converte FD para TcpStream
    ↓
API executa handler em thread pool
    ↓
HTTP parser (minimalista)
    ↓
Vector parser (dual-path)
    ├─ Try custom parser (zero-allocation)
    └─ Fallback to serde_json se falhar
    ↓
Specialist kNN search
    ├─ Compute partition key
    ├─ Search matching partition (key-first)
    ├─ Compute lower bounds de outras partições
    ├─ Sort partitions por bound
    ├─ Visit partitions enquanto bound < best_dist[k-1]
    ├─ AVX2 distance calculation (8 vectors por vez)
    └─ Return fraud count (0-5)
    ↓
Select precomputed response
    ↓
Write response to socket
    ↓
Connection keep-alive ou close
```

## Estruturas de Dados Críticas

### 1. Query Vector (16 dimensions)
```rust
pub type QueryVector = [i16; 16];
```
- 14 dimensões reais + 2 padding para AVX2 alignment
- Quantizado com escala 10000
- Range: [-10000, 10000]

### 2. Index Format (RNSPCST2)
```
Header (46 bytes)
├─ Magic: "RNSPCST2" (8)
├─ Scale: i32 (4)
├─ Packed dims: i32 (4)
├─ Reference count: i32 (4)
├─ Partition count: i32 (4)
├─ Node count: i32 (4)
├─ Block count: i32 (4)
└─ Partition cuts v0: [i16; 7] (14)

Partitions (80 bytes cada)
├─ Key: u32 (4)
├─ Root: i32 (4)
├─ Start: i32 (4) - unused
├─ Len: i32 (4) - unused
├─ Min: [i16; 16] (32)
└─ Max: [i16; 16] (32)

Nodes (80 bytes cada)
├─ Left: i32 (4)
├─ Right: i32 (4)
├─ Start: i32 (4)
├─ Len: i32 (4)
├─ Min: [i16; 16] (32)
└─ Max: [i16; 16] (32)

Vectors (block layout)
├─ 8 vectors de 14 dims cada
├─ Organizado para AVX2 scanning
└─ Total: total_blocks * 14 * 8 * 2 bytes

Labels (1 byte por vector)
└─ Total: total_blocks * 8 bytes
```

### 3. Partition Key (8 bits)
```
Bit 0: vector[9] > 0 (is_online)
Bit 1: vector[10] > 0 (card_present)
Bit 2: vector[11] > 0 (feature derivada)
Bit 3: vector[8] > 2048 (tx_count_24h threshold)
Bit 4: vector[2] > 4096 (amount threshold)
Bits 5-7: bucket8_equifreq_v0(vector[0])
```

## Algoritmos Principais

### 1. Specialist kNN Search

```pseudo
function predict_fraud_count(query):
    best_dists = [∞, ∞, ∞, ∞, ∞]
    best_labels = [?, ?, ?, ?, ?]
    
    // Key-first: busca partição correspondente
    query_key = compute_partition_key(query)
    matching_partition = find_partition(query_key)
    if matching_partition exists:
        search_partition(matching_partition, query, best_dists, best_labels)
        if early_exit_enabled and best_dists[4] < threshold:
            return sum(best_labels)
    
    // Outras partições em ordem de lower bound
    partitions_sorted = []
    for partition in all_partitions:
        if partition.key == query_key: continue
        bound = lower_bound_box(query, partition.min, partition.max)
        if bound < best_dists[4]:
            partitions_sorted.append((bound, partition))
    
    sort(partitions_sorted by bound)
    
    for (bound, partition) in partitions_sorted:
        if bound >= best_dists[4]: break
        search_partition(partition, query, best_dists, best_labels)
        if early_exit_enabled and best_dists[4] < threshold:
            break
    
    return sum(best_labels)
```

### 2. Partition Search (KD-tree)

```pseudo
function search_partition(root, query, best_dists, best_labels):
    stack = [(root, lower_bound(root))]
    
    while stack not empty:
        (node, bound) = stack.pop()
        if bound >= best_dists[4]: continue
        
        if node is leaf:
            scan_leaf(node, query, best_dists, best_labels)
        else:
            left_bound = lower_bound_box(query, node.left.min, node.left.max)
            right_bound = lower_bound_box(query, node.right.min, node.right.max)
            
            // Visit nearer child first
            if left_bound <= right_bound:
                if right_bound < best_dists[4]:
                    stack.push((node.right, right_bound))
                stack.push((node.left, left_bound))
            else:
                if left_bound < best_dists[4]:
                    stack.push((node.left, left_bound))
                stack.push((node.right, right_bound))
```

### 3. AVX2 Distance Calculation

```pseudo
function scan_block_avx2(vectors, block_base, query):
    // Processa 8 vectors simultaneamente
    for dim in 0..14 step 2:
        // Carrega 8 valores de query para 2 registers
        q_dim = broadcast(query[dim])
        q_dim1 = broadcast(query[dim+1])
        
        // Carrega 8 valores do vetor
        v_dim = load_8x_i16(vectors + block_base + dim*8)
        v_dim1 = load_8x_i16(vectors + block_base + (dim+1)*8)
        
        // Calcula diferença
        diff = q_dim - v_dim
        diff1 = q_dim1 - v_dim1
        
        // Calcula quadrado (usando madd_epi16)
        sq = diff * diff + diff1 * diff1
        
        // Acumula em 64-bit
        sum += sq
    
    return sum // 8 distâncias
```

## Otimizações de Performance

### Memory
- **mmap**: Zero-copy loading
- **mlock**: Evita page faults
- **pretouch**: Pré-carrega páginas
- **mimalloc**: Allocator rápido
- **hugepages**: Reduz TLB misses
- **Quantização**: 4x menos memória (i16 vs f64)

### CPU
- **AVX2**: 4x speedup em distâncias
- **LTO**: Otimizações cross-crate
- **Single codegen unit**: Melhor inlining
- **Compile-time constants**: Scale em compile-time
- **CPU pinning**: Evita migration
- **Small stack**: 64KB vs 8MB default

### I/O
- **FD passing**: Elimina TCP overhead
- **Unix socket**: Faster que TCP local
- **No logging**: Remove I/O síncrono
- **Precomputed responses**: Zero allocation
- **Keep-alive**: Reusa conexões

### Algorithm
- **Specialist partitioning**: Reduz espaço de busca
- **Bounding box pruning**: Corta branches
- **Key-first search**: Partição mais provável primeiro
- **Early exit**: Para quando threshold atingido
- **Exact kNN**: Garante corretude

## Configuração e Tuning

### Variáveis de Ambiente

**Index:**
- `RINHA_INDEX_PATH`: Path do índice (default: /app/index/rinha-specialist.idx)
- `RINHA_MLOCK_INDEX`: Habilita mlock do índice (default: 0)
- `RINHA_PRETOUCH_INDEX`: Habilita pretouch (default: 1)

**Runtime:**
- `RINHA_MLOCK_ALL`: Habilita mlockall (default: 0)
- `RINHA_MLOCK_ALL_MODE`: Mode do mlockall (current/future/current-future)
- `RINHA_WARMUP_QUERIES`: Número de warmup queries (default: 256)
- `RINHA_PAYLOAD_WARMUP_REQUESTS`: Warmup do payload path (default: 256)
- `RINHA_THREAD_POOL_SIZE`: Tamanho do thread pool (default: 56)
- `RINHA_EARLY_EXIT_THRESHOLD`: Threshold para early exit (default: 0)

**Network:**
- `BIND_ADDR`: Endereço TCP (default: 0.0.0.0:8080)
- `RINHA_FD_SOCKET`: Path do Unix socket para FD passing
- `RINHA_FD_EVENTED`: Habilita mode evented (default: 1)

**Build:**
- `RINHA_NATIVE_SCALE`: Escala de quantização (default: 10000)
- `RINHA_LEAF_SIZE`: Tamanho do leaf KD-tree (default: 48)
- `RINHA_KD_SPLIT_STRATEGY`: Estratégia de split (widest/variance)

### Docker Compose

**Resource Limits:**
- API 1: 0.45 CPU, 170MB
- API 2: 0.45 CPU, 170MB
- LB: 0.1 CPU, 10MB
- Total: 1.0 CPU, 350MB

**CPU Pinning:**
- API 1: CPU 0
- API 2: CPU 1
- LB: CPUs 2,3

**Volume:**
- sockets: tmpfs 10MB

## Métricas e Monitoramento

### Métricas Disponíveis

**Index Metadata:**
- reference_count
- partition_count
- node_count
- block_count

**Search Stats (opcional):**
- partitions_visited
- nodes_visited
- leaves_scanned
- blocks_scanned

### Métricas de Performance

**Latência:**
- P50: < 1ms (alvo)
- P99: < 5ms (alvo)
- P999: < 10ms (alvo)

**Throughput:**
- Requests por segundo por instância
- Total throughput com 2 instâncias

**Resource Usage:**
- CPU: < 45% por instância
- Memory: < 170MB por instância
- Network: Minimal (FD passing)

## Troubleshooting

### Startup Issues

**Índice não carrega:**
- Verifique se RINHA_INDEX_PATH está correto
- Verifique permissões do arquivo
- Verifique se formato é RNSPCST2 (não RNSPCST1)

**mlock falha:**
- Verifique se container tem CAP_IPC_LOCK
- Reduza RINHA_MLOCK_ALL para "future"
- Desabilite mlock se não necessário

### Runtime Issues

**Latência alta:**
- Verifique se RINHA_PRETOUCH_INDEX=1
- Verifique se hugepages estão habilitadas
- Aumente RINHA_WARMUP_QUERIES
- Verifique CPU pinning

**Memory usage alto:**
- Verifique se thread pool não está muito grande
- Reduza stack size se necessário
- Verifique se não há memory leak

**Throughput baixo:**
- Verifique se RINHA_FD_EVENTED=1
- Aumente RINHA_THREAD_POOL_SIZE
- Verifique se LB não é bottleneck

## Extensões Futuras

### Possíveis Melhorias

1. **SIMD mais agressivo**: Usar AVX-512 se disponível
2. **GPU acceleration**: Offload kNN para GPU
3. **Approximate search**: Permitir busca aproximada para trade-off
4. **Dynamic indexing**: Atualizar índice em runtime
5. **Metrics endpoint**: Expor métricas via HTTP
6. **Config reload**: Reload de configuração sem restart

### Limitações Atuais

1. **Linux-only**: FD passing e mlock são Linux-specific
2. **x86_64 only**: AVX2 é x86-specific
3. **k=5 fixo**: Não configurável em runtime
4. **6 respostas fixas**: Não extensível para outros scores
5. **Build-time index**: Não atualizável em runtime

## Conclusão

Este projeto demonstra como decisões deliberadas em cada camada da stack podem resultar em um sistema de alta performance. O foco em latência mínima e throughput máximo, combinado com otimizações específicas para o workload, resultou em uma implementação que opera eficientemente dentro de limites estritos de recursos.

A arquitetura não é generalista - é altamente especializada para o desafio Rinha de Backend. Esta especialização é exatamente o que permite performance extrema, mas também significa que o sistema não é apropriado para workloads genéricos.
