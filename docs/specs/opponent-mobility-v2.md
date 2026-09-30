# SPEC — Opponent Mobility Constraint V2

**Status:** Planejado  
**Branch alvo:** `spec/opponent-mobility-v2`  
**Base:** `dev`  
**Escopo:** reduzir conjuntos plausíveis de movimento e usar essa redução para planejar contenção  
**Objetivo:** substituir a previsão probabilística por um modelo determinístico/set-based que identifique quais movimentos adversários continuam plausíveis e escolha nossos movimentos para reduzir a mobilidade futura deles sem sacrificar nossa própria sobrevivência.

## 1. Mudança de conceito

A V2 não tenta mais estimar:

```text
P(move | state)
```

Ela mantém dois conjuntos por adversário:

```text
legal_moves     = fisicamente possíveis
plausible_moves = legal_moves após filtros estratégicos conservadores
```

Exemplo:

```text
legal_moves     = {UP, RIGHT, LEFT}
plausible_moves = {RIGHT, LEFT}
```

A nossa estratégia então avalia movimentos que diminuam `plausible_moves` nos próximos turnos.

## 2. Métrica principal da predição

A prioridade não é "top-1 accuracy".

A predição é considerada boa quando:

1. o movimento real continua dentro do conjunto previsto;
2. o conjunto previsto é menor que o conjunto legal bruto.

Métricas:

```text
coverage =
    actual_move in plausible_moves

pruning_ratio =
    1 - plausible_count / legal_count

false_exclusion =
    actual_move not in plausible_moves

average_set_size =
    média de plausible_count
```

Objetivo operacional:

- maximizar coverage;
- reduzir average_set_size;
- reduzir false_exclusion antes de aumentar agressividade.

## 3. Hard filters

Movimentos impossíveis são removidos sem heurística.

Excluir:

- fora do tabuleiro;
- entrada no próprio pescoço/corpo que permanecerá ocupado;
- entrada em corpo conhecido adversário que permanecerá ocupado;
- hazard que garanta morte por health;
- outras colisões determinísticas conhecidas.

Não excluir somente porque uma colisão é possível.

Quando houver incerteza, manter o movimento no conjunto.

## 4. Tail model

### Própria cauda do adversário

Pode ser considerada liberável somente quando pudermos determinar que:

- a célula destino é a própria cauda;
- não haverá crescimento naquele movimento.

### Caudas de outras snakes

Na V2 permanecem conservadoramente bloqueadas.

## 5. Strategic narrowing

Depois dos hard filters, aplicar filtros conservadores que tentem modelar decisões razoáveis.

Esses filtros nunca devem eliminar todas as opções se existirem movimentos legais.

### 5.1 Space survival filter

Para cada movimento legal do adversário:

1. simular o primeiro passo;
2. executar Flood Fill;
3. medir reachable space.

Se existirem alternativas com espaço suficiente:

```text
reachable_space >= enemy.length
```

podemos excluir candidatos claramente menores que o necessário.

Se todos forem ruins, manter o conjunto legal inteiro.

### 5.2 Food intent filter

Criar um distance field de comida por BFS multi-source.

Para cada movimento:

```text
food_distance_after
```

Modo food intent é ativado quando pelo menos uma condição ocorrer:

```text
nearest_food_distance <= FOOD_NEAR_DISTANCE
ou
enemy.health <= FOOD_PRESSURE_HEALTH
```

Valores iniciais a validar:

```text
FOOD_NEAR_DISTANCE = 3
FOOD_PRESSURE_HEALTH = 45
```

Quando ativo, manter movimentos dentro de uma margem:

```text
distance_after <= best_distance_after + FOOD_DISTANCE_MARGIN
```

Inicialmente:

```text
FOOD_DISTANCE_MARGIN = 1
```

A margem evita over-pruning.

### 5.3 Hazard preference

Hazard fatal continua sendo hard exclusion.

Hazard não fatal não deve ser removido automaticamente.

Somente excluir hazard como preferência quando:

- existe alternativa não-hazard;
- health está abaixo de um threshold seguro;
- o filtro não eliminaria todas as alternativas.

## 6. OpponentConstraintSet

Tipo proposto:

```rust
struct OpponentConstraintSet {
    snake_id: String,
    legal_moves: SmallMoveSet,
    plausible_moves: SmallMoveSet,
    eliminated_by: Vec<ConstraintReason>,
}
```

```rust
enum ConstraintReason {
    OutOfBounds,
    BodyCollision,
    NeckCollision,
    FatalHazard,
    InsufficientSpace,
    FoodIntent,
    HazardPreference,
}
```

`SmallMoveSet` pode ser um bitmask de 4 bits.

## 7. Distance fields

Calcular uma vez por turno:

