# SPEC — Survival Mode

**Status:** Implementado V1 — avaliação E2E multi-turn integrada  
**Branch alvo:** `dev`  
**Depende de:** `decision-making.md`

## Estado da implementação

Implementado em `dev`:

- distinção entre movimentos determinísticos e robustamente seguros;
- `ThreatMap` usando movimentos plausíveis dos adversários;
- candidatos imediatos com reachable space;
- `SurvivalStateSnapshot`;
- `SurvivalRouteAssessment` preparado para rotas E2E;
- eventos `SelfConstrained` e `SelfDeadEnd` derivados por transição;
- morte continua sendo evento terminal do `TurnResolver`.

Integração multi-turn concluída:

- snapshots são compostos automaticamente em rotas root → leaf;
- second-order mobility usa os sucessores reais do FutureGraph e mantém a pior mobilidade plausível ao longo da rota;
- risco por direção agrega death/dead-end/forced/constrained sem misturar siblings;
- Survival é comparado lexicograficamente antes de borda/quina e antes de Food/Hunting;
- se existe um movimento robustamente seguro no turno atual, opções imediatamente ameaçadas por head-to-head não entram na comparação estratégica;
- avaliação de uma depth só é aceita se expansão e traversal E2E terminarem dentro do deadline.

## 1. Objetivo

Survival Mode identifica linhas que preservam nossa liberdade futura.

Ele é a fonte de análise de sobrevivência do Decision Engine e possui prioridade máxima sobre Food e Hunting.

Seu foco não é maximizar recompensa, mas reduzir a chance de a trajetória convergir para:

- morte;
- beco sem saída;
- movimento forçado;
- região insuficiente;
- sequência com poucas opções futuras.

## 2. Survival não é um score do node

Um SearchNode não possui `survival_score` acumulado.

Cada transição pode gerar eventos instantâneos:

```rust
InstantEvent::SelfConstrained {
    remaining_moves: u8,
}

InstantEvent::SelfDeadEnd,

InstantEvent::Died {
    cause: EliminationCause,
}
```

A avaliação de Survival é feita sobre uma rota E2E específica.

## 3. Unidade de avaliação

```rust
struct SurvivalRouteAssessment {
    died: bool,
    dead_end: bool,
    final_safe_moves: u8,
    min_safe_moves: u8,
    final_reachable_space: u32,
    min_reachable_space: u32,
    second_order_mobility: u32,
}
```

Ela descreve somente uma sequência concreta root → leaf.

Nunca mistura propriedades de branches irmãos.

## 4. Mobilidade

Classificação conceitual:

```text
4 saídas -> open
3 saídas -> safe
2 saídas -> constrained
1 saída  -> forced
0 saídas -> dead end
death    -> terminal failure
```

O número de saídas é calculado depois da resolução completa do turno.

## 5. Future mobility

Além de `safe_moves`, medir:

```text
second_order_mobility =
    total de saídas seguras dos sucessores imediatos
```

Duas células com mesmo Flood Fill podem ter geometrias diferentes.

```text
A: reachable 30, next moves 1
B: reachable 30, next moves 3

Survival prefere B.
```

## 6. Reachable space

Flood Fill continua como métrica de espaço.

Ela não é suficiente isoladamente.

Survival considera simultaneamente:

- reachable space;
- safe moves;
- second-order mobility;
- gargalos;
- progressão ao longo da rota.

## 7. Convergência futura

O Decision Search expande inicialmente três interações completas.

Survival permite detectar aprisionamento antes de ele ocorrer.

Exemplo:

```text
agora: 4 saídas
t+1:   3
t+2:   2
t+3:   1
```

Essa rota deve receber malefício de Survival apesar de o estado atual parecer aberto.

## 8. Rotas por direção

Para uma direção atual, existirão várias rotas E2E.

Exemplo:

```text
LEFT Route A -> safe
LEFT Route B -> constrained
LEFT Route C -> dead end
```

Survival avalia A, B e C separadamente.

O Decision Engine pode depois analisar a distribuição dessas avaliações para decidir se LEFT é robusto.

Nunca se faz:

```text
benefit from A - harm from C
```

como se fossem uma única linha.

## 9. Morte

Morte é hard failure.

```text
Died => terminal survival rejection
```

Food ou Hunting não podem compensar morte com recompensa.

## 10. Head-to-head

O TurnResolver resolve head-to-head antes de Survival analisar o estado.

Se perdemos ou empatamos em condição fatal:

```text
InstantEvent::Died
```

A rota é terminalmente ruim.

Se vencemos, Survival observa apenas nossa condição resultante; o benefício ofensivo pertence a Hunting/Decision.

## 11. Ativação de Survivor Mode

O Decision Engine pode elevar Survivor Mode quando:

- legal moves <= 2;
- safe next moves <= 2;
- reachable space está baixo;
- árvore futura mostra aumento de forced/dead-end routes;
- Food/Hunting convergem para posições perigosas.

O modo também pode atuar preventivamente, não apenas quando já estamos presos.

## 12. Bordas e quinas

Survival Mode não contém regra de células reservadas.

Ele pode indicar que uma rota pela borda é a melhor rota espacial.

O Decision Engine decide se a política global de bordas/quinas pode ser relaxada.

Isso mantém a separação:

```text
Survival -> "esta rota é a melhor para sobreviver"
Decision -> "esta célula reservada pode ser usada?"
```

## 13. Eventos e malefícios

Eventos instantâneos possíveis:

- SelfConstrained(2);
- SelfConstrained(1);
- SelfDeadEnd;
- Died.

A severidade final é aplicada pelo Decision Engine.

Uma rota que fica com duas saídas depois de comer uma fruta carrega, na mesma rota:

```text
+AteFood
-SelfConstrained(2)
```

Isso permite ponderação Food × Survival causal.

## 14. Avaliação agregada por direção

Survival não mistura eventos, mas o Decision precisa escolher um primeiro movimento.

Assim, depois de avaliar cada rota individualmente, o Decision pode comparar por direção:

- pior resultado de Survival;
- número de rotas de morte;
- número de dead ends;
- número de forced paths;
- min future mobility;
- distribuição das rotas sobreviventes.

A regra exata de agregação pertence a `decision-making.md`.

## 15. Saída

```rust
struct SurvivalCandidate {
    first_move: Direction,
    immediate_safe_moves: u8,
    immediate_reachable_space: u32,
}

struct SurvivalModeOutput {
    candidates: Vec<SurvivalCandidate>,
}
```

O modo também fornece funções de avaliação de rota utilizadas pelo Decision.

## 16. Testes obrigatórios

- 4, 3, 2, 1 e 0 saídas;
- mesmo Flood Fill com mobilidade diferente;
- second-order mobility;
- corredor que parece grande mas termina em forced line;
- três profundidades mostrando convergência;
- rota com comida + constrained na mesma linha;
- rota sibling com comida não contamina outra;
- head-to-head fatal;
- rota por borda espacialmente melhor sem aplicar política de borda;
- morte é irrecuperável por recompensa.

## 17. Critérios de aceite

- [ ] Survival trabalha por rota E2E;
- [ ] morte é hard failure;
- [ ] future mobility é calculada;
- [ ] second-order mobility existe;
- [ ] Flood Fill não é métrica única;
- [ ] eventos de diferentes branches nunca são combinados;
- [ ] bordas/quinas não pertencem ao modo;
- [ ] Decision consegue identificar crescimento futuro do risco antes do aprisionamento.
