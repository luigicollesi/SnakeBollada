#![allow(dead_code)]

use super::{ActorVec, StrategicWeights};
use crate::simulation::state::ActorIndex;
use crate::search::graph::{SearchEdge, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};

const FOOD_POTENTIAL_DELTA_SCALE: i64 = 3;
const FOOD_CONSUMED: i64 = 1000;
const TERRITORY_DELTA_SCALE: i64 = 2;
const HUNTING_TERRITORY_BUDGET: i64 = 2000;
const HUNTING_DENIAL_NUMERATOR: i64 = 1;
const HUNTING_DENIAL_DENOMINATOR: i64 = 2;
const MOBILITY_STEP: i64 = 320;
const ENCLOSURE_STEP: i64 = 220;
const BORDER_EXPOSURE_STEP: i64 = 1;
const HEALTH_PRESSURE_STEP: i64 = 1;
const HAZARD_DAMAGE_STEP: i64 = 35;
const KILL_BENEFIT: i64 = 1400;
const TERMINAL_UTILITY: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActorTransitionFacts {
    ate_food: bool,
    food_potential_before: u16,
    food_potential_after: u16,
    territory_share_delta_milli: i16,
    mobility_delta: i8,
    border_risk_improvement_milli: i16,
    border_exposure_milli: u16,
    health_pressure_milli: u16,
    hazard_damage: u16,
    enclosure_improvement: i8,
    hunting_territory_benefit: i64,
    kill_benefit: i64,
    died: bool,
    sole_survivor: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransitionFacts {
    actors: ActorVec<ActorTransitionFacts>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActorTransitionScore {
    pub(crate) food_benefit: i64,
    pub(crate) food_harm: i64,
    pub(crate) hunting_benefit: i64,
    pub(crate) hunting_harm: i64,
    pub(crate) survival_benefit: i64,
    pub(crate) survival_harm: i64,
    pub(crate) terminal_benefit: i64,
    pub(crate) terminal_harm: i64,
    pub(crate) benefit_total: i64,
    pub(crate) harm_total: i64,
    pub(crate) net: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TransitionScore {
    pub(crate) instant_benefit: i64,
    pub(crate) instant_harm: i64,
    pub(crate) net: i64,
    pub(crate) opponent_net_total: i64,
    pub(crate) actors: ActorVec<ActorTransitionScore>,
}

impl TransitionScore {
    pub(crate) fn from_edge(parent: &SearchNode, edge: &SearchEdge, child: &SearchNode) -> Self {
        Self::from_parts(parent, &edge.events, child)
    }

    pub(crate) fn from_parts(
        parent: &SearchNode,
        events: &[InstantEvent],
        child: &SearchNode,
    ) -> Self {
        let facts = TransitionFacts::from_parts(parent, events, child);
        let mut actors = ActorVec::with_capacity(parent.state.snakes.len());

        for (index, actor) in parent
            .state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
        {
            let Some(actor_index) = ActorIndex::new(index) else {
                continue;
            };
            let (Some(parent_eval), Some(actor_facts)) = (
                actor_evaluation(parent, &actor.id),
                facts.actors.get(actor_index),
            ) else {
                continue;
            };

            actors.insert(
                actor_index,
                score_actor_transition(*actor_facts, parent_eval.weights),
            );
        }

        let our_index = parent.state.actor_index(&parent.state.our_snake_id);
        let ours = our_index
            .and_then(|actor| actors.get(actor))
            .copied()
            .unwrap_or_default();
        let opponent_net_total = actors
            .iter()
            .filter(|(actor, _)| Some(*actor) != our_index)
            .fold(0_i64, |sum, (_, score)| sum.saturating_add(score.net));

        Self {
            instant_benefit: ours.benefit_total,
            instant_harm: ours.harm_total,
            net: ours.net,
            opponent_net_total,
            actors,
        }
    }

    pub(crate) fn for_actor(&self, actor: ActorIndex) -> Option<&ActorTransitionScore> {
        self.actors.get(actor)
    }

    pub(crate) fn route_delta(&self) -> i64 {
        self.net.saturating_sub(self.opponent_net_total)
    }
}

impl TransitionFacts {
    fn from_parts(parent: &SearchNode, events: &[InstantEvent], child: &SearchNode) -> Self {
        let hunting_transfers = hunting_territory_benefits(parent, child);
        let mut ate_food = ActorVec::<bool>::with_capacity(parent.state.snakes.len());
        let mut kill_benefits = ActorVec::<i64>::with_capacity(parent.state.snakes.len());

        for event in events {
            match event {
                InstantEvent::AteFood { snake, .. } => {
                    if let Some(actor) = parent.state.actor_index(snake) {
                        ate_food.insert(actor, true);
                    }
                }
                InstantEvent::EnemyKilled {
                    enemy, attribution, ..
                } => {
                    if let Some(killer) = attributed_actor(attribution, &parent.state.our_snake_id)
                    {
                        if killer != enemy {
                            if let Some(actor) = parent.state.actor_index(killer) {
                                kill_benefits.add(actor, KILL_BENEFIT);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let living_after = child
            .state
            .snakes
            .iter()
            .filter(|snake| snake.alive)
            .count();
        let mut actors = ActorVec::with_capacity(parent.state.snakes.len());

        for (index, actor) in parent
            .state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
        {
            let Some(actor_index) = ActorIndex::new(index) else {
                continue;
            };
            let Some(before) = actor_evaluation(parent, &actor.id) else {
                continue;
            };
            let after = actor_evaluation(child, &actor.id);
            let child_actor = child.state.snake_at(actor_index);
            let alive_after = child_actor.is_some_and(|snake| snake.alive);
            let ate_food_now = ate_food.get(actor_index).copied().unwrap_or(false);
            let health_pressure_milli =
                child_actor.filter(|snake| snake.alive).map_or(0, |snake| {
                    health_pressure_milli(snake.health, child.state.rules.max_health)
                });
            let hazard_damage = if ate_food_now || !alive_after {
                0
            } else {
                child_actor.map_or(0, |snake| extra_hazard_damage(actor.health, snake.health))
            };

            let facts = ActorTransitionFacts {
                ate_food: ate_food_now,
                food_potential_before: before.metrics.food_potential_milli,
                food_potential_after: after
                    .map(|snapshot| snapshot.metrics.food_potential_milli)
                    .unwrap_or(0),
                territory_share_delta_milli: after.map_or(0, |snapshot| {
                    signed_i16(
                        i32::from(snapshot.metrics.territory_share_milli)
                            .saturating_sub(i32::from(before.metrics.territory_share_milli)),
                    )
                }),
                mobility_delta: after.map_or(0, |snapshot| {
                    signed_i8(
                        i16::from(snapshot.metrics.safe_non_reverse_moves)
                            .saturating_sub(i16::from(before.metrics.safe_non_reverse_moves)),
                    )
                }),
                border_risk_improvement_milli: after.map_or(0, |snapshot| {
                    signed_i16(
                        i32::from(before.metrics.border_structural_risk_milli).saturating_sub(
                            i32::from(snapshot.metrics.border_structural_risk_milli),
                        ),
                    )
                }),
                border_exposure_milli: after
                    .map(|snapshot| snapshot.metrics.border_exposure_milli)
                    .unwrap_or(0),
                health_pressure_milli,
                hazard_damage,
                enclosure_improvement: after.map_or(0, |snapshot| {
                    signed_i8(
                        i16::from(before.metrics.enclosure_risk)
                            .saturating_sub(i16::from(snapshot.metrics.enclosure_risk)),
                    )
                }),
                hunting_territory_benefit: hunting_transfers.get(actor_index).copied().unwrap_or(0),
                kill_benefit: kill_benefits.get(actor_index).copied().unwrap_or(0),
                died: !alive_after,
                sole_survivor: alive_after && living_after == 1,
            };
            actors.insert(actor_index, facts);
        }

        Self { actors }
    }
}

fn score_actor_transition(
    facts: ActorTransitionFacts,
    weights: StrategicWeights,
) -> ActorTransitionScore {
    let (mut food_benefit, mut food_harm) = if facts.ate_food {
        (FOOD_CONSUMED, 0)
    } else {
        food_potential_delta(facts.food_potential_before, facts.food_potential_after)
    };

    let mut hunting_benefit = facts
        .hunting_territory_benefit
        .saturating_add(facts.kill_benefit);
    let mut hunting_harm = 0_i64;
    let mut survival_benefit = 0_i64;
    let mut survival_harm = 0_i64;

    add_signed_delta(
        i64::from(facts.territory_share_delta_milli).saturating_mul(TERRITORY_DELTA_SCALE),
        &mut survival_benefit,
        &mut survival_harm,
    );
    add_signed_delta(
        i64::from(facts.mobility_delta).saturating_mul(MOBILITY_STEP),
        &mut survival_benefit,
        &mut survival_harm,
    );
    add_signed_delta(
        i64::from(facts.border_risk_improvement_milli),
        &mut survival_benefit,
        &mut survival_harm,
    );
    survival_harm = survival_harm.saturating_add(
        i64::from(facts.border_exposure_milli).saturating_mul(BORDER_EXPOSURE_STEP),
    );
    survival_harm = survival_harm.saturating_add(
        i64::from(facts.health_pressure_milli).saturating_mul(HEALTH_PRESSURE_STEP),
    );
    survival_harm = survival_harm
        .saturating_add(i64::from(facts.hazard_damage).saturating_mul(HAZARD_DAMAGE_STEP));
    add_signed_delta(
        i64::from(facts.enclosure_improvement).saturating_mul(ENCLOSURE_STEP),
        &mut survival_benefit,
        &mut survival_harm,
    );

    food_benefit = weighted(food_benefit, weights.food);
    food_harm = weighted(food_harm, weights.food);
    hunting_benefit = weighted(hunting_benefit, weights.hunting);
    hunting_harm = weighted(hunting_harm, weights.hunting);
    survival_benefit = weighted(survival_benefit, weights.survival);
    survival_harm = weighted(survival_harm, weights.survival);

    let terminal_benefit = if facts.sole_survivor {
        TERMINAL_UTILITY
    } else {
        0
    };
    let terminal_harm = if facts.died { TERMINAL_UTILITY } else { 0 };

    let benefit_total = food_benefit
        .saturating_add(hunting_benefit)
        .saturating_add(survival_benefit)
        .saturating_add(terminal_benefit);
    let harm_total = food_harm
        .saturating_add(hunting_harm)
        .saturating_add(survival_harm)
        .saturating_add(terminal_harm);

    ActorTransitionScore {
        food_benefit,
        food_harm,
        hunting_benefit,
        hunting_harm,
        survival_benefit,
        survival_harm,
        terminal_benefit,
        terminal_harm,
        benefit_total,
        harm_total,
        net: benefit_total.saturating_sub(harm_total),
    }
}

fn actor_evaluation<'a>(
    node: &'a SearchNode,
    actor_id: &str,
) -> Option<&'a crate::evaluation::ActorSnapshot> {
    node.active_analysis()?.actor_snapshot(actor_id)
}

fn health_pressure_milli(health: i32, max_health: i32) -> u16 {
    if max_health <= 0 {
        return 1000;
    }

    let clamped = health.clamp(0, max_health);
    let reserve_milli = i64::from(clamped)
        .saturating_mul(1000)
        .saturating_div(i64::from(max_health))
        .clamp(0, 1000) as u16;

    match reserve_milli {
        0..=100 => 1000,
        101..=200 => interpolate_pressure(reserve_milli, 100, 200, 1000, 700),
        201..=350 => interpolate_pressure(reserve_milli, 200, 350, 700, 350),
        351..=500 => interpolate_pressure(reserve_milli, 350, 500, 350, 120),
        501..=700 => interpolate_pressure(reserve_milli, 500, 700, 120, 0),
        _ => 0,
    }
}

fn interpolate_pressure(value: u16, x0: u16, x1: u16, y0: u16, y1: u16) -> u16 {
    let span = u32::from(x1.saturating_sub(x0)).max(1);
    let offset = u32::from(value.saturating_sub(x0).min(x1.saturating_sub(x0)));
    let drop = u32::from(y0.saturating_sub(y1))
        .saturating_mul(offset)
        .saturating_div(span);
    u32::from(y0).saturating_sub(drop).try_into().unwrap_or(y1)
}

fn extra_hazard_damage(before_health: i32, after_health: i32) -> u16 {
    let expected_after_decay = before_health.saturating_sub(1).max(0);
    expected_after_decay
        .saturating_sub(after_health.max(0))
        .max(0)
        .try_into()
        .unwrap_or(u16::MAX)
}

fn food_potential_delta(before: u16, after: u16) -> (i64, i64) {
    if after > before {
        (
            i64::from(after.saturating_sub(before)).saturating_mul(FOOD_POTENTIAL_DELTA_SCALE),
            0,
        )
    } else if before > after {
        (
            0,
            i64::from(before.saturating_sub(after)).saturating_mul(FOOD_POTENTIAL_DELTA_SCALE),
        )
    } else {
        (0, 0)
    }
}

fn hunting_territory_benefits(parent: &SearchNode, child: &SearchNode) -> ActorVec<i64> {
    let Some(parent_territory) = parent.active_analysis().map(|analysis| &analysis.territory)
    else {
        return ActorVec::new();
    };
    let Some(child_territory) = child.active_analysis().map(|analysis| &analysis.territory) else {
        return ActorVec::new();
    };

    let board_cells = parent
        .state
        .width
        .saturating_mul(parent.state.height)
        .max(1);
    let cell_value = HUNTING_TERRITORY_BUDGET
        .saturating_div(i64::from(board_cells))
        .max(1);
    let denial_value = cell_value
        .saturating_mul(HUNTING_DENIAL_NUMERATOR)
        .saturating_div(HUNTING_DENIAL_DENOMINATOR)
        .max(1);
    let mut benefits = ActorVec::<i64>::with_capacity(parent.state.snakes.len());

    for y in 0..parent.state.height {
        for x in 0..parent.state.width {
            let coord = crate::Coord {
                x: i32::try_from(x).unwrap_or(i32::MAX),
                y: i32::try_from(y).unwrap_or(i32::MAX),
            };
            let Some(previous_owner) = parent_territory.competitive_owner_at(coord) else {
                continue;
            };

            if let Some(new_owner) = child_territory.competitive_owner_at(coord) {
                if previous_owner != new_owner {
                    if let Some(actor) = parent.state.actor_index(new_owner) {
                        benefits.add(actor, cell_value);
                    }
                }
                continue;
            }

            if !child_territory.competitive_is_contested_at(coord) {
                continue;
            }

            let contender_count = child_territory.competitive_contender_count_at(coord).max(1);
            let split_denial = denial_value
                .saturating_div(i64::try_from(contender_count).unwrap_or(i64::MAX))
                .max(1);

            for actor in child.state.snakes.iter().filter(|snake| snake.alive) {
                if actor.id == previous_owner {
                    continue;
                }
                if child_territory.competitive_contested_by(coord, &actor.id) {
                    if let Some(actor_index) = parent.state.actor_index(&actor.id) {
                        benefits.add(actor_index, split_denial);
                    }
                }
            }
        }
    }

    benefits
}

fn attributed_actor<'a>(
    attribution: &'a EliminationAttribution,
    our_id: &'a str,
) -> Option<&'a str> {
    match attribution {
        EliminationAttribution::OurSnake => Some(our_id),
        EliminationAttribution::OtherSnake(killer) => Some(killer.as_str()),
        EliminationAttribution::SelfInflicted | EliminationAttribution::Environment => None,
    }
}

fn signed_i16(value: i32) -> i16 {
    value
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
        .try_into()
        .unwrap_or(if value.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        })
}

fn signed_i8(value: i16) -> i8 {
    value
        .clamp(i16::from(i8::MIN), i16::from(i8::MAX))
        .try_into()
        .unwrap_or(if value.is_negative() {
            i8::MIN
        } else {
            i8::MAX
        })
}

fn add_signed_delta(value: i64, benefit: &mut i64, harm: &mut i64) {
    if value > 0 {
        *benefit = benefit.saturating_add(value);
    } else if value < 0 {
        *harm = harm.saturating_add(value.saturating_neg());
    }
}

fn weighted(raw: i64, weight_milli: u16) -> i64 {
    raw.saturating_mul(i64::from(weight_milli))
        .saturating_div(1000)
}

#[cfg(test)]
mod tests {
    use crate::direction::Direction;
    use crate::search::graph::FutureGraph;
    use crate::simulation::resolver::InstantEvent;
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedGameState, SimulatedSnake,
    };
    use crate::Coord;

    use super::*;

    fn snake(id: &str, health: i32, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(health: i32, food: Vec<Coord>, head_x: i32) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food,
            hazards: vec![],
            snakes: vec![
                snake("ours", health, &[(head_x, 1), (head_x, 0)]),
                snake("enemy", 100, &[(5, 5), (5, 4)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    fn first_edge_for(
        graph: &FutureGraph,
        direction: Direction,
        predicate: impl Fn(&SearchEdge) -> bool,
    ) -> &SearchEdge {
        let root = graph.node(graph.root());
        root.children
            .iter()
            .find(|edge| {
                edge.joint_action.direction_for("ours") == Some(direction) && predicate(edge)
            })
            .expect("expected matching edge")
    }

    fn territory_state(our_head: Coord, enemy_head: Coord) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes: vec![
                snake(
                    "ours",
                    100,
                    &[(our_head.x, our_head.y), (our_head.x, our_head.y - 1)],
                ),
                snake(
                    "enemy",
                    100,
                    &[
                        (enemy_head.x, enemy_head.y),
                        (enemy_head.x, enemy_head.y - 1),
                    ],
                ),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    fn actor_index(state: &SimulatedGameState, actor_id: &str) -> ActorIndex {
        state.actor_index(actor_id).expect("actor must exist")
    }

    #[test]
    fn hunting_credits_actor_capturing_enemy_owned_cells() {
        let parent_graph =
            FutureGraph::new(territory_state(Coord { x: 1, y: 3 }, Coord { x: 5, y: 3 }));
        let child_graph =
            FutureGraph::new(territory_state(Coord { x: 2, y: 3 }, Coord { x: 5, y: 5 }));

        let benefits = hunting_territory_benefits(
            parent_graph.node(parent_graph.root()),
            child_graph.node(child_graph.root()),
        );

        let ours = actor_index(&parent_graph.node(parent_graph.root()).state, "ours");
        assert!(benefits.get(ours).copied().unwrap_or(0) > 0);
    }

    #[test]
    fn contested_denial_only_credits_actual_contender() {
        let parent_graph =
            FutureGraph::new(territory_state(Coord { x: 0, y: 3 }, Coord { x: 6, y: 3 }));
        let child_graph =
            FutureGraph::new(territory_state(Coord { x: 2, y: 3 }, Coord { x: 6, y: 3 }));

        let parent = parent_graph.node(parent_graph.root());
        let child = child_graph.node(child_graph.root());
        let parent_territory = &parent.active_analysis().unwrap().territory;
        let child_territory = &child.active_analysis().unwrap().territory;

        let mut found_denial = false;
        for y in 0..parent.state.height {
            for x in 0..parent.state.width {
                let coord = Coord {
                    x: i32::try_from(x).unwrap(),
                    y: i32::try_from(y).unwrap(),
                };
                if parent_territory.competitive_owner_at(coord) == Some("enemy")
                    && child_territory.competitive_is_contested_at(coord)
                    && child_territory.competitive_contested_by(coord, "ours")
                {
                    found_denial = true;
                }
            }
        }

        assert!(found_denial, "fixture must create causal contested denial");
        let benefits = hunting_territory_benefits(parent, child);
        let ours = actor_index(&parent.state, "ours");
        assert!(benefits.get(ours).copied().unwrap_or(0) > 0);
    }

    #[test]
    fn transition_scores_every_living_actor() {
        let mut graph = FutureGraph::new(state(80, vec![Coord { x: 4, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |_| true);
        let child = graph.node(edge.child);
        let score = TransitionScore::from_edge(root, edge, child);

        let ours = actor_index(&root.state, "ours");
        let enemy = actor_index(&root.state, "enemy");
        assert!(score.for_actor(ours).is_some());
        assert!(score.for_actor(enemy).is_some());
        assert_eq!(score.opponent_net_total, score.for_actor(enemy).unwrap().net);
    }

    #[test]
    fn approaching_food_produces_food_benefit() {
        let mut graph = FutureGraph::new(state(80, vec![Coord { x: 4, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |edge| {
            !edge.events.iter().any(
                |event| matches!(event, InstantEvent::AteFood { snake, .. } if snake == "ours"),
            )
        });
        let child = graph.node(edge.child);
        let score = TransitionScore::from_edge(root, edge, child);
        let ours = score
            .for_actor(actor_index(&root.state, "ours"))
            .unwrap();

        assert!(ours.food_benefit > 0);
        assert!(ours.net > -TERMINAL_UTILITY);
    }

    #[test]
    fn actor_death_and_sole_survivor_are_terminal_without_child_analysis() {
        let parent_state = SimulatedGameState {
            turn: 1,
            width: 5,
            height: 5,
            food: vec![],
            hazards: vec![],
            snakes: vec![
                snake("ours", 100, &[(1, 1), (1, 0)]),
                snake("enemy", 100, &[(3, 1), (3, 0)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        };
        let mut child_state = parent_state.clone();
        child_state.turn = child_state.turn.saturating_add(1);
        child_state.snake_mut("enemy").unwrap().alive = false;

        let parent_graph = FutureGraph::new(parent_state);
        let child_graph = FutureGraph::new(child_state);
        let score = TransitionScore::from_parts(
            parent_graph.node(parent_graph.root()),
            &[],
            child_graph.node(child_graph.root()),
        );

        let parent = parent_graph.node(parent_graph.root());
        let ours = score
            .for_actor(actor_index(&parent.state, "ours"))
            .unwrap();
        let enemy = score
            .for_actor(actor_index(&parent.state, "enemy"))
            .unwrap();

        assert_eq!(ours.terminal_benefit, TERMINAL_UTILITY);
        assert_eq!(ours.terminal_harm, 0);
        assert!(ours.net >= TERMINAL_UTILITY);

        assert_eq!(enemy.terminal_harm, TERMINAL_UTILITY);
        assert_eq!(enemy.terminal_benefit, 0);
        assert!(enemy.net <= -TERMINAL_UTILITY);
    }

    #[test]
    fn low_health_creates_repeated_survival_pressure() {
        assert_eq!(health_pressure_milli(100, 100), 0);
        assert_eq!(health_pressure_milli(70, 100), 0);
        assert!(health_pressure_milli(35, 100) >= 350);
        assert!(health_pressure_milli(15, 100) >= 700);
        assert_eq!(health_pressure_milli(5, 100), 1000);
    }

    #[test]
    fn hazard_damage_is_only_damage_beyond_normal_turn_decay() {
        assert_eq!(extra_hazard_damage(80, 79), 0);
        assert_eq!(extra_hazard_damage(80, 64), 15);
        assert_eq!(extra_hazard_damage(10, 0), 9);
    }

    #[test]
    fn health_pressure_and_hazard_damage_feed_survival_harm() {
        let weights = StrategicWeights {
            food: 0,
            hunting: 0,
            survival: 1000,
        };
        let facts = ActorTransitionFacts {
            health_pressure_milli: 700,
            hazard_damage: 15,
            ..ActorTransitionFacts::default()
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(score.survival_harm, 700 + 15 * HAZARD_DAMAGE_STEP);
        assert_eq!(score.net, -score.survival_harm);
    }

    #[test]
    fn border_exposure_cost_repeats_on_every_transition() {
        let weights = StrategicWeights {
            food: 0,
            hunting: 0,
            survival: 1000,
        };
        let facts = ActorTransitionFacts {
            border_exposure_milli: 700,
            ..ActorTransitionFacts::default()
        };

        let first = score_actor_transition(facts, weights);
        let second = score_actor_transition(facts, weights);

        assert_eq!(first.survival_harm, 700);
        assert_eq!(second.survival_harm, 700);
        assert_eq!(first.net, -700);
        assert_eq!(second.net, -700);
    }

    #[test]
    fn eating_food_is_a_large_food_benefit() {
        let mut graph = FutureGraph::new(state(20, vec![Coord { x: 3, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |edge| {
            edge.events.iter().any(
                |event| matches!(event, InstantEvent::AteFood { snake, .. } if snake == "ours"),
            )
        });
        let child = graph.node(edge.child);
        let score = TransitionScore::from_edge(root, edge, child);
        let ours = score
            .for_actor(actor_index(&root.state, "ours"))
            .unwrap();

        assert!(ours.food_benefit > 0);
        assert!(ours.benefit_total >= ours.food_benefit);
    }
}
