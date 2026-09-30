# SPEC — Decision Making & Future Search Graph

**Status:** Implementado V1 — tuning e calibração continuam  
**Branch alvo:** `dev`  
**Depende de:** `food-mode.md`, `hunting-mode.md`, `survival-mode.md`, `enemy-tracing.md`

## Estado da implementação

Implementado em `dev`:

- FutureGraph compartilhado com arena de nodes e transposition table por `StateKey`;
- `NodeAnalysis` único por estado, compartilhando rotas, mobilidade, Enemy Tracing e análise tática;
- horizonte base de três turnos completos e iterative deepening até seis quando o orçamento permite;
- budget global cobrindo expansão **e avaliação E2E**; uma profundidade só substitui a anterior quando ambas terminam;
- estimativa de próxima profundidade baseada em frontier, branching e custo observado;
- reserva de deadline baseada em timeout, latência e jitter recente da própria partida;
- avaliação root → leaf isolada por branch, sem somar eventos de siblings;
- Survival lexicográfico antes de qualquer recompensa;
- agressividade branch-local para ponderar Food versus Hunting;
- leaf potential separado de eventos realizados para reduzir horizon blindness;
- política de bordas/quinas com override apenas para resultado tático causal;
- rolling horizon por partida com prune, re-root e garbage collection;
- validação de food antes do lookup, distinguindo `FoodSpawn` e `FoodMutation`;
- telemetria por depth, direção, cache, Enemy Tracing, jitter e componentes estratégicos.

Rulesets com semântica ainda não simulada integralmente continuam usando o baseline conservador.

## 1. Objetivo

Decision Making é o orquestrador central da SnakeBollada.

Ele:

1. valida o estado real recebido;
2. decide se o FutureGraph anterior ainda pode ser reutilizado;
3. calcula uma análise central única por estado;
4. expande ações simultâneas de todas as cobras;
5. mantém horizonte inicial de três interações completas;
6. reutiliza futuros previamente calculados;
7. aumenta profundidade quando o orçamento permite;
8. avalia cada rota E2E sem misturar branches;
9. trata Survival como prioridade máxima;
10. usa agressividade para ponderar Food versus Hunting;
11. aplica política global de bordas e quinas;
12. retorna a direção atual.

## 2. Cinco componentes

```text
StateAnalysisEngine
EnemyTracing
Food Mode
Hunting Mode
Survival Mode
        |
        v
Decision/Search Engine
```

Os modos não constroem árvores próprias.

Existe um único FutureGraph compartilhado.

## 3. Pipeline de cada /move

A ordem é obrigatória:

```text
request
   |
   v
normalize actual state
   |
   v
validate food FIRST
   |
   +-- unexpected spawn/mutation -> clear graph
   |
   v
try graph reconciliation
   |
   +-- exact compatible state -> re-root
   +-- miss                   -> clear graph
   |
   v
StateAnalysis(root)
   |
   v
restore target depth
   |
   v
iterative deepening if budget allows
   |
   v
evaluate E2E routes
   |
   v
apply Survival priority
   |
   v
apply aggression Food/Hunt balance
   |
   v
apply reserved-cell policy
   |
   v
choose current move
   |
   v
prune unchosen current moves
   |
   v
keep future opponent responses
```

## 4. Food validation before cache

Food validation acontece antes de qualquer lookup/re-root.

Partimos do último estado real e da transição simulada selecionada anteriormente.

Calculamos o conjunto de comida esperado depois de consumos conhecidos.

```text
expected_food =
    previous_food
    - deterministically_consumed_food
```

Comparamos com `actual_food`.

### Spawn ou mutação inesperada

Se existir comida nova:

```text
actual_food - expected_food != empty
```

o grafo anterior inteiro fica obsoleto:

```text
FutureGraph.clear()
```

O mesmo ocorre para remoção/mutação que não seja explicada pela transição esperada.

Regra central:

> Food validation occurs before graph reconciliation. Any unexpected mutation in observed food invalidates the entire FutureGraph.

## 5. Estados provisórios de food

