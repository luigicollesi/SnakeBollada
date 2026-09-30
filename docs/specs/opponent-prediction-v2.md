# SPEC — Predição de Ações do Oponente V2

**Status:** Em implementação  
**Branch alvo:** `feat/opponent-prediction-v2`  
**Base:** `feat/initial-spec-implementation`  
**Escopo:** predição adaptativa por partida, em shadow mode  
**Objetivo:** estimar `P(move | opponent, state)`, aprender tendências observadas durante a partida e medir calibração sem alterar ainda as restrições conservadoras de sobrevivência.

## 1. Decisão de arquitetura

A V2 não tentará prever uma única direção como verdade. Para cada adversário vivo, produzirá uma distribuição:

```text
UP      0.18
RIGHT   0.61
DOWN    0.00
LEFT    0.21
```

Movimentos fisicamente impossíveis recebem probabilidade zero. A soma das probabilidades de movimentos possíveis deve ser 1.

A V2 funcionará em **shadow mode**:

- gera previsões;
- mantém histórico adaptativo;
- compara previsão com movimento realmente observado no turno seguinte;
- registra métricas;
- não reduz o `DangerMap` conservador da estratégia `basic-v1`.

Isso evita usar um modelo ainda não calibrado para assumir riscos letais.

## 2. Dependências existentes

A implementação parte da infraestrutura já criada:

- `GameRegistry` por `game.id`;
- `LiveGameState`;
- inferência de `ObservedMove`;
- `BoardMask`;
- snapshots por turno;
- telemetria assíncrona;
- persistência JSON;
- BFS/Flood Fill da estratégia básica.

A V2 não deve duplicar o estado global nem criar I/O no caminho crítico.

## 3. Fluxo por /move

```text
snapshot atual
    |
    v
derivar movimento adversário observado
    |
    v
resolver previsão pendente do turno anterior
    |
    +--> atualizar BehavioralProfile
    +--> atualizar métricas
    +--> armazenar observação recente
    |
    v
gerar features do estado atual
    |
    v
calcular scores por movimento
    |
    v
softmax
    |
    v
misturar com baseline uniforme conforme confiança
    |
    v
OpponentPredictionRecord
    |
    +--> memória da sessão
    +--> telemetria assíncrona
```

## 4. Modelo físico mínimo

Para cada adversário, avaliar `Up/Right/Down/Left`.

Um movimento é considerado impossível quando:

- sai do tabuleiro;
- volta diretamente para o pescoço;
- entra em corpo conhecido que permanecerá ocupado;
- entra em posição fatal por health/hazard quando isso puder ser determinado com segurança.

A própria cauda pode ser tratada como liberável quando a posição de destino não contém comida.

Caudas de outras snakes permanecem bloqueadas nesta versão.

## 5. Features por movimento

A V2 utiliza features baratas e explicáveis:

```rust
CandidateFeatures {
    direction,
    legal,
    food_progress,
    reachable_space,
    enemy_progress,
    enters_hazard,
    lethal_head_risk,
}
```

### food_progress

Diferença entre distância Manhattan para a comida mais próxima antes e depois do movimento.

Valor positivo indica aproximação.

### reachable_space

Flood Fill estático após o primeiro passo, considerando ocupação conhecida.

### enemy_progress

Diferença para a cabeça adversária mais próxima, excluindo a própria snake.

Valor positivo significa aproximação.

### enters_hazard

Indica se o destino está em hazard.

### lethal_head_risk

Indica se uma snake de mesmo tamanho ou maior pode disputar aquela casa no próximo turno.

## 6. BehavioralProfile

Cada adversário possui um perfil vivo durante a partida:

```rust
BehavioralProfile {
    food,
    space,
    aggression,
    hazard_tolerance,
    observations,
}
```

Cada tendência usa um prior Beta.

Inicialmente:

```text
Beta(2, 2)
mean = 0.5
```

Isso evita conclusões extremas com poucas observações.

### Atualização food

Só atualizar quando houver pelo menos um movimento legal que melhore distância de comida.

- ação real melhora comida -> sucesso;
- caso contrário -> falha.

### Atualização space

Só atualizar quando houver diferença relevante de espaço entre alternativas.

- ação escolhida está próxima do maior espaço -> sucesso;
- caso contrário -> falha.

### Atualização aggression

Só atualizar quando houver oportunidade real de se aproximar de outra cabeça.

- ação observada aproxima -> sucesso;
- caso contrário -> falha.

### Atualização hazard_tolerance

Só atualizar quando existir escolha entre hazard e não-hazard.

- entrou em hazard -> sucesso;
- evitou -> falha.

## 7. Score heurístico

Para cada ação legal:

```text
score =
    base_survival
  + food_weight * food_progress
  + space_weight * normalized_space
  + aggression_weight * enemy_progress
  - hazard_penalty
  - lethal_head_penalty
```

Os pesos são modulados pelo `BehavioralProfile`.

A pressão por comida também deve crescer quando health estiver baixo.

A V2 não tenta reproduzir exatamente a função objetivo do inimigo. Ela estima uma política plausível e adaptável.

## 8. Probabilidades

Scores são convertidos por softmax numericamente estável:

```text
P(a) = exp(score(a) - max_score) / sum(exp(score(i) - max_score))
```