### FoodDistanceField

BFS multi-source partindo de todas as comidas.

Fornece:

```text
distance_to_nearest_food[cell]
```

### EnemyDistanceField

Para cada inimigo relevante, BFS a partir da cabeça.

Fornece distância real em grid considerando obstáculos conhecidos.

Esses campos evitam executar A* repetidamente para cada candidato.

## 8. Distância de interação

A estratégia muda de prioridade conforme proximidade real.

Definir:

```text
interaction_distance =
    shortest_path_distance(our_head, enemy_head)
```

Modo contenção começa quando:

```text
interaction_distance <= ENEMY_INTERACTION_RADIUS
```

Valor inicial:

```text
ENEMY_INTERACTION_RADIUS = 4
```

Esse valor é uma hipótese de tuning, não uma regra fixa.

## 9. Movimentação própria: dois objetivos principais

A movimentação passa a combinar:

1. sobrevivência/comida;
2. redução de mobilidade adversária.

### 9.1 Food priority

Quando:

- nossa comida está próxima; ou
- nosso health está pressionado;

manter prioridade de comida.

```text
food_pressure = f(distance_to_food, health)
```

Durante food priority:

- BFS continua escolhendo caminho seguro;
- block score vira desempate;
- nunca sacrificar comida crítica somente para reduzir mobilidade adversária.

### 9.2 Containment priority

Quando inimigo está dentro do interaction radius:

- medir seus movimentos plausíveis;
- avaliar como cada nosso movimento altera esses movimentos;
- favorecer posições que reduzem saídas.

## 10. Mobility reduction score

Para cada nosso movimento legal `m`:

1. simular nosso primeiro passo;
2. recalcular os conjuntos plausíveis dos inimigos relevantes;
3. medir redução de mobilidade.

```text
mobility_before(enemy) = |plausible_moves_before|
mobility_after(enemy,m) = |plausible_moves_after|
```

```text
mobility_reduction =
    mobility_before - mobility_after
```

Métricas adicionais:

```text
forced_enemy =
    plausible_moves_after == 1

trapped_enemy =
    plausible_moves_after == 0
```

`trapped_enemy` só conta como vantagem se a simulação confirmar que nosso movimento não é igualmente fatal.

## 11. Simultaneidade e head-to-head

As ações são simultâneas.

Portanto, nosso movimento pode remover uma opção adversária apenas quando essa opção se torna perdedora sob o movimento que estamos avaliando.

### Se somos maiores

Se:

```text
our.length > enemy.length
```

podemos usar nossa cabeça para contestar células e reduzir opções adversárias.

### Se somos iguais ou menores

Não tratar uma célula compartilhada como bloqueio favorável.

Ela deve ser considerada risco para nós.

## 12. Contested cells

Criar:

```text
ContestedCell {
    coord,
    our_distance,
    enemy_distance,
    length_advantage,
}
```

Uma célula é especialmente interessante quando:

```text
our_distance <= enemy_distance
```

e sua ocupação reduz acesso do inimigo a espaço relevante.

Isso aproxima a estratégia de um modelo Voronoi/frontier sem implementar ainda uma busca adversarial profunda.

## 13. Articulation points / choke points

Modelar células livres como grafo.

Um articulation point é uma célula cuja remoção aumenta o número de componentes conectados.

Detectar em:

```text
O(V + E)
```

por DFS.

Uso estratégico:

- detectar gargalos próximos do inimigo;
- verificar se conseguimos chegar ao gargalo antes ou junto dele;
- verificar se ocupar esse ponto reduz o componente acessível do adversário;
- garantir que não estamos fechando a nós mesmos em região pior.

Criar:

```rust
struct ChokePoint {
    coord: Coord,
    enemy_space_before: u32,
    enemy_space_after_block: u32,
    our_distance: u16,
    enemy_distance: u16,
}
```

## 14. Choke score

```text
choke_gain =
    enemy_space_before - enemy_space_after_block
```

Priorizar choke apenas quando:

```text
our_distance <= enemy_distance
```

e:

```text
our_space_after >= our.length
```

## 15. Strategy evaluator

Cada nosso movimento recebe componentes:

```text
score =
    survival_score
  + food_weight * food_gain
  + space_weight * our_reachable_space
  + block_weight * mobility_reduction
  + forced_weight * forced_enemies
  + choke_weight * choke_gain
  + territory_weight * contested_control
  - hazard_penalty
  - lethal_head_penalty
```

Os pesos variam conforme contexto.

## 16. Dynamic mode blending

Evitar modos rígidos sempre que possível.

Calcular:

```text
food_pressure in [0,1]
interaction_pressure in [0,1]
```

Exemplo:

```text
food_weight =
    base_food + food_pressure * food_bonus

block_weight =
    base_block + interaction_pressure * block_bonus
```

