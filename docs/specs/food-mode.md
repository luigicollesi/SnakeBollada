# SPEC — Food Mode

**Status:** Em implementação — root-state slice funcional  
**Branch alvo:** `dev`  
**Depende de:** `decision-making.md`, `enemy-tracing.md`  
**Substitui conceitualmente:** a estratégia `basic-v1` como modo alimentar

## 1. Objetivo

Food Mode encontra linhas seguras para alimentação usando exclusivamente a análise central já calculada para o estado.

Ele não possui uma árvore própria, não executa BFS própria e não decide sozinho o movimento final.

Sua responsabilidade é:

1. analisar todas as comidas conhecidas;
2. considerar as rotas de todas as cobras até essas comidas;
3. selecionar até dois objetivos alimentares úteis;
4. preferir objetivos que ofereçam direções iniciais distintas;
5. produzir candidatos para o Decision Engine;
6. registrar benefício somente quando uma comida é efetivamente consumida na rota simulada.

## 2. Regra arquitetural

Nenhum modo pode recalcular caminhos Snake → Food.

Para cada estado real ou simulado, o Decision/Search Engine cria uma única `StateAnalysis`.

```rust
struct StateAnalysis {
    state_key: StateKey,
    snake_food_routes: SnakeFoodRouteMatrix,
    food_claims: FoodClaimMatrix,
    certainty: ForecastCertainty,
}
```

Food Mode recebe essa análise:

```rust
fn candidates(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
) -> FoodModeOutput;
```

## 3. Matriz central Snake × Food

Para cada cobra viva, uma BFS partindo da cabeça fornece informação para todas as comidas.

```rust
struct FoodRouteInfo {
    food: Coord,
    reachable: bool,
    distance: Option<u16>,
    first_moves: MoveMask,
}
```

A matriz preserva todas as direções iniciais que pertencem a algum menor caminho.

Exemplo:

```text
Food F1
distance = 5
first_moves = UP | RIGHT
```

Food Mode não pode reduzir isso arbitrariamente para apenas UP.

## 4. Competição pela comida

Para cada comida:

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

```text
claim_margin = nearest_enemy_eta - our_eta
```

Interpretação:

- positivo: chegamos antes;
- zero: chegada disputada;
- negativo: algum oponente pode chegar antes.

Essa informação serve para evitar perseguir uma fruta que provavelmente deixará de existir antes da nossa chegada.

## 5. Até dois objetivos

Food Mode mantém no máximo dois objetivos alimentares por estado.

A seleção considera:

- alcançabilidade;
- distância;
- competição;
- direções iniciais disponíveis;
- saúde atual;
- certeza da previsão.

A intenção principal de manter dois objetivos é preservar optionalidade quando outro jogador puder consumir o primeiro alvo.

## 6. Diversidade de primeira direção

Quando possível, os dois candidatos devem começar por direções distintas.

Exemplo:

```text
F1 -> RIGHT
F2 -> RIGHT
F3 -> UP
```

Se F1 e F3 forem suficientemente competitivas, preferir:

```text
RIGHT -> F1
UP    -> F3
```

em vez de retornar duas opções que começam por RIGHT.

Se todas as rotas alimentares boas compartilham o mesmo primeiro movimento, Food Mode retorna apenas essa direção.

## 7. Food Mode não atribui recompensa antecipada

Não existe benefício porque uma rota "aponta para" uma fruta.

O benefício só aparece quando a simulação realmente resolve:

```text
snake head enters food cell
+
turn resolution confirms consumption
```

Então o Search Graph registra:

```rust
InstantEvent::AteFood {
    snake: SnakeId,
    food: Coord,
}
```

Nenhum `SearchNode` possui `food_score` agregado.

## 8. Rotas E2E independentes

Considere:

```text
LEFT -> RIGHT -> eat food
LEFT -> UP    -> constrained
```

São duas rotas diferentes.

Food Mode nunca produz:

```text
LEFT = food benefit - constrained harm
```

O primeiro evento pertence somente à primeira rota; o segundo pertence somente à segunda.

A combinação benefício - malefício é responsabilidade do Decision Engine sobre cada rota E2E.

## 9. Health pressure

Food Mode pode aumentar a relevância dos candidatos quando saúde estiver baixa.

Isso não altera o fato de que Survival tem prioridade máxima.

Se saúde atingir nível em que comer é requisito de sobrevivência, o Decision Engine poderá tratar alimentação como necessidade de Survival, e não como simples preferência estratégica.

Food Mode apenas fornece os melhores caminhos alimentares disponíveis.

## 10. Agressividade

Food Mode não conhece nem aplica pesos de agressividade.

Ele produz informação alimentar bruta.

O Decision Engine aplica:

```text
food_weight = f(aggression, health)
```

A agressividade cresce conforme a nossa cobra consome frutas.

Ela influencia Food versus Hunting apenas depois da análise de Survival.

## 11. Comida futura e estados provisórios

