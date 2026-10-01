# SPEC — Hunting Mode

**Status:** Implementado V2 — Hunting territorial + enclosure integrado ao FutureGraph  
**Branch alvo:** `dev`  
**Depende de:** `enemy-tracing.md`, `decision-making.md`

## Estado da implementação

Implementado em `dev`:

- candidatos de head-to-head favorável baseados em `ThreatMap`;
- snapshots táticos por inimigo;
- comparação before/after de mobilidade plausível;
- eventos `EnemyForced` e `EnemyTrapped` derivados por transição;
- kills e head-to-head continuam exclusivos do `TurnResolver`;
- eventos de Hunting permanecem específicos da edge/rota.

Integração multi-turn concluída:

- mobilidade inimiga é reavaliada em cada node do FutureGraph;
- `EnemyForced`, `EnemyTrapped`, kills e H2H são avaliados na rota E2E;
- kills carregam atribuição causal (`OurSnake`, outra cobra, ambiente ou autoinduzida);
- forced/trapped só recebem crédito ofensivo quando a geometria da nossa cobra contribui de forma verificável;
- estados `FoodProvisional` recebem desconto próprio;
- leaf hunting potential reconhece pressão no horizonte sem inventar eventos;
- kill causal supersede benefícios menores do mesmo alvo para evitar double counting.

Implementado na V2 territorial:

- `TerritoryAnalysis` por node do FutureGraph, com multi-source BFS/Voronoi de espaço;
- espaço alcançável, exclusivo e contestado por cobra;
- articulation points de largura 1 e cálculo do ganho de corte;
- análise de choke limitada a pontos próximos para proteger o orçamento de busca;
- `escape_frontier`, distância da borda e suporte geométrico de boundary;
- `EnclosureAnalysis` com estados `Safe / Pressure / Constrained / Critical`;
- risco de enclosure próprio faz parte do comparador adversarial de Survival;
- Food Opening abandona uma fruta comprometida se a linha entrar em `Constrained/Critical`;
- Hunting territorial passa a produzir planos progressivos:
  - `HeadPressure`;
  - `EdgePin`;
  - `ChokeCut`;
  - `TerritorySqueeze`;
  - `PartialWrap`;
  - `FullEnclosure`;
  - `StarvationSiege`;
- leaf hunting potential usa o melhor plano territorial além da pressão tática tradicional;
- aumento de oportunidade territorial entre parent/child gera recompensa incremental limitada;
- guaranteed kill continua separado e pode superar Food Opening apenas quando não cria enclosure próprio.

Gates iniciais implementados para Standard:

```text
length >= 6  -> EdgePin
length >= 7  -> ChokeCut / TerritorySqueeze
length >= 9  -> PartialWrap
length >= 12 -> FullEnclosure com suporte de borda/choke
length >= 14 -> FullEnclosure em área aberta
length >= 10 -> StarvationSiege
```

Pendente após V2: separadores explícitos de largura 2, calibração empírica dos gates por benchmark e modelo probabilístico adicional de comportamento.

## 1. Objetivo

Hunting Mode encontra linhas capazes de reduzir a mobilidade de um adversário até forçá-lo, cercá-lo ou eliminá-lo.

Ele não significa simplesmente andar na direção da cabeça inimiga.

Seu objetivo é produzir candidatos táticos em que a geometria futura do tabuleiro reduz as opções plausíveis do alvo.

## 2. Entrada

```rust
fn candidates(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    tracing: &EnemyTracingOutput,
) -> HuntingModeOutput;
```

Ele reutiliza:

- StateAnalysis central;
- rotas de todas as cobras para todas as frutas;
- movimentos legais/plausíveis calculados por Enemy Tracing;
- espaço alcançável;
- distância real até inimigos;
- comprimento das cobras.

## 3. Seleção de alvo

Um alvo é relevante quando existe possibilidade real de interação.

O valor bruto de alvo pode considerar:

- distância real;
- número atual de movimentos plausíveis;
- vantagem de comprimento;
- possibilidade de head-to-head favorável;
- possibilidade de fechamento;
- acesso a choke points;
- competição por comida.

O modo pode analisar mais de um alvo, mas o Decision Engine controla quantos adversários são expandidos profundamente conforme orçamento.

## 4. Redução de mobilidade

Para um alvo:

```text
before = |plausible_moves|
after  = |plausible_moves after our simulated move|

mobility_reduction = before - after
```

Hunting Mode procura candidatos que reduzam `after`.

Isso não gera benefício por si só no node.

O benefício instantâneo só é criado quando a transição produz uma condição concreta.

## 5. Eventos instantâneos de Hunting

```rust
InstantEvent::EnemyForced {
    enemy: SnakeId,
    remaining_moves: 1,
}

InstantEvent::EnemyTrapped {
    enemy: SnakeId,
}

InstantEvent::EnemyKilled {
    enemy: SnakeId,
    cause: EliminationCause,
}

InstantEvent::HeadToHeadWon {
    enemy: SnakeId,
}
```

Esses eventos pertencem apenas à rota que realmente os produziu.

## 6. Forçar versus cercar versus matar

Hierarquia conceitual:

```text
EnemyForced < EnemyTrapped < EnemyKilled
```

O Decision Engine aplica os pesos finais.

Se um mesmo alvo é primeiro forçado e depois morto na mesma rota, a avaliação deve evitar double counting excessivo.

