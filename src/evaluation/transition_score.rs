#![allow(dead_code)]

use std::collections::HashMap;

use super::weights::StrategicWeights;
use crate::search::graph::{SearchEdge, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};

const FOOD_DISTANCE_STEP: i64 = 180;
const FOOD_CONSUMED: i64 = 1000;
const TERRITORY_DELTA_SCALE: i64 = 2;
const MOBILITY_STEP: i64 = 320;
const ENCLOSURE_STEP: i64 = 220;
const KILL_BENEFIT: i64 = 1400;
const DEATH_HARM: i64 = 2000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActorTransitionScore {
    pub(crate) food_benefit: i64,
    pub(crate) food_harm: i64,
    pub(crate) hunting_benefit: i64,
    pub(crate) hunting_harm: i64,
    pub(crate) survival_benefit: i64,
    pub(crate) survival_harm: i64,
    pub(crate) benefit_total: i64,
    pub(crate) harm_total: i64,
    pub(crate) net: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TransitionScore {
    pub(crate) instant_benefit: i64,
    pub(crate) instant_harm: i64,
    pub(crate) structural_delta: i64,
    pub(crate) net: i64,
    pub(crate) opponent_net_total: i64,
    pub(crate) actors: HashMap<String, ActorTransitionScore>,
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
        let mut actors = HashMap::new();

        for actor in parent.state.snakes.iter().filter(|snake| snake.alive) {
            let Some(parent_eval) = actor_evaluation(parent, &actor.id) else {
                continue;
            };
            let score =
                score_actor_transition(parent, events, child, &actor.id, parent_eval.weights);
            actors.insert(actor.id.clone(), score);
        }

        let our_id = parent.state.our_snake_id.as_str();
        let ours = actors.get(our_id).copied().unwrap_or_default();
        let opponent_net_total = actors
            .iter()
            .filter(|(actor_id, _)| actor_id.as_str() != our_id)
            .fold(0_i64, |sum, (_, score)| sum.saturating_add(score.net));

        Self {
            instant_benefit: ours.benefit_total,
            instant_harm: ours.harm_total,
            structural_delta: 0,
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

fn score_actor_transition(
    parent: &SearchNode,
    events: &[InstantEvent],
    child: &SearchNode,
    actor_id: &str,
    weights: StrategicWeights,
) -> ActorTransitionScore {
    let Some(before) = actor_evaluation(parent, actor_id) else {
        return ActorTransitionScore::default();
    };
    let after = actor_evaluation(child, actor_id);

    let ate_food = events
        .iter()
        .any(|event| matches!(event, InstantEvent::AteFood { snake, .. } if snake == actor_id));

    let (mut food_benefit, mut food_harm) = if ate_food {
        (FOOD_CONSUMED, 0)
    } else {
        food_distance_delta(
            before.metrics.best_food_distance,
            after.and_then(|evaluation| evaluation.metrics.best_food_distance),
        )
    };

    let (mut hunting_benefit, mut hunting_harm) = opponent_territory_delta(parent, child, actor_id);

    for event in events {
        if let InstantEvent::EnemyKilled {
            enemy, attribution, ..
        } = event
        {
            if enemy != actor_id
                && elimination_attributed_to(attribution, actor_id, &parent.state.our_snake_id)
            {
                hunting_benefit = hunting_benefit.saturating_add(KILL_BENEFIT);
            }
        }
    }

    let mut survival_benefit = 0_i64;
    let mut survival_harm = 0_i64;

    if let Some(after) = after {
        add_signed_delta(
            i64::from(after.metrics.territory_share_milli)
                .saturating_sub(i64::from(before.metrics.territory_share_milli))
                .saturating_mul(TERRITORY_DELTA_SCALE),
            &mut survival_benefit,
            &mut survival_harm,
        );

        add_signed_delta(
            i64::from(after.metrics.safe_non_reverse_moves)
                .saturating_sub(i64::from(before.metrics.safe_non_reverse_moves))
                .saturating_mul(MOBILITY_STEP),
            &mut survival_benefit,
            &mut survival_harm,
        );

        add_signed_delta(
            i64::from(before.metrics.border_structural_risk_milli)
                .saturating_sub(i64::from(after.metrics.border_structural_risk_milli)),
            &mut survival_benefit,
            &mut survival_harm,
        );

        add_signed_delta(
            i64::from(before.metrics.enclosure_risk)
                .saturating_sub(i64::from(after.metrics.enclosure_risk))
                .saturating_mul(ENCLOSURE_STEP),
            &mut survival_benefit,
            &mut survival_harm,
        );
    } else {
        survival_harm = survival_harm.saturating_add(DEATH_HARM);
    }

    food_benefit = weighted(food_benefit, weights.food);
    food_harm = weighted(food_harm, weights.food);
    hunting_benefit = weighted(hunting_benefit, weights.hunting);
    hunting_harm = weighted(hunting_harm, weights.hunting);
    survival_benefit = weighted(survival_benefit, weights.survival);
    survival_harm = weighted(survival_harm, weights.survival);

    let benefit_total = food_benefit
        .saturating_add(hunting_benefit)
        .saturating_add(survival_benefit);
    let harm_total = food_harm
        .saturating_add(hunting_harm)
        .saturating_add(survival_harm);

    ActorTransitionScore {
        food_benefit,
        food_harm,
        hunting_benefit,
        hunting_harm,
        survival_benefit,
        survival_harm,
        benefit_total,
        harm_total,
        net: benefit_total.saturating_sub(harm_total),
    }
}

fn actor_evaluation<'a>(
    node: &'a SearchNode,
    actor_id: &str,
) -> Option<&'a crate::evaluation::ActorSnapshot> {
    node.active_analysis()?.actor_snapshots.get(actor_id)
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

fn opponent_territory_delta(parent: &SearchNode, child: &SearchNode, actor_id: &str) -> (i64, i64) {
    let mut benefit = 0_i64;
    let mut harm = 0_i64;

    for opponent in parent
        .state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != actor_id)
    {
        let Some(before) = actor_evaluation(parent, &opponent.id) else {
            continue;
        };
        let after_share = actor_evaluation(child, &opponent.id)
            .map_or(0, |evaluation| evaluation.metrics.territory_share_milli);

        let delta = i64::from(before.metrics.territory_share_milli)
            .saturating_sub(i64::from(after_share))
            .saturating_mul(TERRITORY_DELTA_SCALE);

        if delta > 0 {
            benefit = benefit.saturating_add(delta);
        } else {
            harm = harm.saturating_add(delta.saturating_neg());
        }
    }

    (benefit, harm)
}

fn elimination_attributed_to(
    attribution: &EliminationAttribution,
    actor_id: &str,
    our_id: &str,
) -> bool {
    match attribution {
        EliminationAttribution::OurSnake => actor_id == our_id,
        EliminationAttribution::OtherSnake(killer) => killer == actor_id,
        EliminationAttribution::SelfInflicted | EliminationAttribution::Environment => false,
    }
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
        assert!(ours.net > -DEATH_HARM);
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
