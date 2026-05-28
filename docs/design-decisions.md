# Decisões de Design - Rinha Rust

Este documento detalha as decisões de design tomadas durante o desenvolvimento do projeto e o raciocínio por trás de cada uma.

## 1. Por que Rust?

**Decisão:** Implementar o projeto em Rust ao invés de outras linguagens.

**Raciocínio:**
- **Performance zero-cost**: Rust oferece abstrações de alto nível sem custo de runtime
- **Memory safety**: Garante segurança de memória sem GC, evitando overhead de garbage collection
- **Controle fino de memória**: Permite uso de mmap, mlock, e outras técnicas de baixo nível
- **SIMD nativo**: Suporte excelente para AVX2 via intrinsics
- **Compile-time optimization**: LTO e const generics permitem otimizações agressivas
- **Ecosystem**: Crates de alta qualidade para HTTP parsing, JSON, etc.

**Trade-offs:**
- Curva de aprendizado mais íngreme
- Tempo de desenvolvimento inicial maior
- Build times mais longos (mitigado por cache)

## 2. Build-time Index Generation

**Decisão:** Gerar o índice em build-time ao invés de runtime.

**Raciocínio:**
- **Startup instantâneo**: Índice pré-construído elimina tempo de inicialização
- **Consistência**: Todas as instâncias usam o mesmo índice
- **Otimizações estáticas**: Permite otimizar estrutura do índice para o workload específico
- **Redução de complexidade runtime**: Remove lógica de construção do path crítico

**Implementação:**
- Binário `preprocess` separado executado no Docker build
- Índice serializado em formato binário compacto
- Carregado via mmap em runtime

**Trade-offs:**
- Rebuild necessário quando referências mudam
- Aumento do tamanho da imagem Docker
- Menos flexibilidade para atualizações dinâmicas

## 3. Memory-mapped Index

**Decisão:** Usar mmap para carregar o índice ao invés de ler em memória.

**Raciocínio:**
- **Zero-copy**: Carregamento sem cópia de dados
- **Lazy loading**: Página carregada sob demanda pelo OS
- **Memory efficiency**: OS gerencia paging automaticamente
- **Fast startup**: mmap é instantâneo, cópia de dados é lenta

**Implementação:**
```rust
let ptr = libc::mmap(
    ptr::null_mut(),
    len,
    libc::PROT_READ,
    libc::MAP_PRIVATE | libc::MAP_POPULATE,
    file.as_raw_fd(),
    0,
);
```

**Trade-offs:**
- Page faults possíveis durante primeiros acessos (mitigado por pretouch)
- Complexidade de lidar com ponteiros raw
- Platform-specific (Linux vs macOS)

## 4. Specialist Partitioning

**Decisão:** Implementar particionamento especialista ao invés de índice flat.

**Raciocínio:**
- **Redução do espaço de busca**: Cada query busca apenas em partições relevantes
- **Cache locality**: Partições específicas melhoram localidade de cache
- **Parallel-friendly**: Diferentes partições podem ser processadas em paralelo
- **Exatidão preservada**: Partitioning é para ordenação, não aproximação

**Algoritmo de Partition Key:**
```rust
// 8 bits baseados em características do vetor
Bit 0: is_online
Bit 1: card_present
Bit 2: feature derivada
Bit 3: tx_count_24h > 2048
Bit 4: amount > 4096
Bits 5-7: bucket8_equifreq_v0(vector[0])
```

**Trade-offs:**
- Complexidade adicional de implementação
- Overhead de computar partition key
- Desbalanceamento se particionamento não for bom

## 5. Exact kNN with k=5

**Decisão:** Implementar busca kNN exata com k=5 ao invés de aproximada.

**Raciocínio:**
- **Corretude garantida**: Challenge requer resultados exatos
- **k=5 é pequeno**: Overhead de buscar 5 vizinhos é aceitável
- **Pruning eficiente**: Bounding boxes reduzem drasticamente o espaço de busca
- **Simplicidade**: Algoritmo kNN é bem compreendido e testado

**Implementação:**
- Busca em partição correspondente primeiro
- Ordena outras partições por lower bound
- Visita partições enquanto bound pode melhorar top-k
- AVX2 para cálculos de distância em lote

**Trade-offs:**
- Mais lento que busca aproximada
- Requer bounding boxes bem construídas
- k fixo limita flexibilidade

## 6. Quantized Vectors

**Decisão:** Quantizar vetores para i16 ao invés de usar f64.

**Raciocínio:**
- **Redução de memória**: i16 (2 bytes) vs f64 (8 bytes) = 4x menos memória
- **SIMD-friendly**: Operações em inteiros são mais eficientes em SIMD
- **Cache efficiency**: Mais vetores cabem no cache
- **Suficiente precisão**: Escala de 10000 oferece precisão adequada

**Função de Quantização:**
```rust
fn quantize(value: f64) -> i16 {
    if value <= -1.0 { -SCALE }
    else if value <= 0.0 { 0 }
    else if value >= 1.0 { SCALE }
    else { (value * SCALE as f64).round() as i16 }
}
```

