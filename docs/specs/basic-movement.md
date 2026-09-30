# SPEC — Movimentação Básica

**Status:** Em implementação — V1 funcional  
**Branch alvo:** `dev`  
**Escopo:** primeira estratégia determinística da SnakeBollada  
**Objetivo:** alcançar a comida segura mais próxima usando o menor caminho real, evitando colisões óbvias e rejeitando caminhos que levem a espaço insuficiente.

## 1. Objetivos

A estratégia V1 deve:

1. gerar os quatro movimentos cardinais;
2. eliminar movimentos fora do tabuleiro;
3. eliminar colisões com corpos;
4. evitar casas que podem causar head-to-head letal;
5. encontrar comida por menor distância real de caminho;
6. usar BFS em grid não ponderado;
7. recalcular a rota a cada turno;
8. usar Flood Fill para rejeitar movimentos que levem a regiões perigosamente pequenas;
9. possuir fallback de sobrevivência quando nenhuma comida segura for encontrada;
10. ser determinística sempre que possível.

## 2. Não objetivos

Nesta versão não serão implementados:

- Minimax;
- MaxN;
- MCTS;
- previsão profunda de movimentos adversários;
- Voronoi territorial;
- A* ponderado;
- estratégia ofensiva avançada;
- aprendizado de máquina;
- cache de rota entre turnos.

## 3. Princípio de decisão

A estratégia não deve perseguir simplesmente a comida com menor distância Manhattan.

Ela deve buscar:

> a comida alcançável com menor número de movimentos seguros no estado atual.

Em seguida, deve validar se o primeiro movimento deixa espaço suficiente para sobrevivência.

## 4. Replanejamento a cada turno

Nunca armazenar uma rota e segui-la cegamente.

```text
turn N:
snapshot -> search -> move

turn N+1:
novo snapshot -> nova search -> novo move
```

Motivo:

- os adversários se movem simultaneamente;
- corpos mudam;
- comidas desaparecem;
- novas comidas podem surgir;
- hazards podem alterar o risco.

## 5. Pipeline da estratégia

```text
GameState
   |
   v
NavigationMap
   |
   +--> occupied
   +--> food
   +--> hazards
   +--> lethal_head_danger
   |
   v
LegalMoveFilter
   |
   v
BFS to food
   |
   v
candidate first steps ordered by path length
   |
   v
simulate first step
   |
   v
Flood Fill safety check
   |
   +--> safe -> choose
   |
   +--> unsafe -> try next candidate
   |
   v
Survival fallback
```

## 6. NavigationMap

Estrutura proposta:

```rust
struct NavigationMap {
    width: u16,
    height: u16,
    occupied: BoardMask,
    our_body: BoardMask,
    opponent_bodies: BoardMask,
    food: BoardMask,
    hazards: BoardMask,
    lethal_head_danger: BoardMask,
}
```

Funções mínimas:

```rust
fn in_bounds(&self, coord: Coord) -> bool;
fn is_occupied(&self, coord: Coord) -> bool;
fn is_food(&self, coord: Coord) -> bool;
fn is_lethal_head_danger(&self, coord: Coord) -> bool;
fn is_safe_static(&self, coord: Coord) -> bool;
```

## 7. Movimentos cardinais

```rust
enum Direction {
    Up,
    Down,
    Left,
    Right,
}
```

```rust
impl Direction {
    fn apply(self, coord: Coord) -> Coord;
}
```

Convenção Battlesnake:

```text
Up    -> y + 1
Down  -> y - 1
Right -> x + 1
Left  -> x - 1
```

## 8. Casas bloqueadas

Na V1, uma casa é bloqueada quando:

- está fora do mapa;
- contém parte do nosso corpo que permanecerá ocupada;
- contém corpo adversário;
- está marcada como head-to-head letal.

### Nossa cauda

A cauda atual pode ser tratada como potencialmente liberável quando não houver crescimento naquele passo.

A implementação deve ser conservadora:

- permitir nossa cauda somente quando a simulação local garantir que ela será liberada;
- caso contrário, mantê-la bloqueada.

### Cauda adversária

Na V1, permanecerá bloqueada.

Motivo: não sabemos com certeza se o adversário comerá e crescerá.

## 9. Danger map de cabeças

Para cada adversário vivo:

