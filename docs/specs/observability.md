# SPEC — Observabilidade e Memória de Partida

**Status:** Planejado  
**Branch alvo:** `dev`  
**Escopo:** Battlesnake API v1  
**Objetivo:** manter estado vivo confiável durante a partida e registrar um histórico completo, sem bloquear o caminho crítico de `/move`.

## 1. Objetivos

O sistema deve:

1. tratar `POST /start` como início formal de uma sessão de jogo;
2. criar uma sessão identificada por `game.id`;
3. manter, a cada turno, o estado atual de todas as casas ocupadas por:
   - SnakeBollada;
   - cada adversário individualmente;
   - todos os adversários em conjunto;
4. registrar snapshots de tabuleiro ao longo da partida;
5. registrar a decisão tomada pela SnakeBollada em cada turno;
6. inferir movimentos observáveis dos adversários comparando a cabeça entre snapshots consecutivos;
7. registrar eliminações sem inventar movimentos não observados;
8. finalizar a sessão em `POST /end`;
9. persistir um registro completo da partida;
10. manter telemetria fora do caminho crítico da decisão.

## 2. Não objetivos

Nesta primeira versão, o sistema não deve:

- alterar a lógica de movimentação;
- depender de banco de dados remoto;
- fazer machine learning;
- reconstruir estados que contradigam o snapshot recebido da API;
- bloquear `/move` esperando escrita em disco;
- tentar inferir o movimento fatal de uma cobra que desapareceu entre snapshots quando ele não puder ser determinado.

## 3. Princípio de arquitetura

A API recebida do Battlesnake é a fonte de verdade.

A aplicação mantém dois conceitos separados:

### Live Game State

Representa o que existe **agora**.

Responsabilidades:

- posição atual;
- ocupação de casas;
- estado de cada cobra;
- comida;
- hazards;
- turno atual.

### Telemetry / Game Record

Representa o que **aconteceu ao longo do tempo**.

Responsabilidades:

- snapshots;
- decisões;
- movimentos observados;
- tempos de decisão;
- eventos de início/fim;
- eliminações;
- metadados da partida.

Essa separação é obrigatória.

## 4. Fluxo de alto nível

```text
POST /start
    |
    v
create GameSession(game.id)
    |
    +--> initialize LiveGameState
    |
    +--> spawn telemetry recorder
    |
    +--> record turn 0

POST /move
    |
    v
parse request
    |
    +--> rebuild current LiveGameState
    |
    +--> derive opponent movement from previous snapshot
    |
    +--> enqueue TurnSnapshot
    |
    +--> DecisionEngine
    |
    +--> enqueue OurDecision
    |
    v
return move

POST /end
    |
    v
enqueue End
    |
    +--> close recorder
    +--> persist GameRecord
    +--> remove GameSession from registry
```

## 5. Registro global de partidas

Deve existir um registry concorrente:

```rust
GameRegistry
    game_id -> GameHandle
```

Estrutura proposta:

```rust
struct GameHandle {
    live_state: Arc<RwLock<LiveGameState>>,
    telemetry_tx: mpsc::Sender<TelemetryEvent>,
}
```

Implementação sugerida:

- `DashMap<String, GameHandle>` para registry;
- `tokio::sync::RwLock` para estado compartilhado;
- `tokio::sync::mpsc` para telemetria;
- `tokio::spawn` para recorder da partida.

## 6. Representação de casas

O runtime precisa responder rapidamente:

- esta casa está ocupada?
- por nós?
- por qualquer adversário?
- por qual adversário?

Representação inicial:

```rust
struct BoardMask {
    width: u16,
    height: u16,
    bits: Vec<u64>,
}
```

Mapeamento:

```text
index = y * width + x
```

Máscaras mínimas:

```rust
our_cells: BoardMask
opponent_cells: BoardMask
occupied_cells: BoardMask
food_cells: BoardMask
hazard_cells: BoardMask
```

Também deve existir ocupação individual por cobra:

```rust
snake_cells: HashMap<SnakeId, BoardMask>
```

## 7. Estado vivo de cada cobra

```rust
struct SnakeLiveState {
    id: String,
    name: String,
    head: Coord,
    body: Vec<Coord>,
    occupied: BoardMask,
    health: i32,
    length: u32,
    alive: bool,
    last_observed_move: Option<ObservedMove>,
}
```

O corpo recebido no request deve sempre substituir qualquer reconstrução local.

## 8. Eventos de telemetria

```rust
enum TelemetryEvent {
    Start(StartRecord),
    TurnSnapshot(TurnSnapshot),
    OurDecision(DecisionRecord),
    End(EndRecord),
}
```

O envio deve preferencialmente não bloquear o request.

Política inicial:

- usar canal com capacidade limitada;
- evitar qualquer I/O síncrono dentro de `/move`;
- se o canal estiver temporariamente cheio, registrar erro de telemetria, mas não sacrificar a resposta da Battlesnake.

## 9. Snapshot por turno

Cada snapshot deve preservar informação suficiente para reproduzir o estado observado.

```rust
struct TurnSnapshot {
    turn: u32,
    captured_at: InstantOrTimestamp,
    food: Vec<Coord>,
    hazards: Vec<Coord>,
    snakes: Vec<SnakeSnapshot>,
    observed_moves: HashMap<String, ObservedMove>,
}
```

