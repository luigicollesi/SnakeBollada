#![allow(dead_code)]

use rayon::prelude::*;

use crate::direction::{Direction, MoveMask};
use crate::enemy::profile::OpponentProfile;
use crate::evaluation::ActorVec;
use crate::simulation::mobility::{DeterministicMoveBlock, MobilityAnalysis};
use crate::simulation::state::{ActorIndex, SimulatedGameState, SimulatedSnake};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThreatClass {
    None,
    Possible,
    Likely,
    Forced,
}

impl ThreatClass {
    pub(crate) const fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Possible => 1,
            Self::Likely => 2,
            Self::Forced => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct OpponentPolicySupport {
    pub(crate) survival: bool,
    pub(crate) food: bool,
    pub(crate) hunting: bool,
    pub(crate) head_threat: bool,
}

impl OpponentPolicySupport {
    pub(crate) fn count(self) -> u8 {
        u8::from(self.survival)
            .saturating_add(u8::from(self.food))
            .saturating_add(u8::from(self.hunting))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpponentMoveHypothesis {
    pub(crate) direction: Direction,
    pub(crate) support: OpponentPolicySupport,
    pub(crate) threat: ThreatClass,
    pub(crate) plausibility_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnemyMoveSet {
    pub(crate) legal_moves: MoveMask,
    pub(crate) plausible_moves: MoveMask,
    pub(crate) hypotheses: Vec<OpponentMoveHypothesis>,
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

    pub(crate) fn ordered_search_moves(&self) -> Vec<Direction> {
        self.ordered_search_moves_with_profile(None)
    }
    pub(crate) fn ordered_legal_moves_with_profile(
        &self,
        profile: Option<&OpponentProfile>,
    ) -> Vec<Direction> {
        let legal = if self.legal_moves.is_empty() {
            MoveMask::all()
        } else {
            self.legal_moves
        };
        self.ordered_moves(legal, profile)
    }

    pub(crate) fn ordered_search_moves_with_profile(
        &self,
        profile: Option<&OpponentProfile>,
    ) -> Vec<Direction> {
        self.ordered_moves(self.search_moves(), profile)
    }

    fn ordered_moves(
        &self,
        moves_mask: MoveMask,
        profile: Option<&OpponentProfile>,
    ) -> Vec<Direction> {
        let mut moves = moves_mask.iter().collect::<Vec<_>>();
        moves.sort_by(|left, right| {
            let left_hypothesis = self.hypothesis(*left);
            let right_hypothesis = self.hypothesis(*right);

            right_hypothesis
                .map_or(0, |hypothesis| hypothesis.threat.rank())
                .cmp(&left_hypothesis.map_or(0, |hypothesis| hypothesis.threat.rank()))
                .then_with(|| {
                    let left_plausibility = left_hypothesis.map_or(0, |hypothesis| {
                        profile.map_or(hypothesis.plausibility_milli, |profile| {
                            profile.adjusted_plausibility(hypothesis)
                        })
                    });
                    let right_plausibility = right_hypothesis.map_or(0, |hypothesis| {
                        profile.map_or(hypothesis.plausibility_milli, |profile| {
                            profile.adjusted_plausibility(hypothesis)
                        })
                    });
                    right_plausibility.cmp(&left_plausibility)
                })
                .then_with(|| left.rank().cmp(&right.rank()))
        });
        moves
    }

    pub(crate) fn hypothesis(&self, direction: Direction) -> Option<OpponentMoveHypothesis> {
        self.hypotheses
            .iter()
            .copied()
            .find(|hypothesis| hypothesis.direction == direction)
    }

    pub(crate) fn threat_class(&self, direction: Direction) -> ThreatClass {
        self.hypothesis(direction).map_or_else(
            || {
                if self.plausible_moves.contains(direction) {
                    ThreatClass::Likely
                } else {
                    ThreatClass::None
                }
            },
            |hypothesis| hypothesis.threat,
        )
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
    pub(crate) enemies: ActorVec<EnemyMoveSet>,
}

impl EnemyTracingOutput {
    pub(crate) fn for_actor(&self, actor: ActorIndex) -> Option<&EnemyMoveSet> {
        self.enemies.get(actor)
    }
}

pub(crate) fn trace_actor_relative_with_mobility(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
) -> EnemyTracingOutput {
    let enemies = state
        .snakes
        .par_iter()
        .enumerate()
        .filter(|(_, snake)| snake.alive && snake.id != state.our_snake_id)
        .filter_map(|(index, enemy)| {
            Some((
                ActorIndex::new(index)?,
                trace_enemy_actor_relative(state, mobility, enemy),
            ))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<ActorVec<_>>();

    EnemyTracingOutput { enemies }
}

fn trace_enemy_actor_relative(
    state: &SimulatedGameState,
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

    let food_moves = cheap_food_ordering_moves(state, enemy, legal_moves);
    let hunting_moves = hunting_policy_moves(state, enemy, legal_moves);
    let threat_moves = head_threat_moves(state, mobility, enemy, legal_moves);
    let hypotheses = build_hypotheses(
        legal_moves,
        legal_moves,
        MoveMask::empty(),
        food_moves,
        hunting_moves,
        threat_moves,
    );

    EnemyMoveSet {
        legal_moves,
        plausible_moves: legal_moves,
        hypotheses,
        eliminations,
    }
}

fn build_hypotheses(
    structural_moves: MoveMask,
    plausible_moves: MoveMask,
    survival_moves: MoveMask,
    food_moves: MoveMask,
    hunting_moves: MoveMask,
    threat_moves: MoveMask,
) -> Vec<OpponentMoveHypothesis> {
    plausible_moves
        .iter()
        .map(|direction| {
            let support = OpponentPolicySupport {
                survival: survival_moves.contains(direction),
                food: food_moves.contains(direction),
                hunting: hunting_moves.contains(direction),
                head_threat: threat_moves.contains(direction),
            };
            let threat = if !support.head_threat {
                ThreatClass::None
            } else if structural_moves.len() == 1 {
                ThreatClass::Forced
            } else if support.hunting || support.survival || support.count() >= 2 {
                ThreatClass::Likely
            } else {
                ThreatClass::Possible
            };
            let threat_bonus = match threat {
                ThreatClass::None => 0,
                ThreatClass::Possible => 140,
                ThreatClass::Likely => 260,
                ThreatClass::Forced => 380,
            };
            let plausibility_milli = 80_u16
                .saturating_add(u16::from(support.survival) * 260)
                .saturating_add(u16::from(support.food) * 220)
                .saturating_add(u16::from(support.hunting) * 300)
                .saturating_add(threat_bonus)
                .min(1000);

            OpponentMoveHypothesis {
                direction,
                support,
                threat,
                plausibility_milli,
            }
        })
        .collect()
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

fn cheap_food_ordering_moves(
    state: &SimulatedGameState,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    if current.is_empty() || state.food.is_empty() {
        return MoveMask::empty();
    }

    let Some(head) = enemy.head() else {
        return MoveMask::empty();
    };

    let best = current
        .iter()
        .map(|direction| {
            let destination = direction.apply(head);
            state
                .food
                .iter()
                .map(|food| {
                    destination
                        .x
                        .abs_diff(food.x)
                        .saturating_add(destination.y.abs_diff(food.y))
                })
                .min()
                .unwrap_or(u32::MAX)
        })
        .min()
        .unwrap_or(u32::MAX);

    MoveMask::from_iter(current.iter().filter(|direction| {
        let destination = direction.apply(head);
        state
            .food
            .iter()
            .map(|food| {
                destination
                    .x
                    .abs_diff(food.x)
                    .saturating_add(destination.y.abs_diff(food.y))
            })
            .min()
            .unwrap_or(u32::MAX)
            == best
    }))
}

fn hunting_policy_moves(
    state: &SimulatedGameState,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    if current.is_empty() {
        return current;
    }

    // Hunting intent is no longer recomputed through a full actor-perspective
    // analysis here. The actor-relative search evaluates the actual utility of
    // each resolved child state. Tracing only keeps hunting-capable structural
    // moves available so it does not prune them before the utility layer sees
    // them.
    let has_size_advantage = state
        .snakes
        .iter()
        .any(|target| target.alive && target.id != enemy.id && enemy.length() > target.length());

    if has_size_advantage {
        current
    } else {
        MoveMask::empty()
    }
}

fn head_threat_moves(
    state: &SimulatedGameState,
    mobility: &MobilityAnalysis,
    enemy: &SimulatedSnake,
    current: MoveMask,
) -> MoveMask {
    let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enemy::profile::OpponentProfile;
    use crate::simulation::state::{RulesContext};
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
                    }
    }

    fn actor(state: &SimulatedGameState, actor_id: &str) -> ActorIndex {
        state.actor_index(actor_id).expect("actor must exist")
    }

    #[test]
    fn actor_relative_trace_keeps_every_legal_move() {
        let state = state(
            vec![
                snake("ours", 100, &[(1, 1), (1, 0)]),
                snake("enemy", 100, &[(5, 5), (5, 4), (5, 3), (4, 3)]),
            ],
            vec![Coord { x: 3, y: 5 }],
        );
        let mobility = MobilityAnalysis::from_state(&state);

        let traced = trace_actor_relative_with_mobility(&state, &mobility);
        let enemy = traced.for_actor(actor(&state, "enemy")).unwrap();
        let deterministic = mobility.deterministic_moves_for(&state, "enemy");

        assert_eq!(enemy.legal_moves, deterministic);
        assert_eq!(enemy.plausible_moves, deterministic);
        assert!(enemy
            .eliminations
            .iter()
            .all(|elimination| elimination.stage == EliminationStage::Hard));
    }

    #[test]
    fn dead_enemies_are_ignored() {
        let mut dead = snake("dead", 100, &[(4, 4)]);
        dead.alive = false;
        let state = state(vec![snake("ours", 100, &[(1, 1)]), dead], vec![]);
        let mobility = MobilityAnalysis::from_state(&state);

        let output = trace_actor_relative_with_mobility(&state, &mobility);

        assert!(output.enemies.is_empty());
    }

    #[test]
    fn hard_constraints_remove_illegal_moves_without_plausibility_pruning() {
        let state = state(
            vec![
                snake("ours", 100, &[(5, 5), (5, 4)]),
                snake("enemy", 100, &[(0, 1), (0, 0), (1, 0)]),
            ],
            vec![],
        );
        let mobility = MobilityAnalysis::from_state(&state);
        let output = trace_actor_relative_with_mobility(&state, &mobility);
        let enemy = output.for_actor(actor(&state, "enemy")).unwrap();

        assert!(!enemy.legal_moves.contains(Direction::Left));
        assert!(!enemy.legal_moves.contains(Direction::Down));
        assert_eq!(enemy.plausible_moves, enemy.legal_moves);
    }

    #[test]
    fn size_advantage_keeps_hunting_support_available() {
        let state = state(
            vec![
                snake("ours", 100, &[(2, 1), (1, 1)]),
                snake("enemy", 100, &[(2, 3), (2, 4), (1, 4)]),
            ],
            vec![Coord { x: 4, y: 3 }],
        );
        let mobility = MobilityAnalysis::from_state(&state);
        let output = trace_actor_relative_with_mobility(&state, &mobility);
        let enemy = output.for_actor(actor(&state, "enemy")).unwrap();

        assert!(enemy.legal_moves.contains(Direction::Down));
        let attack = enemy.hypothesis(Direction::Down).unwrap();
        assert!(attack.support.hunting);
        assert!(attack.support.head_threat);
    }

    #[test]
    fn profile_changes_order_without_removing_legal_moves() {
        let state = state(
            vec![
                snake("ours", 100, &[(2, 1), (1, 1)]),
                snake("enemy", 100, &[(2, 3), (2, 4), (1, 4)]),
            ],
            vec![Coord { x: 4, y: 3 }],
        );
        let mobility = MobilityAnalysis::from_state(&state);
        let output = trace_actor_relative_with_mobility(&state, &mobility);
        let enemy = output.for_actor(actor(&state, "enemy")).unwrap();
        let profile = OpponentProfile {
            hunting_bias_milli: 1500,
            ..OpponentProfile::default()
        };

        let ordered = enemy.ordered_legal_moves_with_profile(Some(&profile));

        assert_eq!(ordered.len(), enemy.legal_moves.len() as usize);
        assert!(ordered
            .iter()
            .all(|direction| enemy.legal_moves.contains(*direction)));
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
        let mobility = MobilityAnalysis::from_state(&state);

        let output = trace_actor_relative_with_mobility(&state, &mobility);
        let enemy = output.for_actor(actor(&state, "enemy")).unwrap();

        assert!(enemy.legal_moves.is_empty());
        assert_eq!(enemy.search_moves(), MoveMask::all());
    }
}