1. listar seus movimentos cardinais válidos estáticos;
2. marcar as células adjacentes que sua cabeça pode alcançar;
3. comparar comprimento.

Regra V1:

```rust
if enemy.length >= ours.length {
    mark_as_lethal_head_danger(candidate);
}
```

Se somos maiores que o inimigo:

- a célula não é automaticamente letal;
- nesta versão ela continua permitida;
- estratégia ofensiva ficará para uma spec futura.

## 10. BFS até comida

Como todos os movimentos têm custo 1, BFS é o algoritmo principal.

Propriedade desejada:

- visitar células em ordem de distância;
- descobrir as comidas alcançáveis do menor caminho para o maior.

Estrutura conceitual:

```rust
struct BfsWorkspace {
    queue: VecDeque<CellIndex>,
    visited: BoardMask,
    parent: Vec<Option<CellIndex>>,
    distance: Vec<u16>,
}
```

Algoritmo:

```text
enqueue(head)

while queue not empty:
    current = pop_front()

    if current has food:
        record food candidate

    for neighbor in cardinal_neighbors(current):
        if out of bounds:
            continue
        if blocked:
            continue
        if visited:
            continue

        visited[neighbor] = true
        parent[neighbor] = current
        distance[neighbor] = distance[current] + 1
        enqueue(neighbor)
```

## 11. Não parar obrigatoriamente na primeira fruta

A primeira fruta encontrada pelo BFS é a mais próxima em passos, mas pode levar a uma região ruim.

Então o sistema deve poder avaliar frutas em ordem crescente de distância:

```text
Food A -> 3 passos -> primeiro passo leva a região ruim
Food B -> 5 passos -> primeiro passo leva a região segura

=> escolher Food B
```

Não é necessário guardar todas as rotas completas.

Basta guardar, para cada candidato:

- distância;
- primeiro movimento;
- coordenada da comida.

## 12. Reconstrução do primeiro passo

A partir da comida encontrada:

1. seguir `parent` de volta;
2. parar no nó cujo pai é a cabeça;
3. converter delta em `Direction`.

```rust
struct FoodPathCandidate {
    food: Coord,
    distance: u16,
    first_move: Direction,
}
```

## 13. Flood Fill de segurança

Antes de aceitar um `first_move`:

1. simular nossa cabeça na nova posição;
2. atualizar ocupação local mínima;
3. executar flood fill a partir da nova cabeça;
4. medir número de células alcançáveis.

Métrica:

```rust
reachable_cells: u32
```

Regra inicial:

```text
se reachable_cells < our.length:
    rejeitar candidato
```

Esse limite é deliberadamente simples e poderá ser refinado.

## 14. Score de segurança para desempate

Entre candidatos equivalentes:

```text
1. menor distância até comida
2. maior reachable_cells
3. menor exposição a hazards
4. ordem determinística de direção
```

Sugestão de ordem estável:

```text
Up -> Right -> Down -> Left
```

A ordem não representa preferência estratégica; serve apenas para resultados reproduzíveis.

## 15. Hazards

Na primeira implementação, hazards não devem ser tratados automaticamente como parede.

Eles devem ser representados separadamente.

Política inicial:

- preferir caminho sem hazard;
- permitir hazard apenas quando necessário para evitar morte imediata ou quando não houver alternativa segura;
- cálculo avançado de custo/health ficará para versão posterior com busca ponderada.

## 16. Fallback de sobrevivência

Se nenhuma comida segura for encontrada:

1. gerar movimentos legais;
2. para cada movimento:
   - simular primeiro passo;
   - calcular flood fill;
3. escolher o movimento com maior espaço alcançável;
4. evitar head-to-head letal;
5. usar desempate determinístico.

```text
no safe food
    |
    v
legal moves
    |
    v
flood fill each
    |
    v
max reachable space
```

## 17. API interna proposta

```rust
pub fn choose_move(state: &GameState) -> Decision;
```

```rust
struct Decision {
    direction: Direction,
    reason: DecisionReason,
    target_food: Option<Coord>,
    path_distance: Option<u16>,
    reachable_cells: u32,
}
```

```rust
enum DecisionReason {
    NearestSafeFood,
    SurvivalFallback,
    OnlyLegalMove,
}
```