Durante simulação, quando qualquer cobra come uma fruta, os futuros posteriores ficam menos confiáveis porque a distribuição real de food pode mudar.

Não inventamos a posição de uma nova fruta.

A transição marca:

```rust
ForecastDelta::FoodUncertainty
```

e a rota passa a carregar contexto provisional.

Consequência:

- Survival mantém alta relevância;
- Hunting recebe desconto moderado;
- Food recebe desconto maior;
- se o próximo request revelar spawn, todo o grafo é descartado.

Se não houver spawn e o estado real corresponder exatamente a um nó previsto, podemos re-root e continuar usando os descendentes ainda compatíveis.

## 6. FutureGraph não é histórico

Telemetry guarda passado real.

FutureGraph guarda somente presente + futuros possíveis.

```text
Telemetry   -> observed past
FutureGraph -> possible future
```

Assim que um estado deixa de ser alcançável a partir da raiz real atual, pode ser removido.

## 7. SearchNode e SearchEdge

Como o grafo possui transpositions, eventos pertencem à transição e não ao node.

```rust
struct SearchNode {
    state: SimulatedGameState,
    analysis: Arc<StateAnalysis>,
    children: Vec<SearchEdge>,
}

struct SearchEdge {
    joint_action: JointAction,
    events: Vec<InstantEvent>,
    forecast_delta: ForecastDelta,
    child: NodeId,
}
```

O node representa somente o estado.

A edge responde:

> o que aconteceu para chegar neste estado?

Isso permite múltiplos pais apontarem para o mesmo node sem misturar causalidade.

## 8. StateAnalysis central

Cada estado real ou simulado recebe exatamente uma análise compartilhada.

```rust
struct StateAnalysis {
    state_key: StateKey,
    snake_food_routes: SnakeFoodRouteMatrix,
    food_claims: FoodClaimMatrix,
    static_space: StaticSpaceAnalysis,
}
```

Nenhum modo recalcula esses dados.

## 9. Uma BFS por cobra para todas as frutas

Para cada cobra viva:

```text
snake head
   |
   v
single BFS
   |
   +-- distance to every cell
   +-- first move masks
   +-- predecessors
```

Depois todas as frutas são consultadas no mesmo field.

Complexidade aproximada por estado:

```text
O(snakes * (V + E))
```

e não:

```text
O(snakes * foods * (V + E))
```

## 10. SnakeFoodRouteMatrix

```rust
struct FoodRouteInfo {
    reachable: bool,
    distance: Option<u16>,
    first_moves: MoveMask,
}

type SnakeFoodRouteMatrix =
    HashMap<SnakeId, HashMap<Coord, FoodRouteInfo>>;
```

Todos os menores first moves são preservados.

## 11. FoodClaimMatrix

Derivada da matriz central:

```rust
struct FoodClaimInfo {
    food: Coord,
    our_eta: Option<u16>,
    nearest_enemy_eta: Option<u16>,
    nearest_enemy: Option<SnakeId>,
    claim_margin: Option<i16>,
    contested: bool,
}
```

Food Mode e Enemy Tracing consomem essa estrutura.

## 12. Ações simultâneas

Cada profundidade representa uma resolução completa de movimento simultâneo.

```text
state t
   |
   +-- our action
   +-- action of enemy A
   +-- action of enemy B
   +-- ...
   |
   v
TurnResolver
   |
   v
state t+1
```

Enemy Tracing fornece os move sets de todos os adversários.

O Decision gera combinações plausíveis e resolve as regras.

Nenhuma cobra é tratada como se pudesse observar nossa escolha antes de mover.

## 13. TurnResolver

Existe uma única implementação das regras de resolução.

Ela determina:

- paredes;
- corpo;
- health;
- food;
- crescimento;
- hazards;
- head-to-head;
- eliminações;
- estado final da cobra.

Food/Hunting/Survival não implementam regras próprias de colisão.

## 14. Eventos instantâneos

Depois do TurnResolver, a transição produz eventos causais.

