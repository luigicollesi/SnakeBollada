# SPEC — Enemy Tracing

**Status:** Implementado V1  
**Branch alvo:** `dev`  
**Depende de:** `decision-making.md`  
**Substitui:** `opponent-mobility-v2.md`

## Estado da implementação

Implementado em `dev`:

- `legal_moves` com hard constraints determinísticos;
- `plausible_moves` conservador com space filter e food intent;
- fallback para cobras já condenadas sem contaminar o ThreatMap;
- uso da mesma `MobilityAnalysis` e `StateAnalysis` do node;
- participação de todos os adversários vivos no produto cartesiano de ações;
- telemetria de coverage comparando o movimento real observado no turno seguinte contra os conjuntos legal/plausible;
- ordem determinística e nenhuma recursão interna: profundidade continua pertencendo exclusivamente ao FutureGraph.

## 1. Objetivo

Enemy Tracing calcula, para cada cobra adversária, quais movimentos são possíveis e quais continuam plausíveis **somente para o próximo turno**.

Ele não controla profundidade, não mantém histórico, não mantém FutureGraph e não escolhe nossa ação.

```text
current simulated state
        |
        v
Enemy Tracing
        |
        +-- enemy A -> {UP, LEFT}
        +-- enemy B -> {RIGHT}
        +-- enemy C -> {UP, RIGHT, DOWN}
```

O Decision/Search Engine chama Enemy Tracing em cada estado expandido.

## 2. Contrato

```rust
fn trace(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
) -> EnemyTracingOutput;
```

```rust
struct EnemyTracingOutput {
    enemies: HashMap<SnakeId, EnemyMoveSet>,
}

struct EnemyMoveSet {
    legal_moves: MoveMask,
    plausible_moves: MoveMask,
    eliminations: Vec<MoveElimination>,
}
```

`MoveMask` é um bitmask de quatro direções.

## 3. Responsabilidade estritamente one-step

Enemy Tracing responde:

> Dado este estado, quais movimentos este adversário pode tomar na próxima resolução simultânea?

Ele NÃO responde:

- onde o adversário estará em t+2;
- onde estará em t+3;
- qual caminho completo seguirá;
- qual ação é mais provável numericamente.

Recursão pertence ao Decision/Search Engine.

## 4. StateAnalysis compartilhada

Enemy Tracing nunca executa sua própria busca Snake → Food.

Ele recebe a matriz central calculada uma única vez por estado:

```rust
StateAnalysis {
    snake_food_routes,
    food_claims,
    ...
}
```

A matriz cobre:

```text
todas as cobras x todas as comidas
```

e preserva:

- distância;
- alcançabilidade;
- todos os first moves mínimos;
- competição pelas comidas.

## 5. Legal moves: hard constraints

Primeiro construir `legal_moves`.

Remover somente movimentos fisicamente impossíveis ou deterministicamente fatais no estado conhecido:

- fora do board;
- volta direta para pescoço;
- corpo que certamente permanecerá ocupado;
- hazard que garanta morte considerando health/ruleset;
- outras colisões determinísticas conhecidas.

Quando houver dúvida, o movimento permanece.

## 6. Caudas

### Própria cauda do adversário

Pode ser liberável quando a simulação consegue determinar que a cauda se moverá e a cobra não crescerá naquela transição.

### Cauda de outra cobra

Quando sua liberação depende de uma ação simultânea desconhecida, não tratá-la como certeza.

O tracing deve preferir manter possibilidades a produzir false exclusion.

## 7. Plausible moves

`plausible_moves` começa igual a `legal_moves`.

Filtros estratégicos podem reduzir o conjunto apenas de forma conservadora.

Regra:

```text
plausible_moves nunca pode ficar vazio
se legal_moves não estiver vazio,
a menos que todos os movimentos sejam
deterministicamente fatais.
```

## 8. Space filter

Para cada movimento legal, calcular o espaço estático disponível depois do primeiro passo.

Se existem opções claramente viáveis:

```text
reachable_space >= snake.length
```

movimentos que entram em regiões insuficientes podem ser removidos do conjunto plausível.

Se todas as opções possuem espaço ruim, manter o conjunto legal para não inventar uma preferência.

## 9. Food intent

Enemy Tracing usa a matriz central Snake × Food.