```rust
struct SnakeSnapshot {
    id: String,
    name: String,
    health: i32,
    length: u32,
    head: Coord,
    body: Vec<Coord>,
}
```

## 10. Movimento da SnakeBollada

Para nossa cobra, o movimento é conhecido antes da resposta.

```rust
struct DecisionRecord {
    turn: u32,
    chosen_move: Direction,
    decision_time_us: u64,
    strategy_version: String,
}
```

O campo `strategy_version` deve existir desde a V1 para permitir comparar algoritmos futuramente.

## 11. Movimento dos adversários

Quando uma cobra aparece em dois snapshots consecutivos:

```text
delta = current_head - previous_head
```

Mapeamento:

```text
( 0, +1) -> Up
( 0, -1) -> Down
(+1,  0) -> Right
(-1,  0) -> Left
```

Tipo proposto:

```rust
enum ObservedMove {
    Known(Direction),
    EliminatedUnknown,
    NotAvailable,
}
```

### Regra obrigatória

Se um adversário desapareceu no snapshot seguinte, não inferir automaticamente seu movimento fatal.

Registrar:

```rust
ObservedMove::EliminatedUnknown
```

quando não houver evidência suficiente.

## 12. Registro final da partida

```rust
struct GameRecord {
    schema_version: u32,
    game_id: String,
    ruleset: Value,
    timeout_ms: u32,
    board_width: u32,
    board_height: u32,
    our_snake_id: String,
    started_at: Timestamp,
    ended_at: Timestamp,
    turns: Vec<TurnRecord>,
}
```

Cada `TurnRecord` deve poder relacionar:

- snapshot recebido;
- movimentos adversários observados;
- movimento escolhido por nós;
- tempo de decisão;
- estratégia utilizada.

## 13. Persistência V1

Formato inicial:

```text
data/
└── games/
    └── <game-id>.json
```

Requisitos:

- uma partida por arquivo;
- JSON legível;
- escrita apenas fora do caminho crítico;
- gravação atômica quando possível;
- `schema_version` obrigatório.

A interface de armazenamento deve ser abstraída:

```rust
trait GameRecordStorage {
    async fn save(&self, record: &GameRecord) -> Result<()>;
}
```

Isso permitirá posteriormente substituir filesystem por:

- SQLite;
- PostgreSQL;
- S3/R2;
- Parquet.

## 14. Organização proposta

```text
src/
├── game/
│   ├── board.rs
│   ├── direction.rs
│   ├── mask.rs
│   ├── snake.rs
│   └── state.rs
├── runtime/
│   ├── registry.rs
│   └── session.rs
└── telemetry/
    ├── event.rs
    ├── record.rs
    ├── recorder.rs
    └── storage.rs
```

## 15. Requisitos de desempenho

No endpoint `/move`:

- nenhuma escrita síncrona em disco;
- nenhum flush;
- nenhuma serialização completa do histórico;
- atualização do estado deve ser O(células ocupadas);
- enqueue de telemetria deve ter custo desprezível perto do orçamento de decisão.

A lógica de observabilidade nunca deve ser causa direta de timeout.

## 16. Recuperação e robustez

Casos obrigatórios:

- `/move` chega sem sessão conhecida:
  - reconstruir sessão a partir do request;
  - registrar warning;
  - continuar jogando;
- `/start` duplicado:
  - não criar workers concorrentes para o mesmo `game.id`;
- `/end` duplicado:
  - operação idempotente;
- recorder falha:
  - jogo continua;
  - erro é registrado;
- partida nunca recebe `/end`:
  - registry deve futuramente suportar expiração por TTL.

## 17. Testes mínimos

### BoardMask

- set/get de coordenadas;
- células de borda;
- tabuleiro maior que 64 células;
- união/interseção de máscaras.

### LiveGameState

- ocupação nossa correta;
- ocupação adversária correta;
- reconstrução completa a partir do snapshot.

### Movimento observado

- Up;
- Down;
- Left;
- Right;
- cobra eliminada;
- snapshot inicial sem movimento anterior.

### Registry

- duas partidas simultâneas;
- start duplicado;
- end remove sessão.

### Recorder

- eventos preservam ordem;
- fechamento gera `GameRecord`;
- storage falhando não derruba servidor.

## 18. Critérios de aceite

A implementação estará concluída quando:

- [ ] `/start` criar sessão por `game.id`;
- [ ] turno 0 for registrado;
- [ ] cada `/move` atualizar ocupação atual;
- [ ] for possível consultar todas as nossas casas;
- [ ] for possível consultar todas as casas de adversários;
- [ ] for possível consultar as casas de um adversário específico;
- [ ] movimentos conhecidos de adversários forem derivados corretamente;
- [ ] nosso movimento e tempo de decisão forem registrados;
- [ ] `/end` finalizar e remover a sessão;
- [ ] uma partida gerar um arquivo JSON completo;
- [ ] telemetria não executar I/O bloqueante no caminho de `/move`;
- [ ] testes cobrirem os casos acima.

## 19. Referências

- Battlesnake API: https://docs.battlesnake.com/api
- Webhooks: https://docs.battlesnake.com/api/webhooks
- Board object: https://docs.battlesnake.com/api/objects/board
- Battlesnake object: https://docs.battlesnake.com/api/objects/battlesnake
- Tokio: https://tokio.rs/
- DashMap: https://docs.rs/dashmap/