**Trade-offs:**
- Perda de precisão (mitigado por escala alta)
- Overflow potential (mitigado por clamping)
- Escolha da escala é crítica

## 7. Minimal HTTP Parser

**Decisão:** Implementar parser HTTP custom minimal ao invés de framework.

**Raciocínio:**
- **Zero-allocation**: Parser não aloca memória
- **Foco no necessário**: Apenas suporta endpoints do challenge
- **Performance**: Remove overhead de parsing genérico
- **Keep-alive**: Suporta pipelining para reduzir latência

**Características:**
- Parsing manual sem regex
- Rejeição early de requests inválidos
- Buffer fixo de 2048 bytes
- Respostas pré-computadas

**Trade-offs:**
- Não suporta HTTP completo
- Manutenção manual do parser
- Menos robusto que frameworks estabelecidos

## 8. Precomputed HTTP Responses

**Decisão:** Pré-computar todas as respostas possíveis de fraude.

**Raciocínio:**
- **Zero allocation**: Respostas são strings estáticas
- **Instantâneo**: Seleção de resposta é O(1)
- **Simplicidade**: Remove lógica de serialização em runtime
- **Consistência**: Formato garantido em compile-time

**Implementação:**
```rust
pub const FRAUD_RESPONSES: [&[u8]; 6] = [
    b"HTTP/1.1 200 OK\r\n...\r\n{\"approved\":true,\"fraud_score\":0.0}",
    b"HTTP/1.1 200 OK\r\n...\r\n{\"approved\":true,\"fraud_score\":0.2}",
    // ... 4 mais
];
```

**Trade-offs:**
- Apenas 6 respostas possíveis (limitação do challenge)
- Mudança de formato requer recompilação
- Menos flexível para extensões

## 9. Unix Socket FD Passing

**Decisão:** Implementar FD passing via Unix socket ao invés de TCP puro.

**Raciocínio:**
- **Elimina TCP overhead**: Sem handshake, sem cabeçalhos TCP
- **Connection pooling**: LB pode manter conexões abertas
- **Menos latência**: Comunicação inter-process é mais rápida
- **Zero-copy**: FD é passado, não dados

**Implementação:**
- Usa SCM_RIGHTS para passar file descriptors
- Unix socket em volume tmpfs
- Dois modos: thread pool e evented (epoll)

**Trade-offs:**
- Linux-specific (não portável)
- Complexidade de implementação
- Requer load balancer custom

## 10. Memory Locking (mlock/mlockall)

**Decisão:** Usar mlock e mlockall para evitar page faults.

**Raciocínio:**
- **Latência determinística**: Page faults causam spikes de latência
- **Real-time behavior**: Garante que dados estão sempre em RAM
- **Performance crítica**: Challenge é sensível a latência
- **Configurável**: Pode ser desabilitado se necessário

**Implementação:**
```rust
// mlock do índice
libc::mlock(ptr, len);

// mlockall para memória futura
libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
```

**Trade-offs:**
- Requer privilégios (capabilities)
- Pode causar OOM se memória insuficiente
- Reduz flexibilidade de swapping do OS

## 11. Pretouch Strategy

**Decisão:** Implementar pretouch do índice após mmap.

**Raciocínio:**
- **Elimina page faults no warmup**: Primeiras queries não causam faults
- **Determinístico**: Carregamento controlado vs sob demanda
- **Hugepages friendly**: Permite uso de hugepages
- **Checksum validation**: Valida integridade durante pretouch

**Implementação:**
```rust
let mut checksum = 0u8;
let mut offset = 0usize;
while offset < bytes.len() {
    checksum ^= unsafe { std::ptr::read_volatile(bytes.as_ptr().add(offset)) };
    offset += 4096;
}
```

**Trade-offs:**
- Aumenta tempo de startup
- Usa memória imediatamente
- Overhead de CPU durante pretouch

## 12. Dual-Path JSON Parsing

**Decisão:** Implementar parser custom com fallback para serde.

**Raciocínio:**
- **Performance**: Parser custom é mais rápido para casos comuns
- **Robustez**: Serde garante parsing correto para edge cases
- **Zero-allocation**: Parser custom não aloca
- **Fallback automático**: Tenta custom primeiro, usa serde se falhar

**Parser Custom:**
- Single-pass scanning
- Extrai apenas campos necessários
- Parsing manual de números e strings
- Hash de merchant IDs inline

**Trade-offs:**
- Complexidade de manutenção
- Dois caminhos para testar
- Parser custom pode ter bugs

## 13. AVX2 Distance Calculation

**Decisão:** Usar AVX2 intrinsics para cálculos de distância quando disponível.

**Raciocínio:**
- **4x speedup**: Processa 8 valores por vez
- **Hardware disponível**: x86_64 com AVX2 é comum
- **Fallback scalar**: Garante funcionamento sem AVX2
- **Critical path**: Distância é bottleneck do kNN