Assim:

- comida perto aumenta gradualmente prioridade;
- inimigo perto aumenta gradualmente contenção.

## 17. Ordem de segurança

Nenhuma heurística ofensiva pode ultrapassar os filtros:

1. morte imediata;
2. head-to-head desfavorável;
3. região insuficiente;
4. comida crítica por health;
5. só depois containment/blocking.

## 18. Relevant opponents

Não analisar profundamente todos os inimigos.

Um adversário é `relevant` quando:

```text
interaction_distance <= RELEVANCE_RADIUS
```

Valor inicial:

```text
RELEVANCE_RADIUS = 6
```

Inimigos distantes continuam no danger map básico, mas não recebem análise de contenção.

## 19. Planejamento multi-turno V2

Não implementar Minimax completo ainda.

Para cada nosso candidato:

```text
our move
   |
   v
enemy plausible sets
   |
   v
mobility + space + choke evaluation
```

Opcionalmente adicionar lookahead de profundidade 2 depois dos benchmarks.

A V2 inicial deve permanecer barata e explicável.

## 20. Métricas de telemetria

Por inimigo/turno:

```rust
OpponentConstraintRecord {
    turn,
    snake_id,
    legal_moves,
    plausible_moves,
    pruning_ratio,
    actual_move_next_turn,
    coverage,
}
```

Por nossa decisão:

```rust
ContainmentDecisionRecord {
    turn,
    chosen_move,
    relevant_enemy,
    mobility_before,
    mobility_after,
    forced_enemy,
    choke_target,
    choke_gain,
}
```

## 21. Critérios de qualidade da predição

Antes de usar filtros mais agressivos:

```text
coverage >= 0.98
```

como alvo inicial de experimento.

Não é garantia matemática; é um threshold de tuning.

Se coverage cair:

- diminuir filtros estratégicos;
- aumentar FOOD_DISTANCE_MARGIN;
- reduzir assumptions sobre comportamento.

## 22. Testes obrigatórios

### Hard constraints

- parede;
- corpo;
- pescoço;
- tail release;
- hazard fatal;
- hazard não fatal.

### Food intent

- fruta a 1 passo;
- fruta a 2–3 passos;
- duas frutas;
- caminho bloqueado;
- health alto sem food pressure;
- health baixo com food pressure;
- margem preserva múltiplas opções.

### Mobility

- inimigo com 3 saídas;
- nosso movimento reduz para 2;
- nosso movimento força 1;
- falso bloqueio por head-to-head desfavorável não recebe bônus.

### Flood Fill

- saída leva a corredor;
- saída leva a região grande;
- filtro não elimina todas as opções se todas são ruins.

### Articulation points

- corredor entre duas salas;
- falso choke em área aberta;
- nós chegamos antes;
- inimigo chega antes;
- bloquear choke também nos prende.

### Telemetria

- actual move estava no set;
- false exclusion;
- pruning ratio;
- coverage acumulada.

## 23. Fases de implementação

### Fase 1 — Constraint engine

- bitmask de movimentos;
- hard filters;
- food distance field;
- space filter;
- métricas coverage/pruning.

### Fase 2 — Mobility-aware movement

- simular nossos 4 movimentos;
- recalcular plausible set dos inimigos relevantes;
- mobility reduction;
- forced move score.

### Fase 3 — Choke control

- articulation points;
- choke gain;
- contested arrival time.

### Fase 4 — Tuning

- partidas locais em lote;
- coverage;
- pruning ratio;
- win rate;
- mortes por agressividade;
- calibrar thresholds/pesos.

## 24. Não objetivos da V2

- distribuição probabilística;
- softmax;
- Beta priors;
- Brier score;
- log loss;
- ML;
- Minimax profundo;
- MCTS;
- persistência de perfil entre partidas.

## 25. Evolução futura

Depois de validar a V2:

1. depth-2 set-based lookahead;
2. Voronoi completo;
3. chamber/tree-of-chambers;
4. multi-opponent joint constraints;
5. Minimax/MaxN apenas se necessário;
6. probabilidades somente se dados mostrarem ganho real.

## 26. Referências

- Battlesnake Useful Algorithms: https://docs.battlesnake.com/guides/useful-algorithms
- Battlesnake Competitive Play: https://docs.battlesnake.com/guides/competitive-play
- CP-Algorithms — Articulation Points: https://cp-algorithms.com/graph/cutpoints.html
- Red Blob Games — Flow Field Pathfinding: https://www.redblobgames.com/blog/2024-04-27-flow-field-pathfinding/
- Codingame Tron Battle — Flood Fill, Voronoi, Minimax: https://www.codingame.com/multiplayer/bot-programming/tron-battle