Durante uma simulação, se qualquer cobra consumir comida, os descendentes podem depender de um conjunto futuro de comidas ainda desconhecido.

Eles passam a usar:

```text
ForecastCertainty::FoodProvisional
```

Food Mode pode continuar usando a comida conhecida nesses estados, mas suas conclusões recebem menor confiança no Decision Engine.

Food Mode não inventa a posição de futuras frutas.

## 12. Invalidação por spawn observado

No começo de cada request real, o Decision Engine valida o conjunto de food antes de reutilizar o FutureGraph.

Se houver spawn ou mutação inesperada:

```text
FutureGraph.clear()
```

Food Mode recebe então uma nova `StateAnalysis` construída a partir do estado real.

## 13. Saída do modo

```rust
struct FoodCandidate {
    target_food: Coord,
    first_move: Direction,
    distance: u16,
    claim_margin: Option<i16>,
    contested: bool,
    certainty: ForecastCertainty,
}

struct FoodModeOutput {
    candidates: Vec<FoodCandidate>, // max 2
}
```

## 14. O que Food Mode NÃO decide

Food Mode não decide:

- bordas;
- quinas;
- células reservadas;
- head-to-head final;
- prioridade sobre Hunting;
- prioridade sobre Survival;
- depth da busca;
- timeout;
- reutilização de grafo;
- valor final de uma rota.

Tudo isso pertence ao Decision Engine.

## 15. Testes obrigatórios

- uma comida alcançável;
- comida Manhattan próxima mas bloqueada;
- múltiplos menores caminhos preservam múltiplos first moves;
- duas frutas retornam duas direções;
- três frutas onde duas compartilham first move favorecem diversidade útil;
- inimigo chega antes na fruta mais próxima;
- nossa cobra chega antes em fruta mais distante;
- nenhuma comida alcançável;
- health baixo mantém candidato alimentar;
- consumo só gera benefício depois de efetivamente acontecer;
- branch que não consome comida não herda benefício de sibling;
- estado provisional reduz confiança externa, sem modificar a matriz central.

## 16. Critérios de aceite

- [x] nenhuma BFS Snake → Food é executada dentro do modo;
- [x] todas as comidas usam a matriz central;
- [x] competição de todas as cobras é considerada;
- [x] até dois objetivos são retornados;
- [x] direções iniciais distintas são preservadas quando úteis;
- [ ] benefício alimentar é instantâneo e causal;
- [x] Food Mode não contém política de borda/quina;
- [x] Food Mode não mistura eventos de rotas diferentes.


## 17. Estado da implementação V1

Implementado no `dev`:

- `src/direction.rs`: `Direction` compartilhado e `MoveMask` de 4 bits;
- `src/analysis/routes.rs`: uma BFS por cobra viva para todas as comidas conhecidas;
- preservação de todos os first moves pertencentes a caminhos mínimos;
- `StateAnalysis` central com `FoodRouteInfo` e `FoodClaimInfo`;
- comparação de ETA de todas as cobras para cada comida;
- `src/modes/food.rs`: seleção determinística de até dois candidatos;
- preferência por direções iniciais distintas;
- fruta mais distante e claimable pode superar fruta próxima perdida para adversário;
- `strategy::choose_move` usa Food Mode e mantém validação imediata de perigo, Flood Fill e hazards;
- estratégia registrada na telemetria como `food-mode-v1`;
- o BFS alimentar anterior foi removido de `navigation.rs`.

Ainda depende do futuro Decision/TurnResolver:

- emissão real de `InstantEvent::AteFood`;
- avaliação benefício − malefício de rotas E2E;
- invalidação do FutureGraph por spawn;
- confiança provisional por branch;
- ponderação por agressividade;
- pressão de health integrada à busca futura.


## 17. Estado da implementação root-state

Implementado em `dev`:

- `Direction` e `MoveMask` compartilhados;
- `StateAnalysis` central;
- uma BFS por cobra viva para todas as comidas conhecidas;
- preservação de todos os primeiros movimentos pertencentes a menores caminhos;
- `SnakeFoodRouteMatrix` implícita no `StateAnalysis`;
- `FoodClaimInfo` derivado sem novas BFS;
- comparação de ETA entre nossa cobra e todos os adversários;
- `FoodModeOutput` com no máximo dois candidatos e direções iniciais distintas;
- estratégia atual consumindo `FoodModeOutput`;
- competição por comida podendo superar distância puramente geométrica;
- Danger Map, Flood Fill e preferência imediata por não entrar em hazard permanecendo fora do Food Mode;
- `STRATEGY_VERSION = "food-mode-v1"`.

Ainda depende de `decision-making.md` / TurnResolver:

- emissão de `InstantEvent::AteFood`;
- benefício alimentar causal em rotas E2E;
- invalidação do FutureGraph por spawn;
- ponderação de branches provisórios;
- agressividade Food × Hunting;
- busca depth 3+;
- política de bordas/quinas.