Para cada adversário sabemos:

```text
food F1 -> distance 2 -> first moves {RIGHT, UP}
food F2 -> distance 6 -> first move {LEFT}
```

Quando comida está próxima ou health está pressionado, movimentos pertencentes a menores rotas alimentares ganham plausibilidade.

Filtro inicial pode usar:

```text
nearest_food_distance <= FOOD_NEAR_DISTANCE
ou
health <= FOOD_PRESSURE_HEALTH
```

com thresholds configuráveis.

O tracing mantém uma margem de distância para evitar over-pruning.

## 10. Competição por comida

Como `StateAnalysis` contém rotas de todas as cobras, Enemy Tracing pode reconhecer que uma fruta aparentemente próxima será provavelmente consumida por outro jogador primeiro.

Isso reduz a força do food-intent filter para essa fruta.

Nenhuma nova BFS é executada.

## 11. Hazard preference

Hazard fatal é hard exclusion.

Hazard não fatal é preferência, não impossibilidade.

Ele só pode ser removido de `plausible_moves` quando existe alternativa razoavelmente segura e o filtro não cria confiança excessiva.

## 12. Head-to-head e simultaneidade

Enemy Tracing não conhece ainda qual ação nossa será escolhida.

Portanto, um movimento adversário para uma célula que também podemos alcançar não é removido só por existir risco de head-to-head.

A resolução real pertence ao TurnResolver depois que o Decision constrói uma ação conjunta.

```text
our move + enemy moves
        |
        v
TurnResolver
        |
        +-- win
        +-- loss
        +-- tie
```

## 13. Uso pelo Hunting Mode

Hunting recebe `EnemyTracingOutput` para medir:

```text
mobility = |plausible_moves|
```

Ao simular novos estados, o Decision chama novamente Enemy Tracing.

Assim Hunting pode comparar:

```text
before: 3 plausible moves
after:  1 plausible move
```

sem Enemy Tracing precisar conhecer Hunting.

## 14. Uso pelo Food Mode

Food Mode não depende diretamente da previsão comportamental dos inimigos para pathfinding; ele usa a mesma matriz central.

Enemy Tracing complementa essa informação quando o Decision precisa avaliar se um inimigo pode consumir uma comida ou entrar numa região disputada no próximo turno.

## 15. Métricas

Quando o próximo estado real chega, observabilidade pode comparar o movimento real com o conjunto anterior.

```text
coverage =
    actual_move in plausible_moves

pruning_ratio =
    1 - plausible_count / legal_count

false_exclusion =
    actual_move not in plausible_moves
```

Objetivo:

- coverage muito alto;
- pruning útil;
- false exclusion próximo de zero.

## 16. Sem probabilidades

Esta versão não usa:

- softmax;
- Beta priors;
- Brier score;
- log loss;
- probabilidades por direção;
- ML.

A saída é um conjunto.

## 17. Comida e previsões provisórias

Enemy Tracing trabalha sobre o estado que recebeu.

Se o estado faz parte de uma rota marcada como provisional por incerteza de food, o Decision reduz a confiança estratégica do resultado.

Enemy Tracing não gerencia essa confiança.

Quando um spawn real é observado, o Decision invalida o FutureGraph antes de reutilizar tracing antigo.

## 18. Testes obrigatórios

- parede;
- pescoço;
- corpo;
- própria cauda liberável;
- cauda incerta de outra cobra;
- hazard fatal;
- hazard não fatal;
- alimento próximo;
- health baixo;
- fruta disputada por outra cobra;
- todos os menores first moves são preservados;
- filtro de espaço;
- filtro nunca elimina arbitrariamente todas as opções;
- célula de head-to-head permanece possível antes do TurnResolver;
- coverage do movimento observado;
- false exclusion.

## 19. Critérios de aceite

- [ ] tracing é estritamente next-turn;
- [ ] todas as cobras adversárias recebem legal/plausible set;
- [ ] usa StateAnalysis central;
- [ ] não executa BFS Snake → Food própria;
- [ ] não contém recursion;
- [ ] não contém cache;
- [ ] não decide bordas/quinas;
- [ ] não aplica agressividade;
- [ ] head-to-head é resolvido fora do tracing;
- [ ] métricas de coverage/pruning podem ser registradas.
