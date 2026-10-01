#![allow(dead_code)]

use super::{ActorTable, StrategicWeights};
use crate::search::graph::{SearchEdge, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};

const FOOD_DISTANCE_STEP: i64 = 180;
const FOOD_CONSUMED: i64 = 1000;
const TERRITORY_DELTA_SCALE: i64 = 2;
const HUNTING_TERRITORY_BUDGET: i64 = 2000;
const MOBILITY_STEP: i64 = 320;
const ENCLOSURE_STEP: i64 = 220;
const KILL_BENEFIT: i64 = 1400;
const TERMINAL_UTILITY: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActorTransitionFacts {
    ate_food: bool,
    food_distance_before: Option<u16>,
    food_distance_after: Option<u16>,
    territory_share_delta_milli: i16,
    mobility_delta: i8,
    border_risk_improvement_milli: i16,
    enclosure_improvement: i8,
    hunting_territory_benefit: i64,
    kill_benefit: i64,
    died: bool,
    sole_survivor: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransitionFacts {
    actors: ActorTable<ActorTransitionFacts>,
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
    pub(crate) actors: ActorTable<ActorTransitionScore>,
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
        let mut actors = ActorTable::new();

        for actor in parent.state.snakes.iter().filter(|snake| snake.alive) {
            let (Some(parent_eval), Some(actor_facts)) = (
                actor_evaluation(parent, &actor.id),
                facts.actors.get(&actor.id),
            ) else {
                continue;
            };

            actors.insert(
                actor.id.clone(),
                score_actor_transition(*actor_facts, parent_eval.weights),
            );
        }

        let our_id = parent.state.our_snake_id.as_str();
        let ours = actors.get(our_id).copied().unwrap_or_default();
        let opponent_net_total = actors
            .iter()
            .filter(|(actor_id, _)| *actor_id != our_id)
            .fold(0_i64, |sum, (_, score)| sum.saturating_add(score.net));

        Self {
            instant_benefit: ours.benefit_total,
            instant_harm: ours.harm_total,
            net: ours.net,
            opponent_net_total,
            actors,
        }
    }

    pub(crate) fn for_actor(&self, actor_id: &str) -> Option<&ActorTransitionScore> {
        self.actors.get(actor_id)
    }

    pub(crate) fn route_delta(&self) -> i64 {
        self.net.saturating_sub(self.opponent_net_total)
    }
}