Esses metadados devem ser enviados ao sistema de observabilidade.

## 18. Organização proposta

```text
src/
├── navigation/
│   ├── bfs.rs
│   ├── flood_fill.rs
│   ├── map.rs
│   └── mod.rs
└── strategy/
    ├── basic.rs
    ├── danger.rs
    └── mod.rs
```

## 19. Complexidade esperada

Para um tabuleiro com `V` células:

- BFS: `O(V + E)`;
- grid cardinal: `E <= 4V`;
- Flood Fill: `O(V + E)`.

Para tabuleiros pequenos de Battlesnake, esse custo deve ser muito inferior ao orçamento total de request.

A implementação inicial deve privilegiar:

- clareza;
- ausência de alocações desnecessárias;
- reutilização de `BoardMask`;
- facilidade de benchmark.

## 20. Interação com observabilidade

Cada decisão deve fornecer dados para telemetria:

```rust
DecisionRecord {
    chosen_move,
    decision_time_us,
    strategy_version,
    reason,
    target_food,
    path_distance,
    reachable_cells,
}
```

Assim será possível posteriormente medir:

- frequência de fallback;
- distância média até comida;
- mortes após perseguição de comida;
- espaço médio escolhido;
- custo computacional por turno.

## 21. Casos de teste obrigatórios

### Limites

- cabeça no canto;
- cabeça na borda;
- única saída legal.

### Corpos

- próprio corpo bloqueia;
- corpo adversário bloqueia;
- nossa cauda liberável;
- cauda adversária conservadoramente bloqueada.

### Head-to-head

- inimigo maior;
- inimigo do mesmo tamanho;
- inimigo menor.

### BFS

- uma comida em linha reta;
- comida Manhattan próxima mas bloqueada;
- duas comidas com distâncias diferentes;
- comida inalcançável;
- múltiplas comidas na mesma distância.

### Flood Fill

- corredor sem saída;
- região menor que nosso comprimento;
- região ampla;
- comida mais próxima rejeitada por espaço.

### Fallback

- nenhuma comida;
- todas as comidas inacessíveis;
- somente um movimento seguro;
- múltiplos movimentos e escolha do maior espaço.

## 22. Critérios de aceite

A movimentação básica estará concluída quando:

- [x] nunca escolher deliberadamente movimento fora do mapa;
- [x] evitar corpos conhecidos;
- [x] evitar head-to-head contra cobra de mesmo tamanho ou maior;
- [x] BFS encontrar menor caminho seguro até comida;
- [x] rota for recalculada a cada turno;
- [x] comida insegura puder ser rejeitada;
- [x] Flood Fill medir espaço após o primeiro passo;
- [x] fallback escolher melhor espaço quando não houver comida segura;
- [x] decisão produzir metadados para observabilidade;
- [ ] testes cobrirem os casos definidos;
- [ ] benchmark confirmar custo muito abaixo do timeout da partida.

## 23. Implementação V1

Implementação inicial:

- `src/navigation.rs`: mapa estático, danger map, BFS e flood fill;
- `src/strategy.rs`: política determinística `basic-v1`;
- primeira busca evita hazards completamente;
- segunda busca permite hazards apenas quando nenhuma rota sem hazard foi aceita;
- fallback escolhe maior espaço alcançável, com desempate determinístico;
- ordem estável: `Up -> Right -> Down -> Left`;
- decisões registram motivo, alvo, distância e espaço alcançável.

Ainda pendente para fechar integralmente a spec:

- cobrir todos os cenários de teste listados na seção 21;
- adicionar benchmark formal e limite de desempenho automatizado;
- evoluir tratamento de hazards para considerar health/dano.

## 23. Evolução futura

Depois desta spec:

1. custos ponderados;
2. Dijkstra/A*;
3. health-aware hazard routing;
4. Voronoi territorial;
5. previsão de caudas adversárias;
6. simulação simultânea de turnos;
7. iterative deepening;
8. Minimax/MaxN/MCTS.

## 24. Referências

- Battlesnake Rules: https://docs.battlesnake.com/rules
- Useful Algorithms: https://docs.battlesnake.com/guides/useful-algorithms
- Battlesnake API: https://docs.battlesnake.com/api
- Board object: https://docs.battlesnake.com/api/objects/board