```rust
enum InstantEvent {
    AteFood {
        snake: SnakeId,
        food: Coord,
    },

    EnemyForced {
        enemy: SnakeId,
        remaining_moves: u8,
    },

    EnemyTrapped {
        enemy: SnakeId,
    },

    EnemyKilled {
        enemy: SnakeId,
        cause: EliminationCause,
    },

    SelfConstrained {
        remaining_moves: u8,
    },

    SelfDeadEnd,

    HeadToHeadWon {
        enemy: SnakeId,
    },

    HeadToHeadLost {
        enemy: SnakeId,
    },

    Died {
        cause: EliminationCause,
    },
}
```

Eventos não são scores.

## 15. Nenhum benefício agregado no node

Não existem:

```text
node.food_score
node.hunt_score
node.survival_score
```

A avaliação acontece somente durante traversal de uma rota completa.

## 16. Rota E2E é unidade atômica

Exemplo:

```text
Route A:
LEFT -> RIGHT -> eat food

Route B:
LEFT -> UP -> constrained
```

Nunca calculamos:

```text
LEFT = AteFood(Route A) - Constrained(Route B)
```

Primeiro:

```text
evaluate Route A
evaluate Route B
```

separadamente.

Somente depois agrupamos os resultados por primeira direção.

## 17. RouteEvaluation

```rust
struct RouteEvaluation {
    initial_move: Direction,

    benefit: f32,
    harm: f32,
    utility: f32,

    survival: SurvivalRouteAssessment,
    died: bool,

    events: Vec<InstantEvent>,
    certainty: RouteCertainty,
}
```

```text
utility = benefit - harm
```

Morte não é uma penalidade finita:

```text
Died => terminal hard failure
```

## 18. Benefícios

Benefícios estratégicos instantâneos incluem:

- AteFood;
- EnemyForced;
- EnemyTrapped;
- EnemyKilled;
- HeadToHeadWon.

A avaliação pode aplicar precedência para evitar double counting.

Exemplo:

```text
EnemyKilled supersedes prior
EnemyForced/EnemyTrapped
for the same enemy on this route.
```

## 19. Malefícios

Malefícios incluem:

- SelfConstrained(2);
- SelfConstrained(1);
- SelfDeadEnd;
- HeadToHeadLost;
- Died;
- outros riscos de Survival explicitamente derivados da mesma rota.

O mesmo caminho pode conter benefício e malefício.

Exemplo:

```text
LEFT -> DOWN -> eat food -> ends with 2 moves

+AteFood
-SelfConstrained(2)
```

Essa ponderação é válida porque os dois eventos pertencem à mesma rota.

## 20. Survival é prioridade máxima

Survival não é apenas outro weight.

A seleção usa uma ordem lexicográfica:

1. evitar morte;
2. reduzir dead-end risk;
3. reduzir forced/constrained futures;
4. preservar future mobility;
5. somente depois comparar valor Food/Hunting.

Food ou kill nunca compensam uma morte inevitável.

## 21. Survival por distribuição de rotas futuras

Para cada primeira direção, depois de avaliar as rotas E2E separadamente:

```rust
struct DirectionSurvivalSummary {
    total_routes: u32,
    death_routes: u32,
    dead_end_routes: u32,
    forced_routes: u32,
    constrained_routes: u32,
    min_future_mobility: u8,
}
```

Esse resumo não mistura eventos entre rotas; ele conta outcomes.

Quanto maior a fração de futuros que converge para morte/beco/forced state, pior a direção.

## 22. Agressividade

O estado da nossa cobra mantém:

```rust
struct AggressionState {
    fruits_eaten: u32,
    value: f32,
}
```

Agressividade cresce quando **nós** consumimos fruta.

Parâmetros são configuráveis:

```text
BASE_AGGRESSION
AGGRESSION_PER_FOOD
MAX_AGGRESSION
```

## 23. Agressividade dentro da rota simulada

Cada rota evolui sua própria agressividade.

```text
t0 aggression 0.30
t1 eat food
t2 aggression 0.40
```

Outra branch que não comeu continua em 0.30.

Isso faz parte do estado/contexto da rota e nunca é compartilhado entre siblings.

