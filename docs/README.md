# Documentação - Rinha Rust

Esta pasta contém a documentação técnica detalhada do projeto Rinha Rust, uma implementação de alta performance em Rust para o desafio Rinha de Backend 2026.

## Documentos Disponíveis

### [Arquitetura](architecture.md)
Documentação completa da arquitetura do sistema, incluindo:
- Visão geral dos componentes principais
- Estrutura detalhada dos módulos (api, index, vector, http, fd_passing)
- Formato do índice e estruturas de dados
- Fluxo de requisição completo
- Configuração e orquestração
- Otimizações de performance

**Leitura recomendada:** Primeiro documento a ler para entender o sistema como um todo.

### [Decisões de Design](design-decisions.md)
Explicação detalhada das 20 principais decisões de design tomadas durante o desenvolvimento:
- Por que Rust?
- Build-time index generation
- Memory-mapped index
- Specialist partitioning
- Exact kNN com k=5
- Quantized vectors
- Minimal HTTP parser
- Precomputed HTTP responses
- Unix socket FD passing
- Memory locking (mlock/mlockall)
- Pretouch strategy
- Dual-path JSON parsing
- AVX2 distance calculation
- Thread pool com stack reduzido
- Mimalloc allocator
- Aggressive release profile
- CPU pinning via cpuset
- Logging desabilitado
- Warmup queries
- Early exit threshold

**Leitura recomendada:** Para entender o raciocínio por trás de cada decisão técnica.

### [Visão Técnica](technical-overview.md)
Visão técnica abrangente cobrindo:
- Resumo executivo e objetivos de design
- Stack tecnológica completa
- Arquitetura em camadas com diagramas
- Fluxo de dados detalhado (build time, startup, request processing)
- Estruturas de dados críticas
- Algoritmos principais (kNN search, partition search, AVX2)
- Otimizações de performance por categoria
- Configuração e tuning
- Métricas e monitoramento
- Troubleshooting
- Extensões futuras

**Leitura recomendada:** Para uma visão técnica completa e profunda do sistema.

### [README em Português](README.pt-BR.md)
Versão em português do README principal do projeto.

**Leitura recomendada:** Se você prefere documentação em português.

## Ordem de Leitura Sugerida

Para novos desenvolvedores ou analistas:

1. **[Arquitetura](architecture.md)** - Entenda a estrutura geral
2. **[Decisões de Design](design-decisions.md)** - Entenda o porquê das escolhas
3. **[Visão Técnica](technical-overview.md)** - Aprofunde-se nos detalhes técnicos

Para desenvolvedores experientes que querem modificar o código:

1. **[Arquitetura](architecture.md)** - Visão geral rápida
2. **[Visão Técnica](technical-overview.md)** - Detalhes de implementação
3. **[Decisões de Design](design-decisions.md)** - Contexto das decisões

Para operadores/DevOps:

1. **[Visão Técnica](technical-overview.md)** - Seção de configuração e tuning
2. **[Arquitetura](architecture.md)** - Seção de orquestração
3. **[Decisões de Design](design-decisions.md)** - Entenda trade-offs operacionais

## Conceitos Chave

### Performance Extrema
O projeto foi projetado para latência mínima e throughput máximo dentro de limites estritos de recursos (1 CPU / 350MB). Cada decisão foi tomada com foco em performance.

### Especialização vs Generalização
A arquitetura é altamente especializada para o workload específico do desafio Rinha de Backend. Isso permite performance extrema, mas significa que o sistema não é apropriado para workloads genéricos.

### Exatidão Garantida
Apesar das otimizações, o sistema garante resultados exatos. O particionamento é usado para ordenação e pruning, não para aproximação.

### Zero-Copy e Zero-Allocation
Muitas técnicas são usadas para evitar cópias e alocações: mmap, FD passing, parsing sem alocação, respostas pré-computadas.

## Perguntas Frequentes

**Por que não usar um framework HTTP?**
Frameworks genéricos adicionam overhead significativo. O parser custom é focado apenas nos endpoints necessários e não aloca memória.

**Por que build-time index generation?**
Elimina tempo de inicialização e permite otimizações estáticas que não seriam possíveis em runtime.

**Por que Unix socket FD passing?**
Elimina overhead de TCP entre load balancer e API, permitindo connection pooling e reduzindo latência.

**Por que quantização com i16?**
Reduz uso de memória em 4x e permite operações SIMD mais eficientes, com precisão suficiente para o problema.

**Por que exatidão em vez de aproximação?**
O desafio requer resultados exatos. As otimizações (partitioning, bounding boxes) garantem exatidão enquanto reduzem o espaço de busca.

**É portável para outras plataformas?**
Não. O projeto usa Linux-specific features (mmap, mlock, FD passing) e x86_64-specific features (AVX2).

## Contribuindo

Ao modificar o código, considere:

1. **Performance**: Qualquer mudança deve ser avaliada por impacto em latência/throughput
2. **Memory**: O sistema opera dentro de limites estritos de memória
3. **Determinismo**: Evite introduzir comportamento não-determinístico
4. **Corretude**: Mantenha as garantias de exatidão do kNN
5. **Documentação**: Atualize a documentação se mudanças significativas forem feitas

## Recursos Externos

- [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) - Desafio oficial
- [fksegundo/rinha-dotnetrust-lb](https://github.com/fksegundo/rinha-dotnetrust-lb) - Load balancer companion
- [Rust Book](https://doc.rust-lang.org/book/) - Documentação oficial de Rust
- [The Rustonomicon](https://doc.rust-lang.org/nomicon/) - Unsafe Rust documentation

## Licença

MIT - Veja o arquivo LICENSE no root do repositório.