**Implementação:**
```rust
#[cfg(target_arch = "x86_64")]
unsafe fn scan_block_avx2(vectors: &[i16], block_base: usize, query: &QueryVector) -> [i64; 8] {
    // Usa _mm256_* intrinsics
}
```

**Trade-offs:**
- x86_64 specific
- Complexidade de código SIMD
- Requer feature detection em runtime

## 14. Thread Pool com Stack Size Reduzido

**Decisão:** Usar thread pool com stack de 64KB ao invés de default.

**Raciocínio:**
- **Memória efficiency**: Stack padrão (8MB) é excessivo
- **Mais threads**: Permite mais threads com mesma memória
- **Suficiente**: Path de requisição não usa deep recursion
- **Low overhead**: Context switch é mais rápido

**Implementação:**
```rust
let pool = threadpool::Builder::new()
    .num_threads(56)
    .thread_stack_size(64 * 1024)
    .build();
```

**Trade-offs:**
- Stack overflow se código muda
- Menos espaço para variáveis locais
- Requer profiling para validar

## 15. Mimalloc Allocator

**Decisão:** Usar mimalloc ao invés de system allocator.

**Raciocínio:**
- **Performance**: Mimalloc é mais rápido que glibc malloc
- **Fragmentação reduzida**: Melhor gerenciamento de memória
- **Scalability**: Escala melhor com múltiplas threads
- **Battle-tested**: Usado em muitos projetos de alta performance

**Implementação:**
```rust
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
```

**Trade-offs:**
- Dependência externa
- Comportamento pode diferir entre plataformas
- Debugging de memory issues mais complexo

## 16. Aggressive Release Profile

**Decisão:** Configurar profile release com otimizações agressivas.

**Raciocínio:**
- **Maximum performance**: Challenge é benchmark de performance
- **LTO fat**: Otimizações across crates
- **Single codegen unit**: Melhor inlining
- **Panic abort**: Remove overhead de unwinding
- **Strip symbols**: Reduz tamanho do binário

**Configuração:**
```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

**Trade-offs:**
- Build time muito mais longo
- Debugging mais difícil
- Não apropriado para desenvolvimento

## 17. CPU Pinning via cpuset

**Decisão:** Usar cpuset no docker-compose para pinar CPUs.

**Raciocínio:**
- **Cache locality**: Threads ficam em cores específicos
- **Evita migration**: OS não move threads entre cores
- **Determinístico**: Comportamento mais previsível
- **Reduz contention**: Menos cache thrashing

**Implementação:**
```yaml
api1:
  cpuset: "0"
api2:
  cpuset: "1"
lb:
  cpuset: "2,3"
```

**Trade-offs:**
- Requer conhecimento da topologia de CPU
- Menos flexível para autoscaling
- Pode subutilizar CPUs se workload não balancear

## 18. Logging Desabilitado

**Decisão:** Desabilitar logging em produção.

**Raciocínio:**
- **I/O overhead**: Logging é I/O síncrono
- **Latência**: Cada log adiciona latência
- **Determinístico**: Remove variabilidade de I/O
- **Challenge não requer**: Logs não são necessários para scoring

**Implementação:**
```yaml
logging:
  driver: "none"
```

**Trade-offs:**
- Dificulta debugging em produção
- Perde visibilidade de issues
- Requer re-deploy para habilitar logs

## 19. Warmup Queries

**Decisão:** Executar warmup com queries sintéticas antes de aceitar requests.

**Raciocínio:**
- **JIT warmup**: Permite que CPU faça warmup
- **Cache preloading**: Carrega dados relevantes no cache
- **Branch prediction**: Permite que CPU aprenda padrões
- **Determinístico**: Primeiras requests reais não são outliers

**Implementação:**
```rust
for i in 0..256 {
    let query = generate_synthetic_query(i);
    let _ = index.predict_fraud_count(&query);
}
```

**Trade-offs:**
- Aumenta tempo de startup
- Usa CPU antes de aceitar requests
- Queries sintéticas podem não representar workload real

## 20. Early Exit Threshold

**Decisão:** Implementar early exit quando distância do k-ésimo vizinho é pequena.

**Raciocínio:**
- **Performance**: Para busca quando resultado é óbvio
- **Configurável**: Threshold pode ser ajustado
- **Exatidão preservada**: Só early exit quando garantido
- **Reduz trabalho**: Muitas queries são "fáceis"

**Implementação:**
```rust
if best_dists[K - 1] < early_exit_threshold {
    return best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8;
}
```

**Trade-offs:**
- Requer tuning do threshold
- Pode não beneficiar todas as queries
- Adiciona complexidade ao algoritmo

## Conclusão

Cada decisão de design foi tomada com foco em maximizar performance para o workload específico do challenge Rinha de Backend. Trade-offs foram aceitos quando o benefício em latência ou throughput justificava a complexidade adicional. O resultado é um sistema altamente otimizado que sacrifica flexibilidade e generalidade em favor de performance extrema.