## 24. Food versus Hunting

Depois de passar pelo filtro de Survival:

```text
food_weight ~= 1 - aggression
hunt_weight ~= aggression
```

Health pode aumentar o valor alimentar.

Quando comida é necessária para não morrer, ela é elevada a necessidade de Survival.

## 25. Direção versus rotas futuras

Escolhemos apenas o primeiro movimento atual, mas cada direção possui várias rotas possíveis.

```rust
struct DirectionEvaluation {
    direction: Direction,
    routes: Vec<RouteEvaluation>,
    survival_summary: DirectionSurvivalSummary,
}
```

Ordem inicial:

1. melhor classe de Survival;
2. menor death/dead-end/forced ratio;
3. melhor pior rota estratégica sobrevivente;
4. comparar próximas rotas piores quando necessário;
5. aplicar critérios determinísticos de desempate.

Não somar eventos de routes diferentes.

## 26. Reserved Cell Policy

Bordas e quinas pertencem exclusivamente ao Decision.

Food, Hunting e Survival não conhecem essa política.

```rust
struct ReservedCellPolicy {
    edge_penalty: f32,
    corner_penalty: f32,
}
```

Preferência normal:

```text
interior > edge > corner
```

Cantos têm penalidade maior.

## 27. Exceções para células reservadas

Uma célula de borda/quina pode ser liberada quando:

### Eliminação tática

A rota confirma que entrar nela produz benefício ofensivo concreto, como:

- EnemyTrapped;
- EnemyKilled;
- HeadToHeadWon;

e nossa sobrevivência permanece aceitável.

### Última fuga

Todas as opções não reservadas são piores em Survival e a célula reservada preserva nossa vida/liberdade.

Comida sozinha não remove a reserva de uma quina quando existe opção interior segura.

## 28. FutureGraph rolling horizon

Profundidade inicial:

```text
TARGET_DEPTH = 3
```

Cada depth é uma interação simultânea completa.

No primeiro request:

```text
root
  -> t+1
      -> t+2
          -> t+3
```

Depois de escolher nossa direção, remover branches de outras direções nossas, mas manter todas as respostas adversárias plausíveis ao movimento escolhido.

## 29. Re-root no próximo request

Novo estado real chega.

Depois da validação de food:

1. procurar o estado real no subtree retido;
2. se encontrar, torná-lo nova raiz;
3. remover nodes não alcançáveis;
4. reaproveitar os dois níveis futuros restantes;
5. expandir apenas a nova fronteira até depth 3.

Exemplo:

```text
old:
t -> t+1 -> t+2 -> t+3

new request:
root = old t+1

cached:
new t+1 = old t+2
new t+2 = old t+3

calculate:
new t+3
```

## 30. Transposition table

Estados equivalentes são deduplicados.

```rust
struct SearchGraph {
    root: NodeId,
    nodes: NodeArena,
    transpositions: HashMap<StateKey, NodeId>,
}
```

Como eventos pertencem às edges, dois caminhos podem compartilhar um node sem perder causalidade.

## 31. StateKey

StateKey deve conter tudo que altera o futuro determinístico, incluindo:

- bodies;
- heads;
- health;
- length;
- alive/dead;
- food conhecido;
- hazards;
- turn/ruleset relevant state;
- aggression state da nossa cobra.

## 32. Garbage collection

O grafo não guarda passado.

Depois do re-root:

```text
retain only nodes reachable from current root
```

Branches incompatíveis e decisões passadas são removidos.

## 33. Iterative deepening

Depth 3 é obrigatório como alvo inicial quando o orçamento permitir.

Se houver folga:

```text
3 -> 4 -> 5 -> ...
```

Somente resultados de profundidades completamente concluídas podem substituir a decisão anterior.

Se depth 5 interrompe por deadline:

```text
use completed depth 4
```

## 34. Orçamento e latência

Usar:

- `game.timeout`;
- `you.latency`;
- tempo local de compute;
- histórico recente de jitter.

Nunca assumir timeout fixo.

```text
search_budget =
    game.timeout - safety_reserve
```

