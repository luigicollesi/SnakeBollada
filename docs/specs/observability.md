# SPEC — Sessão Efêmera e Observabilidade

**Status:** Observabilidade persistida desativada  
**Branch alvo:** `dev`  
**Escopo:** Battlesnake API v1

## Decisão atual

A SnakeBollada não persiste histórico de partidas e não envia telemetria para disco, banco de dados ou serviço externo.

A aplicação assume uma única partida ativa por processo.

O único estado mantido entre requests é o estado efêmero necessário para melhorar a decisão durante a partida:

- `DecisionState`;
- `FutureGraph` reaproveitável;
- agressividade acumulada;
- último comprimento observado;
- food observado para validar o rolling graph;
- pequena janela de tempos usada pelo SearchBudget.

Esse estado existe somente em memória.

## Ciclo de vida

```text
POST /start
    ↓
cria ActiveGame
    ↓
DecisionState::default()
    ↓
FutureGraph será criado no primeiro /move

POST /move
    ↓
usa DecisionState da sessão ativa
    ↓
reconcilia/reutiliza FutureGraph
    ↓
DecisionEngine
    ↓
mantém somente futuro relevante em memória
    ↓
retorna movimento

POST /end
    ↓
remove ActiveGame
    ↓
drop DecisionState
    ↓
drop FutureGraph
    ↓
nenhum histórico é persistido
```

## Runtime

A aplicação mantém apenas:

```rust
struct GameRuntime {
    active: Mutex<Option<ActiveGame>>,
}

struct ActiveGame {
    game_id: String,
    decision_state: DecisionState,
}
```

Não existem mais no runtime:

- `DashMap<game_id, GameHandle>`;
- múltiplas sessões simultâneas;
- `LiveGameState` separado;
- `mpsc` de telemetria;
- recorder assíncrono;
- `GameRecordStorage`;
- arquivos `data/games/*.json`;
- conexão com Neon/Postgres;
- persistência de snapshots ou decisões.

## Concorrência

O runtime suporta uma única partida ativa.

- `/start` para o mesmo `game.id`: tratado como duplicado;
- `/start` para outro jogo: substitui a sessão anterior;
- `/move` para jogo diferente da sessão ativa: usa fallback stateless e preserva a sessão ativa;
- `/end` para o jogo ativo: remove toda a sessão e o tree cache;
- `/end` para outro jogo: não altera a sessão ativa.

## Objetivo de desempenho

Nenhuma operação de I/O relacionada a histórico deve existir no caminho crítico de `/move`.

O caminho crítico é:

```text
request
→ lock da sessão
→ DecisionState
→ FutureGraph/Search
→ resposta
```

Logs normais de processo continuam permitidos, mas não fazem parte de um sistema de histórico da partida.

## Futuro

Persistência e observabilidade podem ser reintroduzidas depois como trabalho separado, desde que não adicionem latência significativa ao caminho crítico de decisão.