A distribuição heurística é misturada com uma distribuição uniforme sobre movimentos legais:

```text
P_final =
    confidence * P_heuristic
  + (1 - confidence) * P_uniform
```

### Confiança

A confiança deve crescer com observações válidas e separação entre probabilidades.

Com poucas observações, o modelo permanece conservador.

Se só existe um movimento legal:

```text
P(move) = 1
confidence = 1
```

## 9. Histórico bounded

Não guardar histórico infinito no caminho quente.

```rust
OpponentObservation {
    turn,
    actual_move,
    predicted_probability,
    top1_correct,
    brier_score,
    log_loss,
}
```

A memória em runtime mantém no máximo as últimas 64 observações por adversário.

O histórico completo continua recuperável pela telemetria por turno.

## 10. Métricas

Por adversário:

- previsões resolvidas;
- top-1 accuracy;
- mean Brier score;
- mean log loss.

Não atualizar métricas para:

- `EliminatedUnknown`;
- `NotAvailable`;
- ausência de previsão pendente.

## 11. Telemetria

Adicionar evento:

```rust
TelemetryEvent::OpponentPredictions(OpponentPredictionRecord)
```

Cada turno registra:

```rust
OpponentPredictionRecord {
    turn,
    opponents: HashMap<SnakeId, OpponentPredictionSnapshot>,
}
```

Cada snapshot contém:

- distribuição;
- confidence;
- perfil comportamental;
- métricas acumuladas.

A previsão do turno N deve ser comparável ao `observed_move` registrado no snapshot N+1.

## 12. Estado concorrente

Cada `GameHandle` passa a possuir:

```rust
predictor: Arc<RwLock<OpponentPredictor>>
```

O predictor existe exclusivamente no escopo de uma partida.

Não compartilhar perfis entre partidas nesta versão.

Motivo: primeiro validar adaptação online sem introduzir persistência de identidade ou contaminação entre rulesets/mapas.

## 13. Robustez

- `/move` sem sessão conhecida continua recuperável;
- adversário novo no meio da partida recebe prior padrão;
- adversário eliminado pode manter histórico, mas não recebe nova previsão;
- IDs são usados como chave, nunca nomes;
- scores inválidos ou degenerados devem cair para distribuição uniforme;
- nenhuma previsão pode impedir resposta ao Battlesnake engine.

## 14. Performance

A predição deve ser O(oponentes * movimentos * células) no pior caso por causa dos flood fills.

Para boards comuns isso permanece pequeno, mas será medido.

Regras:

- sem I/O síncrono;
- sem locks durante escrita em disco;
- sem alocação histórica ilimitada;
- nenhuma busca multi-turno nesta versão.

O timeout do Battlesnake é informado por request e frequentemente é 500 ms; a V2 deve consumir apenas uma fração pequena desse orçamento.

## 15. Testes obrigatórios

### Distribuição

- movimentos impossíveis = 0;
- soma das probabilidades ~= 1;
- único movimento legal = 1;
- softmax não gera NaN.

### Perfil

- prior inicia em 0.5;
- buscar comida aumenta food bias;
- evitar comida quando havia oportunidade reduz food bias;
- escolher maior região aumenta space bias;
- aproximação ofensiva aumenta aggression;
- hazard só atualiza quando havia alternativa.

### Métricas

- top-1 correto;
- top-1 incorreto;
- Brier score finito;
- log loss protegido contra log(0);
- `EliminatedUnknown` não treina.

### Runtime

- perfis são isolados por game.id;
- previsão pendente do turno N é resolvida no N+1;
- adversário novo recebe prior;
- histórico respeita limite.

### Telemetria

- prediction record entra no mesmo turno;
- serialização JSON preserva distribuição e perfil.

## 16. Critérios de aceite

- [ ] cada adversário vivo recebe uma distribuição válida;
- [ ] ações impossíveis recebem zero;
- [ ] modelo aprende a partir de movimentos observados;
- [ ] perfis usam priors e não saturam após poucas amostras;
- [ ] métricas de calibração são calculadas online;
- [ ] histórico de runtime é bounded;
- [ ] previsões são persistidas em telemetria;
- [ ] nenhuma inferência é criada para movimento fatal desconhecido;
- [ ] estratégia `basic-v1` permanece conservadora;
- [ ] testes existentes continuam passando;
- [ ] novos testes cobrem predictor e aprendizagem;
- [ ] clippy e rustfmt passam.

## 17. Evolução V3

Após coletar partidas suficientes:

1. calibrar pesos com telemetria real;
2. criar `ProbabilisticHeadRiskMap`;
3. medir thresholds seguros por Brier/log loss;
4. integrar risco probabilístico à estratégia;
5. selecionar adversários relevantes;
6. implementar simulação simultânea;
7. probability pruning;
8. beam search com iterative deepening.

Somente depois da calibração a previsão poderá reduzir restrições atualmente letais do `DangerMap`.

## 18. Referências

- Battlesnake Rules: https://docs.battlesnake.com/rules
- Battlesnake Competitive Play: https://docs.battlesnake.com/guides/competitive-play
- Battlesnake Useful Algorithms: https://docs.battlesnake.com/guides/useful-algorithms
- Battlesnake Ruleset Settings: https://docs.battlesnake.com/api/objects/ruleset-settings