Safety reserve deve cobrir:

- transporte;
- serialização;
- scheduler/runtime jitter;
- oscilação espontânea de latência.

Inicialmente usar reserva conservadora e ajustar por telemetria.

## 35. Previsão de custo da próxima profundidade

Registrar por depth:

- nodes expanded;
- elapsed time;
- branching factor.

Antes de iniciar depth N+1:

```text
estimated_cost(next_depth)
<
remaining_safe_budget
```

Caso contrário, parar.

Cache/re-root reduz custo marginal, mas nunca elimina essa verificação.

## 36. Todos os jogadores participam

Enemy Tracing calcula movimentos de cada adversário vivo.

O JointAction representa todos os jogadores relevantes à resolução das regras.

Não remover uma cobra do TurnResolver por estar distante.

Deduplicação de estados, hard pruning e constraints são usados para controlar explosão combinatória sem violar a simultaneidade.

## 37. Determinismo

A V1 do Decision Search não usa amostragem aleatória.

Quando necessário controlar expansão:

- remover movimentos impossíveis;
- usar plausible sets;
- deduplicar transpositions;
- interromper aumento de depth;
- usar desempates estáveis.

Preferir reduzir profundidade a ignorar arbitrariamente ações plausíveis necessárias para corretude.

## 38. Telemetria

Registrar:

- depth concluído;
- nodes;
- edges;
- transposition hits;
- cache reuse;
- cache invalidation reason;
- food spawn invalidations;
- provisional routes;
- elapsed search time;
- safety reserve;
- chosen direction;
- aggression;
- survival summary;
- route utility da linha escolhida;
- distribuição de route outcomes por primeira direção.

## 39. Testes do StateAnalysis

- uma BFS por cobra;
- todas as comidas consultáveis;
- múltiplos first moves mínimos;
- todas as cobras presentes;
- alimento inalcançável;
- competição por alimento;
- StateAnalysis calculada uma vez por StateKey.

## 40. Testes do TurnResolver

- paredes;
- body collision;
- food;
- growth;
- health;
- hazard;
- head-to-head maior;
- head-to-head menor;
- head-to-head igual;
- múltiplas cobras simultâneas;
- eliminações.

## 41. Testes E2E de rotas

- benefit e harm na mesma rota;
- eventos de siblings nunca misturados;
- kill substitui reward de trap do mesmo alvo;
- morte não é compensada por comida;
- agressividade evolui só na branch que come;
- provisional confidence é branch-local.

## 42. Testes de cache

- escolha poda outros first moves;
- respostas adversárias permanecem;
- request seguinte encontra node;
- re-root preserva profundidade restante;
- apenas nova frontier é expandida;
- mismatch limpa cache;
- spawn observado limpa cache antes do lookup;
- GC remove passado;
- transposition compartilha node sem compartilhar edge events.

## 43. Testes de células reservadas

- interior vence edge equivalente;
- edge vence corner equivalente;
- fruta sozinha não libera corner;
- kill pode liberar reserved cell;
- head-to-head favorável pode liberar;
- último escape libera edge/corner;
- rota reservada fatal continua rejeitada.

## 44. Critérios de aceite

- [ ] existem exatamente um FutureGraph e uma StateAnalysis por estado;
- [ ] Food validation ocorre antes do cache lookup;
- [ ] spawn inesperado invalida o grafo inteiro;
- [ ] transições após consumo podem ser provisórias;
- [ ] eventos são armazenados em edges;
- [ ] nodes não carregam score agregado;
- [ ] cada rota E2E é avaliada isoladamente;
- [ ] Survival é prioridade máxima;
- [ ] Food/Hunting são ponderados por agressividade;
- [ ] todas as cobras participam da resolução simultânea;
- [ ] depth inicial é 3;
- [ ] profundidade pode crescer com iterative deepening;
- [ ] margem de latência é preservada;
- [ ] rolling cache reaproveita futuros compatíveis;
- [ ] bordas/quinas são política exclusiva do Decision;
- [ ] regras do Battlesnake existem em um único TurnResolver.
