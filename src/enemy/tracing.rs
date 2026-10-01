#![allow(dead_code)]

use std::collections::HashMap;

use crate::analysis::{
    EnclosureAnalysis, StateAnalysis, TacticalStateAnalysis, TerritoryAnalysis,
};
use crate::direction::{Direction, MoveMask};
use crate::forecast::ForecastCertainty;
use crate::modes::{food, hunting};
use crate::simulation::mobility::{DeterministicMoveBlock, MobilityAnalysis};
use crate::simulation::state::{SimulatedGameState, SimulatedSnake};

pub(crate) const FOOD_PRESSURE_HEALTH: i32 = 40;
pub(crate) const FOOD_NEAR_DISTANCE: u16 = 3;
pub(crate) const FOOD_ROUTE_MARGIN: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EliminationStage {
    Hard,
    Plausibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveEliminationReason {
    OutOfBounds,
    DeterministicBodyCollision,
    FatalHazard,
    Starvation,
    InsufficientSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoveElimination {
    pub(crate) direction: Direction,
    pub(crate) stage: EliminationStage,
    pub(crate) reason: MoveEliminationReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnemyMoveSet {
    pub(crate) legal_moves: MoveMask,
    pub(crate) plausible_moves: MoveMask,
    pub(crate) eliminations: Vec<MoveElimination>,
}

impl EnemyMoveSet {
    pub(crate) fn search_moves(&self) -> MoveMask {
        if !self.plausible_moves.is_empty() {
            self.plausible_moves
        } else if !self.legal_moves.is_empty() {
            self.legal_moves
        } else {
            MoveMask::all()
        }
    }

    pub(crate) fn pruning_ratio(&self) -> f32 {
        let legal = self.legal_moves.len();
        if legal == 0 {
            return 0.0;
        }

        1.0 - f32::from(self.plausible_moves.len()) / f32::from(legal)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EnemyTracingOutput {
    pub(crate) enemies: HashMap<String, EnemyMoveSet>,
}

impl EnemyTracingOutput {
    pub(crate) fn for_enemy(&self, snake_id: &str) -> Option<&EnemyMoveSet> {
        self.enemies.get(snake_id)
    }
}

pub(crate) fn trace(state: &SimulatedGameState, analysis: &StateAnalysis) -> EnemyTracingOutput {
    let mobility = MobilityAnalysis::from_state(state);
    trace_with_mobility(state, analysis, &mobility)
}

pub(crate) fn trace_with_mobility(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    mobility: &MobilityAnalysis,
) -> EnemyTracingOutput {
    let mut enemies = HashMap::new();

    for enemy in state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != state.our_snake_id)
    {
        enemies.insert(
            enemy.id.clone(),
            trace_enemy(state, analysis, mobility, enemy),
        );
    }

    EnemyTracingOutput { enemies }
}

fn trace_enemy(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    mobility: &MobilityAnalysis,
    enemy: &SimulatedSnake,
) -> EnemyMoveSet {
    let mut legal_moves = MoveMask::empty();
    let mut eliminations = Vec::new();

    for direction in Direction::ALL {
        match mobility.classify_move(state, enemy, direction) {
            None => legal_moves.insert(direction),
            Some(block) => eliminations.push(MoveElimination {
                direction,
                stage: EliminationStage::Hard,
                reason: hard_reason(block),
            }),
        }
    }

    let structural_moves =
        apply_space_filter(state, mobility, enemy, legal_moves, &mut eliminations);

    let survival_moves = survival_policy_moves(state, mobility, enemy, structural_moves);
    let food_moves = food_policy_moves(state, analysis, enemy, structural_moves);
    let hunting_moves = hunting_policy_moves(state, enemy, structural_moves);
    let threat_moves = head_threat_moves(state, mobility, enemy, structural_moves);

    let mut plausible_moves = survival_moves;
    plausible_moves.union_with(food_moves);
    plausible_moves.union_with(hunting_moves);
    plausible_moves.union_with(threat_moves);

    if plausible_moves.is_empty() {
        plausible_moves = structural_moves;
    }

    debug_assert!(plausible_moves.is_subset(legal_moves));

    EnemyMoveSet {
        legal_moves,
        plausible_moves,
        eliminations,
    }
}

fn hard_reason(block: DeterministicMoveBlock) -> MoveEliminationReason {
    match block {
        DeterministicMoveBlock::OutOfBounds => MoveEliminationReason::OutOfBounds,
        DeterministicMoveBlock::DeterministicBodyCollision => {
            MoveEliminationReason::DeterministicBodyCollision
        }
        DeterministicMoveBlock::FatalHazard => MoveEliminationReason::FatalHazard,
        DeterministicMoveBlock::Starvation => MoveEliminationReason::Starvation,
    }
}

fn apply_space_filter(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
    enemy: &SimulatedSnake,
    current: MoveMask,
    eliminations: &mut Vec<MoveElimination>,
) -> MoveMask {
    if current.is_empty() {
        return current;
    }

    let viable = MoveMask::from_iter(current.iter().filter(|direction| {
        mobility.reachable_space(state, &enemy.id, *direction) >= enemy.length() as u32
    }));

    conservative_filter(
        current,
        viable,
        MoveEliminationReason::InsufficientSpace,
        eliminations,
    )
}

fn survival_policy_moves(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    if current.is_empty() {
        return current;
    }

    let scored = current
        .iter()
        .map(|direction| {
            (
                direction,
                mobility.reachable_space(state, &enemy.id, direction),
            )
        })
        .collect::<Vec<_>>();
    let best_space = scored
        .iter()
        .map(|(_, space)| *space)
        .max()
        .unwrap_or(0);
    let floor = best_space.saturating_mul(3).saturating_div(4);

    MoveMask::from_iter(
        scored
            .into_iter()
            .filter(|(_, space)| *space >= floor)
            .map(|(direction, _)| direction),
    )
}

fn food_policy_moves(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    if current.is_empty() || state.food.is_empty() {
        return MoveMask::empty();
    }

    let output = food::candidates_for_actor(
        state,
        analysis,
        &enemy.id,
        ForecastCertainty::Plausible,
    );
    let candidates = MoveMask::from_iter(
        output
            .candidates
            .iter()
            .map(|candidate| candidate.first_move),
    );

    current.intersection(candidates)
}

fn hunting_policy_moves(
    state: &SimulatedGameState,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    if current.is_empty() {
        return current;
    }

    let mut perspective = state.clone();
    perspective.our_snake_id = enemy.id.clone();

    let mobility = MobilityAnalysis::from_state(&perspective);
    let structural_tracing = structural_trace(&perspective, &mobility);
    let tactical =
        TacticalStateAnalysis::from_parts(&perspective, &structural_tracing, &mobility);
    let territory = TerritoryAnalysis::from_state(&perspective);
    let enclosure = EnclosureAnalysis::from_parts(&perspective, &territory, &tactical);
    let output = hunting::analyze(
        &perspective,
        &tactical,
        &structural_tracing,
        &territory,
        &enclosure,
    );

    let candidates = MoveMask::from_iter(
        output
            .candidates
            .iter()
            .map(|candidate| candidate.first_move),
    );

    current.intersection(candidates)
}

fn head_threat_moves(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    let Some(ours) = state
        .snake(&state.our_snake_id)
        .filter(|snake| snake.alive)
    else {
        return MoveMask::empty();
    };
    if enemy.length() < ours.length() {
        return MoveMask::empty();
    }

    let Some(our_head) = ours.head() else {
        return MoveMask::empty();
    };
    let Some(enemy_head) = enemy.head() else {
        return MoveMask::empty();
    };

    let our_moves = mobility.deterministic_moves_for(state, &state.our_snake_id);
    let our_destinations = our_moves
        .iter()
        .map(|direction| direction.apply(our_head))
        .collect::<Vec<_>>();

    MoveMask::from_iter(current.iter().filter(|direction| {
        let target = direction.apply(enemy_head);
        our_destinations.contains(&target)
    }))
}

fn structural_trace(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
) -> EnemyTracingOutput {
    let mut enemies = HashMap::new();

    for snake in state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != state.our_snake_id)
    {
        enemies.insert(
            snake.id.clone(),
            structural_move_set(state, mobility, snake),
        );
    }

    EnemyTracingOutput { enemies }
}

fn structural_move_set(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
    snake: &SimulatedSnake,
) -> EnemyMoveSet {
    let mut legal_moves = MoveMask::empty();
    let mut eliminations = Vec::new();

    for direction in Direction::ALL {
        match mobility.classify_move(state, snake, direction) {
            None => legal_moves.insert(direction),
            Some(block) => eliminations.push(MoveElimination {
                direction,
                stage: EliminationStage::Hard,
                reason: hard_reason(block),
            }),
        }
    }

    let plausible_moves =
        apply_space_filter(state, mobility, snake, legal_moves, &mut eliminations);

    EnemyMoveSet {
        legal_moves,
        plausible_moves,
        eliminations,
    }
}

fn conservative_filter(
    current: MoveMask,
    preferred: MoveMask,
    reason: MoveEliminationReason,
    eliminations: &mut Vec<MoveElimination>,
) -> MoveMask {
    let filtered = current.intersection(preferred);
    if filtered.is_empty() {
        return current;
    }

    for direction in current.difference(filtered).iter() {
        eliminations.push(MoveElimination {
            direction,
            stage: EliminationStage::Plausibility,
            reason,
        });
    }

    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{AggressionState, RulesContext};
    use crate::Coord;

    fn snake(id: &str, health: i32, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>, food: Vec<Coord>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food,
            hazards: vec![],
            snakes,
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    fn analyze(state: &SimulatedGameState) -> StateAnalysis {
        StateAnalysis::from_simulated(state)
    }

    #[test]
    fn no_enemies_produces_empty_output() {
        let state = state(vec![snake("ours", 100, &[(1, 1)])], vec![]);
        let output = trace(&state, &analyze(&state));

        assert!(output.enemies.is_empty());
    }

    #[test]
    fn dead_enemies_are_ignored() {
        let mut dead = snake("dead", 100, &[(4, 4)]);
        dead.alive = false;
        let state = state(vec![snake("ours", 100, &[(1, 1)]), dead], vec![]);

        let output = trace(&state, &analyze(&state));

        assert!(output.enemies.is_empty());
    }

    #[test]
    fn hard_constraints_remove_wall_and_retained_body() {
        let state = state(
            vec![
                snake("ours", 100, &[(5, 5), (5, 4)]),
                snake("enemy", 100, &[(0, 1), (0, 0), (1, 0)]),
            ],
            vec![],
        );
        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(!enemy.legal_moves.contains(Direction::Left));
        assert!(!enemy.legal_moves.contains(Direction::Down));
        assert!(enemy.plausible_moves.is_subset(enemy.legal_moves));
    }

    #[test]
    fn possible_head_to_head_cell_remains_available() {
        let state = state(
            vec![
                snake("ours", 100, &[(2, 1), (1, 1)]),
                snake("enemy", 100, &[(2, 3), (3, 3)]),
            ],
            vec![],
        );
        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(enemy.legal_moves.contains(Direction::Down));
        assert!(enemy.plausible_moves.contains(Direction::Down));
    }

    #[test]
    fn space_filter_removes_obvious_one_cell_trap() {
        let state = state(
            vec![
                snake("ours", 100, &[(6, 6), (6, 5)]),
                snake("enemy", 100, &[(1, 2), (1, 1), (0, 1), (0, 0)]),
                snake(
                    "wall",
                    100,
                    &[(2, 3), (3, 3), (3, 2), (3, 1), (2, 1), (4, 1)],
                ),
            ],
            vec![],
        );
        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(enemy.legal_moves.contains(Direction::Right));
        assert!(!enemy.plausible_moves.contains(Direction::Right));
        assert!(enemy.plausible_moves.contains(Direction::Left));
    }

    #[test]
    fn food_policy_preserves_all_equal_shortest_first_moves() {
        let food = Coord { x: 2, y: 2 };
        let state = state(
            vec![
                snake("ours", 100, &[(6, 6), (6, 5)]),
                snake("enemy", 20, &[(1, 1), (1, 0)]),
            ],
            vec![food],
        );
        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(enemy.plausible_moves.contains(Direction::Up));
        assert!(enemy.plausible_moves.contains(Direction::Right));
        assert!(enemy.plausible_moves.is_subset(enemy.legal_moves));
    }

    #[test]
    fn larger_enemy_keeps_head_to_head_hunt_even_when_food_is_available() {
        let food = Coord { x: 4, y: 3 };
        let state = state(
            vec![
                snake("ours", 100, &[(2, 1), (1, 1)]),
                snake("enemy", 100, &[(2, 3), (3, 3), (3, 2)]),
            ],
            vec![food],
        );
        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(enemy.legal_moves.contains(Direction::Down));
        assert!(enemy.plausible_moves.contains(Direction::Right));
        assert!(
            enemy.plausible_moves.contains(Direction::Down),
            "food intent must not hide a winning head-to-head move"
        );
    }

    #[test]
    fn clearly_lost_food_does_not_force_food_intent() {
        let food = Coord { x: 2, y: 0 };
        let state = state(
            vec![
                snake("ours", 100, &[(1, 0), (1, 1)]),
                snake("enemy", 20, &[(5, 0), (5, 1)]),
            ],
            vec![food],
        );
        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(enemy.plausible_moves.len() >= 2);
    }

    #[test]
    fn empty_legal_set_falls_back_to_all_search_moves() {
        let mut state = state(
            vec![
                snake("ours", 100, &[(6, 6), (6, 5)]),
                snake("enemy", 1, &[(0, 0), (0, 1)]),
            ],
            vec![],
        );
        state.hazards = vec![Coord { x: 1, y: 0 }, Coord { x: 0, y: 1 }];
        state.rules.hazard_damage_per_turn = 100;

        let output = trace(&state, &analyze(&state));
        let enemy = output.for_enemy("enemy").unwrap();

        assert!(enemy.legal_moves.is_empty());
        assert_eq!(enemy.search_moves(), MoveMask::all());
    }

    #[test]
    fn pruning_ratio_is_derived_from_move_sets() {
        let set = EnemyMoveSet {
            legal_moves: MoveMask::all(),
            plausible_moves: MoveMask::from_iter([Direction::Up, Direction::Right]),
            eliminations: vec![],
        };

        assert!((set.pruning_ratio() - 0.5).abs() < f32::EPSILON);
    }
}
