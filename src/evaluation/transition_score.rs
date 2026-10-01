#![allow(dead_code)]

use crate::search::graph::{SearchEdge, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TransitionScore {
    pub(crate) instant_benefit: i64,
    pub(crate) instant_harm: i64,
    pub(crate) structural_delta: i64,
    pub(crate) net: i64,
}

impl TransitionScore {
    pub(crate) fn from_edge(parent: &SearchNode, edge: &SearchEdge, child: &SearchNode) -> Self {
        let actor_id = &parent.state.our_snake_id;
        let parent_eval = parent
            .active_analysis()
            .and_then(|analysis| analysis.actor_evaluations.get(actor_id));
        let child_eval = child
            .active_analysis()
            .and_then(|analysis| analysis.actor_evaluations.get(actor_id));

        let structural_delta = match (parent_eval, child_eval) {
            (Some(before), Some(after)) => after.net.saturating_sub(before.net),
            _ => 0,
        };

        let mut instant_benefit = 0_i64;
        let mut instant_harm = 0_i64;

        let weights = child_eval
            .or(parent_eval)
            .map(|evaluation| evaluation.weights);

        for event in &edge.events {
            match event {
                InstantEvent::AteFood { snake, .. } if snake == actor_id => {
                    if let Some(weights) = weights {
                        let food = weighted(1000, weights.food);
                        let growth = weighted(650, weights.growth);
                        instant_benefit = instant_benefit.saturating_add(food.saturating_add(growth));
                    }
                }
                InstantEvent::EnemyForced {
                    caused_by_ours: true,
                    ..
                } => {
                    if let Some(weights) = weights {
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(350, weights.pressure));
                    }
                }
                InstantEvent::EnemyTrapped {
                    caused_by_ours: true,
                    ..
                } => {
                    if let Some(weights) = weights {
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(700, weights.hunting))
                            .saturating_add(weighted(350, weights.pressure));
                    }
                }
                InstantEvent::EnemyKilled {
                    attribution: EliminationAttribution::OurSnake,
                    ..
                } => {
                    if let Some(weights) = weights {
                        let terminal_pressure = kill_importance(parent);
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(1000, weights.hunting))
                            .saturating_add(weighted(terminal_pressure, weights.dominance));
                    }
                }
                InstantEvent::HeadToHeadWon { .. } => {
                    if let Some(weights) = weights {
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(1000, weights.hunting))
                            .saturating_add(weighted(700, weights.dominance));
                    }
                }
                InstantEvent::SelfConstrained { remaining_moves } => {
                    if let Some(weights) = weights {
                        let raw = match remaining_moves {
                            0 | 1 => 850,
                            2 => 400,
                            _ => 0,
                        };
                        instant_harm = instant_harm
                            .saturating_add(weighted(raw, weights.survival))
                            .saturating_add(weighted(raw / 2, weights.mobility));
                    }
                }
                InstantEvent::SelfDeadEnd => {
                    if let Some(weights) = weights {
                        instant_harm = instant_harm
                            .saturating_add(weighted(1000, weights.survival))
                            .saturating_add(weighted(700, weights.mobility));
                    }
                }
                InstantEvent::SelfBorderExposure { fear_milli, corner } => {
                    if let Some(weights) = weights {
                        let mut raw = i64::from(*fear_milli);
                        if *corner {
                            raw = raw.saturating_mul(135).saturating_div(100);
                        }
                        instant_harm =
                            instant_harm.saturating_add(weighted(raw.min(1000), weights.border));
                    }
                }
                InstantEvent::HeadToHeadLost { .. } | InstantEvent::Died { .. } => {
                    if let Some(weights) = weights {
                        instant_harm = instant_harm
                            .saturating_add(weighted(1000, weights.survival))
                            .saturating_add(weighted(1000, weights.mobility));
                    }
                }
                _ => {}
            }
        }

        let net = instant_benefit
            .saturating_sub(instant_harm)
            .saturating_add(structural_delta);

        Self {
            instant_benefit,
            instant_harm,
            structural_delta,
            net,
        }
    }
}

fn weighted(raw_milli: i64, weight_milli: u16) -> i64 {
    raw_milli
        .saturating_mul(i64::from(weight_milli))
        .saturating_div(1000)
}