impl TransitionFacts {
    fn from_parts(parent: &SearchNode, events: &[InstantEvent], child: &SearchNode) -> Self {
        let hunting_transfers = territory_transfer_benefits(parent, child);
        let mut ate_food = ActorTable::<bool>::new();
        let mut kill_benefits = ActorTable::<i64>::new();

        for event in events {
            match event {
                InstantEvent::AteFood { snake, .. } => {
                    ate_food.insert(snake.clone(), true);
                }
                InstantEvent::EnemyKilled {
                    enemy, attribution, ..
                } => {
                    if let Some(killer) = attributed_actor(attribution, &parent.state.our_snake_id)
                    {
                        if killer != enemy {
                            kill_benefits.add(killer, KILL_BENEFIT);
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
        let mut actors = ActorTable::new();

        for actor in parent.state.snakes.iter().filter(|snake| snake.alive) {
            let Some(before) = actor_evaluation(parent, &actor.id) else {
                continue;
            };
            let after = actor_evaluation(child, &actor.id);
            let alive_after = child
                .state
                .snake(&actor.id)
                .is_some_and(|snake| snake.alive);

            let facts = ActorTransitionFacts {
                ate_food: ate_food.get(&actor.id).copied().unwrap_or(false),
                food_distance_before: before.metrics.best_food_distance,
                food_distance_after: after.and_then(|snapshot| snapshot.metrics.best_food_distance),
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
                enclosure_improvement: after.map_or(0, |snapshot| {
                    signed_i8(
                        i16::from(before.metrics.enclosure_risk)
                            .saturating_sub(i16::from(snapshot.metrics.enclosure_risk)),
                    )
                }),
                hunting_territory_benefit: hunting_transfers.get(&actor.id).copied().unwrap_or(0),
                kill_benefit: kill_benefits.get(&actor.id).copied().unwrap_or(0),
                died: !alive_after,
                sole_survivor: alive_after && living_after == 1,
            };
            actors.insert(actor.id.clone(), facts);
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
        food_distance_delta(facts.food_distance_before, facts.food_distance_after)
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

fn food_distance_delta(before: Option<u16>, after: Option<u16>) -> (i64, i64) {
    match (before, after) {
        (Some(before), Some(after)) if after < before => (
            i64::from(before.saturating_sub(after)).saturating_mul(FOOD_DISTANCE_STEP),
            0,
        ),
        (Some(before), Some(after)) if after > before => (
            0,
            i64::from(after.saturating_sub(before)).saturating_mul(FOOD_DISTANCE_STEP),
        ),
        (None, Some(_)) => (FOOD_DISTANCE_STEP / 2, 0),
        (Some(_), None) => (0, FOOD_DISTANCE_STEP / 2),
        _ => (0, 0),
    }
}

fn territory_transfer_benefits(parent: &SearchNode, child: &SearchNode) -> ActorTable<i64> {
    let Some(parent_territory) = parent.active_analysis().map(|analysis| &analysis.territory)
    else {
        return ActorTable::new();
    };
    let Some(child_territory) = child.active_analysis().map(|analysis| &analysis.territory) else {
        return ActorTable::new();
    };

    let board_cells = parent
        .state
        .width
        .saturating_mul(parent.state.height)
        .max(1);
    let cell_value = HUNTING_TERRITORY_BUDGET
        .saturating_div(i64::from(board_cells))
        .max(1);
    let mut benefits = ActorTable::<i64>::new();

    for y in 0..parent.state.height {
        for x in 0..parent.state.width {
            let coord = crate::Coord {
                x: i32::try_from(x).unwrap_or(i32::MAX),
                y: i32::try_from(y).unwrap_or(i32::MAX),
            };
            let before = parent_territory.competitive_owner_at(coord);
            let after = child_territory.competitive_owner_at(coord);

            let (Some(before), Some(after)) = (before, after) else {
                continue;
            };
            if before == after {
                continue;
            }

            benefits.add(after, cell_value);
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

    #[test]
    fn hunting_credits_only_actor_receiving_enemy_owned_cells() {
        let parent_graph =
            FutureGraph::new(territory_state(Coord { x: 1, y: 3 }, Coord { x: 5, y: 3 }));
        let child_graph =
            FutureGraph::new(territory_state(Coord { x: 2, y: 3 }, Coord { x: 5, y: 5 }));

        let benefits = territory_transfer_benefits(
            parent_graph.node(parent_graph.root()),
            child_graph.node(child_graph.root()),
        );

        assert!(benefits.get("ours").copied().unwrap_or(0) > 0);
    }

    #[test]
    fn transition_scores_every_living_actor() {
        let mut graph = FutureGraph::new(state(80, vec![Coord { x: 4, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |_| true);
        let child = graph.node(edge.child);
        let score = TransitionScore::from_edge(root, edge, child);

        assert!(score.for_actor("ours").is_some());
        assert!(score.for_actor("enemy").is_some());
        assert_eq!(
            score.opponent_net_total,
            score.for_actor("enemy").unwrap().net
        );
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
        let ours = score.for_actor("ours").unwrap();

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

        let ours = score.for_actor("ours").unwrap();
        let enemy = score.for_actor("enemy").unwrap();

        assert_eq!(ours.terminal_benefit, TERMINAL_UTILITY);
        assert_eq!(ours.terminal_harm, 0);
        assert!(ours.net >= TERMINAL_UTILITY);

        assert_eq!(enemy.terminal_harm, TERMINAL_UTILITY);
        assert_eq!(enemy.terminal_benefit, 0);
        assert!(enemy.net <= -TERMINAL_UTILITY);
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
        let ours = score.for_actor("ours").unwrap();

        assert!(ours.food_benefit > 0);
        assert!(ours.benefit_total >= ours.food_benefit);
    }
}