Regra recomendada:

```text
EnemyKilled supersedes EnemyForced/EnemyTrapped
for the same enemy in the same route
```

Os eventos ainda podem ser mantidos para telemetria.

## 7. Head-to-head

Toda interação de cabeça deve ser resolvida pelo TurnResolver comum.

Regras:

- nossa cobra maior: oponente menor morre;
- nossa cobra menor: morremos;
- comprimentos iguais: ambas morrem.

Hunting Mode não pode transformar um head-to-head desfavorável em vantagem apenas porque reduz movimentos do inimigo.

O resultado resolvido gera:

```text
HeadToHeadWon
HeadToHeadLost
Died
EnemyKilled
```

conforme aplicável.

## 8. Contested cells

Uma célula pode servir para bloquear o oponente quando nossa chegada é competitiva.

```rust
struct ContestedCell {
    coord: Coord,
    our_distance: u16,
    enemy_distance: u16,
    length_advantage: i32,
}
```

Uma célula é taticamente interessante quando podemos chegar antes ou simultaneamente e a resolução simultânea é favorável.

## 9. Choke points

Hunting Mode pode usar articulation points/choke points identificados pela análise compartilhada.

Um choke é útil quando:

- reduz o componente acessível do alvo;
- chegamos antes ou em condição favorável;
- não exige morte nossa;
- realmente diminui as opções futuras do alvo.

Hunting Mode propõe a linha. O benefício somente aparece se o estado simulado confirmar `EnemyForced`, `EnemyTrapped` ou `EnemyKilled`.

## 10. Food e intenção inimiga

Hunting Mode pode consultar a matriz central Snake × Food para antecipar regiões atraentes ao alvo.

Ele não executa pathfinding próprio.

Exemplo:

```text
enemy -> F1 first_moves {LEFT, UP}
```

Isso pode indicar uma região onde bloquear LEFT ou UP é taticamente útil.

A inferência de quais movimentos permanecem plausíveis pertence ao Enemy Tracing.

## 11. Agressividade

Hunting Mode produz valor bruto de ataque.

Ele não decide quanto esse valor pesa.

O Decision Engine aplica:

```text
hunt_weight = f(aggression)
```

A agressividade aumenta conforme nossa cobra come frutas.

Survival sempre é avaliado antes.

## 12. Estados provisórios

Se uma rota atravessa uma transição onde comida foi consumida, descendentes podem ser `FoodProvisional`.

Isso reduz a confiança de Hunting porque novas frutas podem mudar a intenção de um oponente.

O desconto é aplicado no Decision Engine, por rota.

## 13. Bordas e quinas

Hunting Mode não conhece células reservadas.

Uma linha ofensiva pode atravessar borda ou quina.

Quem decide se essa exceção é aceitável é o Decision Engine.

Uma célula reservada pode ser liberada quando:

- a linha confirma uma eliminação/cerco muito forte;
- ou for necessária para sobrevivência.

## 14. Saída

```rust
struct HuntingCandidate {
    target: SnakeId,
    first_move: Direction,
    mobility_before: u8,
    mobility_after_if_resolved: u8,
    contested_cell: Option<Coord>,
    choke_target: Option<Coord>,
}

struct HuntingModeOutput {
    candidates: Vec<HuntingCandidate>,
}
```

A saída é candidata, não pontuação final.

## 15. Testes obrigatórios

- alvo com três saídas;
- nosso movimento reduz três para duas;
- linha força uma saída;
- linha cerca oponente;
- linha elimina oponente;
- head-to-head favorável;
- head-to-head desfavorável;
- empate de comprimento elimina ambos;
- choke que parece bom mas também nos prende;
- outro jogador chega antes ao choke;
- inimigo atraído por comida conhecida;
- rota provisional perde confiança externamente;
- eventos de siblings não são misturados.

## 16. Critérios de aceite

- [ ] Hunting reutiliza Enemy Tracing;
- [ ] não há pathfinding Snake → Food interno;
- [ ] ataque significa redução de futuro, não simples aproximação;
- [ ] kills dependem do TurnResolver;
- [ ] eventos são instantâneos e específicos da rota;
- [ ] agressividade não é aplicada dentro do modo;
- [ ] bordas/quinas não são decididas pelo modo.

## Atualização V3 — Dominance Frontier

Competitive Territory distingue células ganhas por vantagem de comprimento:

- `dominance_claim_cells`;
- `dominance_frontier_cells`;
- `favorable_head_frontier`.

Quando nossa ETA empata com a de um inimigo menor, a célula pode representar território que ele não consegue contestar sem perder o head-to-head. Essa vantagem entra no score territorial, no HuntIntent progress e no selective search.

`HeadPressure` agora é committable quando o score alcança o threshold e usa lock curto de dois turnos. Outros HuntIntents mantêm janela maior. HeadPressure só deve ser criado quando existe pressão de fronteira real, não apenas porque somos maiores.

Territory hunting não depende mais de `aggression >= 0.20`. Ele pode ser ativado por `StrategicPosture.hunt_drive_milli` ou por sermos a maior cobra.

O progresso de Hunting combina:

```text
target territory loss
our territory gain
frontier gain
dominance frontier gain
target escape loss
enclosure progress
plan progress
- self enclosure
- border growth
```

Se o inimigo recua e cede território, o HuntIntent pode continuar sem perseguir diretamente sua cabeça.