fn kill_importance(parent: &SearchNode) -> i64 {
    let living_enemies = parent
        .state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != parent.state.our_snake_id)
        .count();

    match living_enemies {
        0 => 0,
        1 => 1000,
        2 => 800,
        3 => 650,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::analysis::{
        BorderFobicAnalysis, EnclosureAnalysis, StateAnalysis, StrategicPosture,
        TacticalStateAnalysis, TerritoryAnalysis,
    };
    use crate::decision::state_key::StateKey;
    use crate::direction::Direction;
    use crate::enemy::tracing::EnemyTracingOutput;
    use crate::evaluation::{ActorContext, ActorEvaluation, ActorMetrics};
    use crate::modes::hunting::HuntingModeOutput;
    use crate::modes::survival::SurvivalModeOutput;
    use crate::search::graph::{NodeAnalysis, SearchEdge, SearchNode};
    use crate::simulation::joint_action::JointAction;
    use crate::simulation::mobility::MobilityAnalysis;
    use crate::simulation::resolver::{ForecastDelta, InstantEvent};
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::spatial::SpatialOccupancy;
    use crate::Coord;

    fn snake(id: &str, health: i32, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(health: i32) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![Coord { x: 2, y: 1 }],
            hazards: vec![],
            snakes: vec![
                snake("ours", health, &[(1, 1), (1, 0)]),
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

    fn node(state: SimulatedGameState, net_bias: i64) -> SearchNode {
        let state_analysis = Arc::new(StateAnalysis::from_simulated(&state));
        let spatial = Arc::new(SpatialOccupancy::from_state(&state));
        let mobility = Arc::new(MobilityAnalysis::from_spatial(Arc::clone(&spatial)));
        let tracing = Arc::new(EnemyTracingOutput::default());
        let tactical = Arc::new(TacticalStateAnalysis::from_parts(
            &state,
            &tracing,
            &mobility,
        ));
        let territory = Arc::new(TerritoryAnalysis::from_spatial(&state, &spatial));
        let border = Arc::new(BorderFobicAnalysis::from_parts_with_territory(
            &state,
            &tactical,
            &territory,
        ));
        let enclosure = Arc::new(EnclosureAnalysis::from_parts(&state, &territory, &tactical));
        let posture = Arc::new(StrategicPosture::from_state(&state));

        let context = ActorContext::from_state(&state, "ours").unwrap();
        let metrics = ActorMetrics::from_parts(
            &state,
            "ours",
            &state_analysis,
            &tactical,
            &territory,
            &enclosure,
            &border,
        )
        .unwrap();
        let mut evaluation = ActorEvaluation::from_metrics(context, metrics);
        evaluation.net = evaluation.net.saturating_add(net_bias);

        let analysis = Arc::new(NodeAnalysis {
            state: state_analysis,
            mobility,
            tracing,
            tactical,
            territory,
            border,
            posture,
            enclosure,
            survival: Arc::new(SurvivalModeOutput::default()),
            hunting: Arc::new(HuntingModeOutput::default()),
            actor_evaluations: HashMap::from([("ours".to_string(), evaluation)]),
        });

        SearchNode::test_node(state, StateKey::from_state(&state), Some(analysis))
    }

    #[test]
    fn structural_delta_is_counted_once_per_edge() {
        let parent = node(state(80), 0);
        let child = node(state(80), 400);
        let edge = SearchEdge {
            joint_action: JointAction::new().with_move("ours", Direction::Right),
            events: vec![],
            forecast_delta: ForecastDelta::None,
            child: 1,
        };

        let score = TransitionScore::from_edge(&parent, &edge, &child);

        assert_eq!(score.structural_delta, 400);
        assert_eq!(score.net, 400);
    }

    #[test]
    fn food_is_an_instant_benefit_in_addition_to_state_delta() {
        let parent = node(state(20), 0);
        let child = node(state(100), 0);
        let edge = SearchEdge {
            joint_action: JointAction::new().with_move("ours", Direction::Right),
            events: vec![InstantEvent::AteFood {
                snake: "ours".to_string(),
                food: Coord { x: 2, y: 1 },
            }],
            forecast_delta: ForecastDelta::None,
            child: 1,
        };

        let score = TransitionScore::from_edge(&parent, &edge, &child);

        assert!(score.instant_benefit > 0);
        assert!(score.net > score.structural_delta);
    }

    #[test]
    fn border_exposure_is_an_instant_harm() {
        let parent = node(state(90), 0);
        let child = node(state(90), 0);
        let edge = SearchEdge {
            joint_action: JointAction::new().with_move("ours", Direction::Left),
            events: vec![InstantEvent::SelfBorderExposure {
                fear_milli: 900,
                corner: false,
            }],
            forecast_delta: ForecastDelta::None,
            child: 1,
        };

        let score = TransitionScore::from_edge(&parent, &edge, &child);

        assert!(score.instant_harm > 0);
        assert!(score.net < score.structural_delta);
    }
}
