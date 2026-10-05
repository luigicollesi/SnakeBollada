#![allow(dead_code)]

use super::{ActorVec, StrategicWeights};
use crate::search::graph::SearchNode;
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};
use crate::simulation::state::ActorIndex;

const FOOD_POTENTIAL_DELTA_SCALE: i64 = 3;
const FOOD_POTENTIAL_DEADBAND: u16 = 20;
const SPACE_CAPACITY_DEADBAND: i16 = 20;
const TERRITORY_CONTROL_DEADBAND: i16 = 15;
const BORDER_RISK_DEADBAND: i16 = 20;
const FOOD_CONSUMED: i64 = 1000;
const FOOD_ETA_STEP: i64 = 220;
const SIZE_SECURITY_DELTA_SCALE: i64 = 3;
const GROWTH_PRESSURE_DELTA_SCALE: i64 = 3;
const GROWTH_STALL_THRESHOLD_MILLI: u16 = 750;
const GROWTH_CONSUMPTION_BONUS_SCALE: i64 = 2;
const GROWTH_DETOUR_DIVISOR: i64 = 2;
const GROWTH_STALL_DIVISOR: i64 = 8;
const SPACE_CAPACITY_DELTA_SCALE: i64 = 2;
const TERRITORY_CONTROL_DELTA_SCALE: i64 = 4;
const HUNTING_TERRITORY_BUDGET: i64 = 2000;
const HUNTING_DENIAL_NUMERATOR: i64 = 1;
const HUNTING_DENIAL_DENOMINATOR: i64 = 2;
const MOBILITY_STEP: i64 = 320;
const ENCLOSURE_STEP: i64 = 220;
const BORDER_EXPOSURE_STEP: i64 = 1;
const BORDER_PIN_RISK_STEP: i64 = 1;
const BORDER_ESCAPE_PRESSURE_STEP: i64 = 1;
const FOOD_SURVIVAL_PRESSURE_STEP: i64 = 1;
const HEALTH_PRESSURE_STEP: i64 = 1;
const HAZARD_DAMAGE_STEP: i64 = 35;
const FORCED_CORRIDOR_ENTRY_HARM: i64 = 0;
const NO_SAFE_MOVE_HARM: i64 = 50_000;
const KILL_BENEFIT: i64 = 1400;
const TERMINAL_UTILITY: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActorTransitionFacts {
    ate_food: bool,
    consumed_food: Option<crate::Coord>,
    food_consumption_factor_milli: u16,
    food_potential_before: u16,
    food_potential_after: u16,
    claimable_food_eta_before: Option<u16>,
    claimable_food_eta_after: Option<u16>,
    size_security_before: u16,
    size_security_after: u16,
    growth_pressure_before: u16,
    growth_pressure_after: u16,
    space_capacity_delta_milli: i16,
    territory_control_delta_milli: i16,
    survival_weight_after: u16,
    mobility_delta: i8,
    entered_forced_corridor: bool,
    no_safe_moves_after: bool,
    border_risk_improvement_milli: i16,
    border_exposure_milli: u16,
    border_pin_risk_milli: u16,
    border_escape_pressure_milli: u16,
    food_survival_pressure_milli: u16,
    health_pressure_milli: u16,
    hazard_damage: u16,
    enclosure_improvement: i8,
    hunting_territory_benefit: i64,
    kill_benefit: i64,
    attributed_kill: bool,
    died: bool,
    sole_survivor: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransitionFacts {
    actors: ActorVec<ActorTransitionFacts>,
    our_elimination_attribution: Option<EliminationAttribution>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActorTransitionScore {
    pub(crate) food_benefit: i64,
    pub(crate) food_harm: i64,
    pub(crate) hunting_benefit: i64,
    pub(crate) hunting_harm: i64,
    pub(crate) raw_hunting_milli: u16,
    pub(crate) attributed_kill: bool,
    pub(crate) survival_benefit: i64,
    pub(crate) survival_harm: i64,
    pub(crate) terminal_benefit: i64,
    pub(crate) terminal_harm: i64,
    pub(crate) benefit_total: i64,
    pub(crate) harm_total: i64,
    pub(crate) net: i64,
    pub(crate) actor_terminal: i64,
    pub(crate) actor_choice_net: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TransitionScore {
    pub(crate) instant_benefit: i64,
    pub(crate) instant_harm: i64,
    pub(crate) opponent_benefit_total: i64,
    pub(crate) opponent_harm_total: i64,
    pub(crate) effective_benefit: i64,
    pub(crate) effective_harm: i64,
    pub(crate) net: i64,
    pub(crate) opponent_net_total: i64,
    pub(crate) actors: ActorVec<ActorTransitionScore>,
    pub(crate) our_elimination_attribution: Option<EliminationAttribution>,
}

impl TransitionScore {
    pub(crate) fn from_parts(
        parent: &SearchNode,
        events: &[InstantEvent],
        child: &SearchNode,
    ) -> Self {
        let facts = TransitionFacts::from_parts(parent, events, child);
        let mut actors = ActorVec::with_capacity(parent.state.snakes.len());

        for (index, _) in parent
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
                actor_evaluation(parent, actor_index),
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
        let (opponent_benefit_total, opponent_harm_total, opponent_net_total) = actors
            .iter()
            .filter(|(actor, _)| Some(*actor) != our_index)
            .fold((0_i64, 0_i64, 0_i64), |(benefit, harm, net), (_, score)| {
                (
                    benefit.saturating_add(score.benefit_total),
                    harm.saturating_add(score.harm_total),
                    net.saturating_add(score.net),
                )
            });
        let effective_benefit = ours.benefit_total.saturating_add(opponent_harm_total);
        let effective_harm = ours.harm_total.saturating_add(opponent_benefit_total);

        Self {
            instant_benefit: ours.benefit_total,
            instant_harm: ours.harm_total,
            opponent_benefit_total,
            opponent_harm_total,
            effective_benefit,
            effective_harm,
            net: ours.net,
            opponent_net_total,
            actors,
            our_elimination_attribution: facts.our_elimination_attribution,
        }
    }

    pub(crate) fn for_actor(&self, actor: ActorIndex) -> Option<&ActorTransitionScore> {
        self.actors.get(actor)
    }

    pub(crate) fn route_delta(&self) -> i64 {
        self.effective_benefit.saturating_sub(self.effective_harm)
    }
}

impl TransitionFacts {
    fn from_parts(parent: &SearchNode, events: &[InstantEvent], child: &SearchNode) -> Self {
        let hunting_transfers = hunting_territory_benefits(parent, child);
        let mut ate_food = ActorVec::<crate::Coord>::with_capacity(parent.state.snakes.len());
        let mut kill_benefits = ActorVec::<i64>::with_capacity(parent.state.snakes.len());
        let our_elimination_attribution = events.iter().find_map(|event| match event {
            InstantEvent::Died { attribution, .. } => Some(*attribution),
            _ => None,
        });

        for event in events {
            match event {
                InstantEvent::AteFood { actor, food } => {
                    ate_food.insert(*actor, *food);
                }
                InstantEvent::EnemyKilled {
                    enemy, attribution, ..
                } => {
                    if let Some(killer) = attributed_actor(attribution) {
                        if killer != *enemy {
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
            let Some(before) = actor_evaluation(parent, actor_index) else {
                continue;
            };
            let after = actor_evaluation(child, actor_index);
            let child_actor = child.state.snake_at(actor_index);
            let alive_after = child_actor.is_some_and(|snake| snake.alive);
            let consumed_food = ate_food.get(actor_index).copied();
            let ate_food_now = consumed_food.is_some();
            let health_pressure_milli = after
                .map(|snapshot| snapshot.metrics.health_pressure_milli)
                .unwrap_or(0);
            let hazard_damage = if ate_food_now || !alive_after {
                0
            } else {
                child_actor.map_or(0, |snake| extra_hazard_damage(actor.health, snake.health))
            };
            let kill_benefit = kill_benefits.get(actor_index).copied().unwrap_or(0);
            let attributed_kill = kill_benefit > 0
                || matches!(
                    our_elimination_attribution,
                    Some(EliminationAttribution::Actor(killer)) if killer == actor_index
                );

            let facts = ActorTransitionFacts {
                ate_food: ate_food_now,
                consumed_food,
                food_consumption_factor_milli: consumed_food.map_or(1000, |food| {
                    super::metrics::food_border_attraction_milli(&parent.state, actor_index, food)
                }),
                food_potential_before: consumed_food.map_or(
                    before.metrics.food_potential_milli,
                    |food| {
                        parent.active_analysis().map_or(
                            before.metrics.food_potential_milli,
                            |analysis| {
                                super::metrics::food_potential_milli_excluding(
                                    &parent.state,
                                    &analysis.territory,
                                    actor_index,
                                    Some(food),
                                )
                            },
                        )
                    },
                ),
                food_potential_after: after
                    .map(|snapshot| snapshot.metrics.food_potential_milli)
                    .unwrap_or(0),
                claimable_food_eta_before: before.metrics.claimable_food_eta,
                claimable_food_eta_after: after
                    .and_then(|snapshot| snapshot.metrics.claimable_food_eta),
                size_security_before: before.metrics.size_security_milli,
                size_security_after: after
                    .map(|snapshot| snapshot.metrics.size_security_milli)
                    .unwrap_or(0),
                growth_pressure_before: before.metrics.growth_pressure_milli,
                growth_pressure_after: after
                    .map(|snapshot| snapshot.metrics.growth_pressure_milli)
                    .unwrap_or(0),
                space_capacity_delta_milli: after.map_or(0, |snapshot| {
                    signed_i16(
                        i32::from(snapshot.metrics.space_capacity_milli)
                            .saturating_sub(i32::from(before.metrics.space_capacity_milli)),
                    )
                }),
                territory_control_delta_milli: after.map_or(0, |snapshot| {
                    signed_i16(
                        i32::from(snapshot.metrics.territory_control_milli)
                            .saturating_sub(i32::from(before.metrics.territory_control_milli)),
                    )
                }),
                survival_weight_after: after
                    .map(|snapshot| snapshot.weights.survival)
                    .unwrap_or(before.weights.survival),
                mobility_delta: after.map_or(0, |snapshot| {
                    signed_i8(
                        i16::from(snapshot.metrics.safe_non_reverse_moves)
                            .saturating_sub(i16::from(before.metrics.safe_non_reverse_moves)),
                    )
                }),
                entered_forced_corridor: alive_after
                    && before.metrics.safe_non_reverse_moves >= 2
                    && after.is_some_and(|snapshot| snapshot.metrics.safe_non_reverse_moves == 1),
                no_safe_moves_after: alive_after
                    && after.is_some_and(|snapshot| snapshot.metrics.safe_non_reverse_moves == 0),
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
                border_pin_risk_milli: after
                    .map(|snapshot| snapshot.metrics.border_pin_risk_milli)
                    .unwrap_or(0),
                border_escape_pressure_milli: after
                    .map(|snapshot| snapshot.metrics.border_escape_pressure_milli)
                    .unwrap_or(0),
                food_survival_pressure_milli: after
                    .map(|snapshot| snapshot.metrics.food_survival_pressure_milli)
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
                kill_benefit,
                attributed_kill,
                died: !alive_after,
                sole_survivor: alive_after && living_after == 1,
            };
            actors.insert(actor_index, facts);
        }

        Self {
            actors,
            our_elimination_attribution,
        }
    }
}

fn score_actor_transition(
    facts: ActorTransitionFacts,
    weights: StrategicWeights,
) -> ActorTransitionScore {
    let (mut food_benefit, mut food_harm) =
        food_potential_delta(facts.food_potential_before, facts.food_potential_after);
    let growth_consumption_urgency = if facts.ate_food {
        i64::from(facts.growth_pressure_before)
            .saturating_mul(GROWTH_CONSUMPTION_BONUS_SCALE)
            .saturating_mul(i64::from(facts.food_consumption_factor_milli))
            .saturating_div(1000)
    } else {
        0
    };
    if facts.ate_food {
        food_benefit = food_benefit.saturating_add(
            FOOD_CONSUMED
                .saturating_mul(i64::from(facts.food_consumption_factor_milli))
                .saturating_div(1000),
        );
    }

    let (eta_benefit, eta_harm) = food_eta_delta(
        facts.claimable_food_eta_before,
        facts.claimable_food_eta_after,
        facts.ate_food,
    );
    food_benefit = food_benefit.saturating_add(eta_benefit);
    food_harm = food_harm.saturating_add(eta_harm);

    add_signed_delta(
        i64::from(facts.size_security_after)
            .saturating_sub(i64::from(facts.size_security_before))
            .saturating_mul(SIZE_SECURITY_DELTA_SCALE),
        &mut food_benefit,
        &mut food_harm,
    );
    if !facts.died {
        add_signed_delta(
            i64::from(facts.growth_pressure_before)
                .saturating_sub(i64::from(facts.growth_pressure_after))
                .saturating_mul(GROWTH_PRESSURE_DELTA_SCALE),
            &mut food_benefit,
            &mut food_harm,
        );

        if facts.growth_pressure_after >= GROWTH_STALL_THRESHOLD_MILLI
            && !made_growth_progress(&facts)
        {
            let divisor = if food_eta_worsened(&facts) {
                GROWTH_DETOUR_DIVISOR
            } else {
                GROWTH_STALL_DIVISOR
            };
            food_harm = food_harm
                .saturating_add(i64::from(facts.growth_pressure_after).saturating_div(divisor));
        }
    }

    let raw_hunting_milli = facts
        .hunting_territory_benefit
        .saturating_add(if facts.attributed_kill {
            KILL_BENEFIT
        } else {
            0
        })
        .clamp(0, 1000)
        .try_into()
        .unwrap_or(1000);
    let mut hunting_benefit = facts
        .hunting_territory_benefit
        .saturating_add(facts.kill_benefit);
    let mut hunting_harm = 0_i64;
    let (space_benefit, space_harm) = weighted_survival_delta(
        deadband_i16(facts.space_capacity_delta_milli, SPACE_CAPACITY_DEADBAND),
        SPACE_CAPACITY_DELTA_SCALE,
        weights.survival,
        facts.survival_weight_after,
    );
    let (territory_benefit, territory_harm) = weighted_survival_delta(
        deadband_i16(
            facts.territory_control_delta_milli,
            TERRITORY_CONTROL_DEADBAND,
        ),
        TERRITORY_CONTROL_DELTA_SCALE,
        weights.survival,
        facts.survival_weight_after,
    );
    let mut survival_benefit = 0_i64;
    let mut survival_harm = 0_i64;

    add_signed_delta(
        i64::from(facts.mobility_delta).saturating_mul(MOBILITY_STEP),
        &mut survival_benefit,
        &mut survival_harm,
    );
    add_signed_delta(
        i64::from(deadband_i16(
            facts.border_risk_improvement_milli,
            BORDER_RISK_DEADBAND,
        )),
        &mut survival_benefit,
        &mut survival_harm,
    );
    survival_harm = survival_harm.saturating_add(
        i64::from(facts.border_exposure_milli).saturating_mul(BORDER_EXPOSURE_STEP),
    );
    survival_harm = survival_harm.saturating_add(
        i64::from(facts.border_pin_risk_milli).saturating_mul(BORDER_PIN_RISK_STEP),
    );
    survival_harm = survival_harm.saturating_add(
        i64::from(facts.food_survival_pressure_milli).saturating_mul(FOOD_SURVIVAL_PRESSURE_STEP),
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

    food_benefit = weighted(food_benefit, weights.food).saturating_add(growth_consumption_urgency);
    food_harm = weighted(food_harm, weights.food);
    hunting_benefit = weighted(hunting_benefit, weights.hunting);
    hunting_harm = weighted(hunting_harm, weights.hunting);
    survival_benefit = weighted(survival_benefit, weights.survival)
        .saturating_add(space_benefit)
        .saturating_add(territory_benefit);
    survival_harm = weighted(survival_harm, weights.survival)
        .saturating_add(space_harm)
        .saturating_add(territory_harm)
        .saturating_add(
            i64::from(facts.border_escape_pressure_milli)
                .saturating_mul(BORDER_ESCAPE_PRESSURE_STEP),
        );
    if facts.entered_forced_corridor && !facts.died {
        survival_harm = survival_harm.saturating_add(FORCED_CORRIDOR_ENTRY_HARM);
    }
    if facts.no_safe_moves_after && !facts.died {
        survival_harm = survival_harm.saturating_add(NO_SAFE_MOVE_HARM);
    }

    let terminal_benefit = if facts.sole_survivor {
        TERMINAL_UTILITY
    } else {
        0
    };
    let terminal_harm = if facts.died { TERMINAL_UTILITY } else { 0 };

    let benefit_total = food_benefit
        .saturating_add(hunting_benefit)
        .saturating_add(survival_benefit);
    let harm_total = food_harm
        .saturating_add(hunting_harm)
        .saturating_add(survival_harm);
    let net = benefit_total.saturating_sub(harm_total);
    let actor_terminal = terminal_benefit.saturating_sub(terminal_harm);
    let actor_choice_net = if actor_terminal != 0 {
        actor_terminal
    } else {
        net
    };

    ActorTransitionScore {
        food_benefit,
        food_harm,
        hunting_benefit,
        hunting_harm,
        raw_hunting_milli,
        attributed_kill: facts.attributed_kill,
        survival_benefit,
        survival_harm,
        terminal_benefit,
        terminal_harm,
        benefit_total,
        harm_total,
        net,
        actor_terminal,
        actor_choice_net,
    }
}

fn actor_evaluation(
    node: &SearchNode,
    actor: ActorIndex,
) -> Option<&crate::evaluation::ActorSnapshot> {
    node.active_analysis()?.actor_snapshot(actor)
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
    let raw = i32::from(after).saturating_sub(i32::from(before));
    let delta = deadband_i32(raw, i32::from(FOOD_POTENTIAL_DEADBAND));

    if delta > 0 {
        (
            i64::from(delta).saturating_mul(FOOD_POTENTIAL_DELTA_SCALE),
            0,
        )
    } else if delta < 0 {
        (
            0,
            i64::from(delta.saturating_neg()).saturating_mul(FOOD_POTENTIAL_DELTA_SCALE),
        )
    } else {
        (0, 0)
    }
}

fn food_eta_worsened(facts: &ActorTransitionFacts) -> bool {
    match (
        facts.claimable_food_eta_before,
        facts.claimable_food_eta_after,
    ) {
        (Some(before), Some(after)) => after > before,
        (Some(_), None) => true,
        _ => false,
    }
}

fn made_growth_progress(facts: &ActorTransitionFacts) -> bool {
    if facts.ate_food || facts.growth_pressure_after < facts.growth_pressure_before {
        return true;
    }

    let eta_improved = match (
        facts.claimable_food_eta_before,
        facts.claimable_food_eta_after,
    ) {
        (Some(before), Some(after)) => after < before,
        (None, Some(_)) => true,
        _ => false,
    };
    if eta_improved {
        return true;
    }

    facts.food_potential_after
        > facts
            .food_potential_before
            .saturating_add(FOOD_POTENTIAL_DEADBAND)
}

fn food_eta_delta(before: Option<u16>, after: Option<u16>, ate_food: bool) -> (i64, i64) {
    if ate_food {
        return (0, 0);
    }

    match (before, after) {
        (Some(before), Some(after)) if after < before => (
            i64::from(before.saturating_sub(after)).saturating_mul(FOOD_ETA_STEP),
            0,
        ),
        (Some(before), Some(after)) if after > before => (
            0,
            i64::from(after.saturating_sub(before)).saturating_mul(FOOD_ETA_STEP),
        ),
        (Some(_), None) => (0, FOOD_ETA_STEP),
        (None, Some(_)) => (FOOD_ETA_STEP.saturating_div(2), 0),
        _ => (0, 0),
    }
}

fn deadband_i16(value: i16, deadband: i16) -> i16 {
    if value > deadband {
        value.saturating_sub(deadband)
    } else if value < deadband.saturating_neg() {
        value.saturating_add(deadband)
    } else {
        0
    }
}

fn deadband_i32(value: i32, deadband: i32) -> i32 {
    if value > deadband {
        value.saturating_sub(deadband)
    } else if value < deadband.saturating_neg() {
        value.saturating_add(deadband)
    } else {
        0
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

    let transition = parent_territory.competitive_transition_to(child_territory);
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

    for (actor, captures) in transition.captures.iter() {
        benefits.add(actor, i64::from(*captures).saturating_mul(cell_value));
    }
    for (actor, denials) in transition.denials.iter() {
        benefits.add(actor, i64::from(*denials).saturating_mul(denial_value));
    }

    benefits
}

fn attributed_actor(attribution: &EliminationAttribution) -> Option<ActorIndex> {
    match attribution {
        EliminationAttribution::Actor(actor) => Some(*actor),
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

fn weighted_survival_delta(
    delta_milli: i16,
    scale: i64,
    current_weight: u16,
    next_weight: u16,
) -> (i64, i64) {
    let mut benefit = 0_i64;
    let mut harm = 0_i64;
    add_signed_delta(
        i64::from(delta_milli).saturating_mul(scale),
        &mut benefit,
        &mut harm,
    );
    let weight = if delta_milli < 0 {
        current_weight.max(next_weight)
    } else {
        current_weight
    };
    (weighted(benefit, weight), weighted(harm, weight))
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
    use crate::search::graph::{FutureGraph, SearchEdge};
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::Coord;

    use super::*;

    #[test]
    fn tiny_strategic_deltas_are_absorbed_by_deadbands() {
        assert_eq!(deadband_i16(20, SPACE_CAPACITY_DEADBAND), 0);
        assert_eq!(deadband_i16(-15, TERRITORY_CONTROL_DEADBAND), 0);
        assert_eq!(deadband_i16(20, BORDER_RISK_DEADBAND), 0);
        assert_eq!(food_potential_delta(500, 520), (0, 0));
    }

    #[test]
    fn meaningful_deltas_keep_only_signal_beyond_deadband() {
        assert_eq!(deadband_i16(50, SPACE_CAPACITY_DEADBAND), 30);
        assert_eq!(deadband_i16(-35, TERRITORY_CONTROL_DEADBAND), -20);
        assert_eq!(
            food_potential_delta(500, 550),
            (30 * FOOD_POTENTIAL_DELTA_SCALE, 0)
        );
    }

    #[test]
    fn food_eta_progress_and_size_security_reward_growth() {
        let facts = ActorTransitionFacts {
            claimable_food_eta_before: Some(2),
            claimable_food_eta_after: Some(1),
            size_security_before: 0,
            size_security_after: 300,
            growth_pressure_before: 850,
            growth_pressure_after: 850,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 50,
            survival: 250,
        };

        let score = score_actor_transition(facts, weights);

        assert!(score.food_benefit > score.food_harm);
        assert!(score.food_benefit >= 700);
    }

    #[test]
    fn moving_away_from_claimable_food_while_undersized_is_costly() {
        let facts = ActorTransitionFacts {
            claimable_food_eta_before: Some(1),
            claimable_food_eta_after: Some(2),
            size_security_before: 0,
            size_security_after: 0,
            growth_pressure_before: 1000,
            growth_pressure_after: 1000,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 50,
            survival: 250,
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(score.food_benefit, 0);
        assert!(score.food_harm >= 500);
    }

    #[test]
    fn growth_consumption_urgency_is_not_erased_by_low_food_weight() {
        let facts = ActorTransitionFacts {
            ate_food: true,
            food_consumption_factor_milli: 1000,
            growth_pressure_before: 850,
            growth_pressure_after: 650,
            ..ActorTransitionFacts::default()
        };
        let low_food_weight = StrategicWeights {
            food: 200,
            hunting: 50,
            survival: 750,
        };

        let score = score_actor_transition(facts, low_food_weight);

        assert!(score.food_benefit >= 1700);
        assert_eq!(score.food_harm, 0);
    }

    #[test]
    fn eating_food_scales_with_growth_urgency_without_recurring_debt() {
        let urgent = ActorTransitionFacts {
            ate_food: true,
            food_consumption_factor_milli: 1000,
            growth_pressure_before: 1000,
            growth_pressure_after: 1000,
            ..ActorTransitionFacts::default()
        };
        let comfortable = ActorTransitionFacts {
            ate_food: true,
            food_consumption_factor_milli: 1000,
            growth_pressure_before: 0,
            growth_pressure_after: 0,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 50,
            survival: 250,
        };

        let urgent_score = score_actor_transition(urgent, weights);
        let comfortable_score = score_actor_transition(comfortable, weights);

        assert!(urgent_score.food_benefit > comfortable_score.food_benefit);
        assert_eq!(urgent_score.food_harm, 0);
    }

    #[test]
    fn steady_growth_pressure_is_not_recharged_when_food_eta_improves() {
        let facts = ActorTransitionFacts {
            claimable_food_eta_before: Some(3),
            claimable_food_eta_after: Some(2),
            growth_pressure_before: 1000,
            growth_pressure_after: 1000,
            food_potential_before: 300,
            food_potential_after: 300,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 50,
            survival: 250,
        };

        let score = score_actor_transition(facts, weights);

        assert!(score.food_benefit > 0);
        assert_eq!(score.food_harm, 0);
    }

    #[test]
    fn severe_growth_pressure_penalizes_stalling_without_food_progress() {
        let facts = ActorTransitionFacts {
            claimable_food_eta_before: Some(3),
            claimable_food_eta_after: Some(3),
            growth_pressure_before: 1000,
            growth_pressure_after: 1000,
            food_potential_before: 300,
            food_potential_after: 300,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 50,
            survival: 250,
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(score.food_benefit, 0);
        assert!(score.food_harm > 0);
        assert!(score.food_harm < 200);
    }

    #[test]
    fn reducing_growth_pressure_is_rewarded_instead_of_recharged() {
        let facts = ActorTransitionFacts {
            growth_pressure_before: 1000,
            growth_pressure_after: 850,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 50,
            survival: 250,
        };

        let score = score_actor_transition(facts, weights);

        assert!(score.food_benefit >= 300);
        assert_eq!(score.food_harm, 0);
    }

    #[test]
    fn border_escape_pressure_is_direct_harm_even_when_survival_weight_is_low() {
        let facts = ActorTransitionFacts {
            border_escape_pressure_milli: 900,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 700,
            hunting: 150,
            survival: 150,
        };

        let score = score_actor_transition(facts, weights);

        assert!(score.survival_harm >= 900);
    }

    #[test]
    fn raw_hunting_signal_is_independent_from_strategic_weight() {
        let facts = ActorTransitionFacts {
            hunting_territory_benefit: 700,
            ..ActorTransitionFacts::default()
        };
        let low_hunting = StrategicWeights {
            food: 450,
            hunting: 100,
            survival: 450,
        };
        let high_hunting = StrategicWeights {
            food: 100,
            hunting: 800,
            survival: 100,
        };

        let low = score_actor_transition(facts, low_hunting);
        let high = score_actor_transition(facts, high_hunting);

        assert_eq!(low.raw_hunting_milli, high.raw_hunting_milli);
        assert_eq!(low.raw_hunting_milli, 700);
        assert!(high.hunting_benefit > low.hunting_benefit);
    }

    #[test]
    fn our_death_attributes_intent_kill_only_to_causal_enemy() {
        let parent_state = SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes: vec![
                snake("ours", 100, &[(2, 2), (2, 1)]),
                snake("enemy-a", 100, &[(3, 2), (3, 1)]),
                snake("enemy-b", 100, &[(5, 5), (5, 4)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        };
        let mut child_state = parent_state.clone();
        child_state.turn = 2;
        child_state.snake_mut("ours").unwrap().alive = false;

        let parent_graph = FutureGraph::new(parent_state);
        let child_graph = FutureGraph::new(child_state);
        let parent = parent_graph.node(parent_graph.root());
        let child = child_graph.node(child_graph.root());
        let enemy_a = actor_index(&parent.state, "enemy-a");
        let enemy_b = actor_index(&parent.state, "enemy-b");
        let events = vec![InstantEvent::Died {
            cause: crate::simulation::resolver::EliminationCause::BodyCollision,
            attribution: EliminationAttribution::Actor(enemy_a),
        }];

        let score = TransitionScore::from_parts(parent, &events, child);

        assert_eq!(
            score.our_elimination_attribution,
            Some(EliminationAttribution::Actor(enemy_a))
        );
        assert!(score.for_actor(enemy_a).unwrap().attributed_kill);
        assert!(!score.for_actor(enemy_b).unwrap().attributed_kill);
        assert_eq!(score.for_actor(enemy_a).unwrap().raw_hunting_milli, 1000);
    }

    #[test]
    fn space_capacity_collapse_is_survival_harm_and_uses_child_pressure() {
        let facts = ActorTransitionFacts {
            space_capacity_delta_milli: -100,
            survival_weight_after: 900,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 275,
            hunting: 275,
            survival: 450,
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(score.survival_benefit, 0);
        assert_eq!(
            score.survival_harm,
            i64::from(100 - SPACE_CAPACITY_DEADBAND)
                .saturating_mul(SPACE_CAPACITY_DELTA_SCALE)
                .saturating_mul(900)
                .saturating_div(1000)
        );
    }

    #[test]
    fn space_capacity_recovery_is_strongest_when_currently_constrained() {
        let facts = ActorTransitionFacts {
            space_capacity_delta_milli: 100,
            survival_weight_after: 450,
            ..ActorTransitionFacts::default()
        };
        let constrained = StrategicWeights {
            food: 125,
            hunting: 125,
            survival: 750,
        };
        let comfortable = StrategicWeights {
            food: 425,
            hunting: 425,
            survival: 150,
        };

        let constrained_score = score_actor_transition(facts, constrained);
        let comfortable_score = score_actor_transition(facts, comfortable);

        assert!(constrained_score.survival_benefit > comfortable_score.survival_benefit);
        assert_eq!(constrained_score.survival_harm, 0);
    }

    #[test]
    fn territory_control_collapse_uses_the_higher_child_survival_pressure_immediately() {
        let facts = ActorTransitionFacts {
            territory_control_delta_milli: -50,
            survival_weight_after: 750,
            ..ActorTransitionFacts::default()
        };
        let weights = StrategicWeights {
            food: 275,
            hunting: 275,
            survival: 450,
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(score.survival_benefit, 0);
        assert_eq!(
            score.survival_harm,
            i64::from(50 - TERRITORY_CONTROL_DEADBAND)
                .saturating_mul(TERRITORY_CONTROL_DELTA_SCALE)
                .saturating_mul(750)
                .saturating_div(1000)
        );
    }

    #[test]
    fn territory_control_recovery_is_rewarded_by_the_current_high_survival_pressure() {
        let facts = ActorTransitionFacts {
            territory_control_delta_milli: 80,
            survival_weight_after: 600,
            ..ActorTransitionFacts::default()
        };
        let constrained = StrategicWeights {
            food: 125,
            hunting: 125,
            survival: 750,
        };
        let comfortable = StrategicWeights {
            food: 425,
            hunting: 425,
            survival: 150,
        };

        let constrained_score = score_actor_transition(facts, constrained);
        let comfortable_score = score_actor_transition(facts, comfortable);

        assert!(constrained_score.survival_benefit > comfortable_score.survival_benefit);
        assert_eq!(constrained_score.survival_harm, 0);
    }

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
        }
    }

    fn first_edge_for(
        graph: &FutureGraph,
        direction: Direction,
        predicate: impl Fn(&SearchEdge) -> bool,
    ) -> &SearchEdge {
        let root = graph.node(graph.root());
        let our_actor = root.state.actor_index("ours").expect("ours must exist");
        root.children
            .iter()
            .find(|edge| {
                edge.joint_action.direction_for(our_actor) == Some(direction) && predicate(edge)
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
        }
    }

    fn actor_index(state: &SimulatedGameState, actor_id: &str) -> ActorIndex {
        state.actor_index(actor_id).expect("actor must exist")
    }

    #[test]
    fn hunting_transition_uses_only_shared_open_competitive_cells() {
        let parent_graph =
            FutureGraph::new(territory_state(Coord { x: 1, y: 3 }, Coord { x: 5, y: 3 }));
        let child_graph =
            FutureGraph::new(territory_state(Coord { x: 2, y: 3 }, Coord { x: 5, y: 5 }));

        let parent = parent_graph.node(parent_graph.root());
        let child = child_graph.node(child_graph.root());
        let parent_territory = &parent.active_analysis().unwrap().territory;
        let child_territory = &child.active_analysis().unwrap().territory;
        let delta = parent_territory.competitive_transition_to(child_territory);
        let ours = actor_index(&parent.state, "ours");

        assert_eq!(
            hunting_territory_benefits(parent, child)
                .get(ours)
                .copied()
                .unwrap_or(0)
                > 0,
            delta.captures.get(ours).copied().unwrap_or(0) > 0
                || delta.denials.get(ours).copied().unwrap_or(0) > 0
        );
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
    fn effective_benefit_minus_harm_matches_competitive_route_delta() {
        let mut graph = FutureGraph::new(state(80, vec![Coord { x: 4, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let edge = first_edge_for(&graph, Direction::Right, |_| true);
        let score = &edge.transition;

        assert_eq!(
            score.effective_benefit.saturating_sub(score.effective_harm),
            score.net.saturating_sub(score.opponent_net_total)
        );
        assert_eq!(
            score.route_delta(),
            score.effective_benefit.saturating_sub(score.effective_harm)
        );
    }

    #[test]
    fn transition_scores_every_living_actor() {
        let mut graph = FutureGraph::new(state(80, vec![Coord { x: 4, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |_| true);
        let score = &edge.transition;

        let ours = actor_index(&root.state, "ours");
        let enemy = actor_index(&root.state, "enemy");
        assert!(score.for_actor(ours).is_some());
        assert!(score.for_actor(enemy).is_some());
        assert_eq!(
            score.opponent_net_total,
            score.for_actor(enemy).unwrap().net
        );
    }

    #[test]
    fn approaching_food_produces_food_benefit() {
        let mut graph = FutureGraph::new(state(80, vec![Coord { x: 4, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |edge| {
            graph
                .node(edge.child)
                .state
                .food
                .contains(&Coord { x: 4, y: 1 })
        });
        let score = &edge.transition;
        let ours = score.for_actor(actor_index(&root.state, "ours")).unwrap();

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
        let ours = score.for_actor(actor_index(&parent.state, "ours")).unwrap();
        let enemy = score
            .for_actor(actor_index(&parent.state, "enemy"))
            .unwrap();

        assert_eq!(ours.terminal_benefit, TERMINAL_UTILITY);
        assert_eq!(ours.terminal_harm, 0);
        assert!(ours.actor_choice_net >= TERMINAL_UTILITY);
        assert!(ours.net < TERMINAL_UTILITY);

        assert_eq!(enemy.terminal_harm, TERMINAL_UTILITY);
        assert_eq!(enemy.terminal_benefit, 0);
        assert!(enemy.actor_choice_net <= -TERMINAL_UTILITY);
        assert!(enemy.net > -TERMINAL_UTILITY);
        assert!(score.opponent_net_total > -TERMINAL_UTILITY);
    }

    #[test]
    fn intermediate_enemy_death_is_not_a_global_billion_point_reward() {
        let parent_state = SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes: vec![
                snake("ours", 100, &[(1, 1), (1, 0)]),
                snake("enemy-a", 100, &[(3, 1), (3, 0)]),
                snake("enemy-b", 100, &[(5, 5), (5, 4)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        };
        let mut child_state = parent_state.clone();
        child_state.turn = 2;
        child_state.snake_mut("enemy-a").unwrap().alive = false;

        let parent_graph = FutureGraph::new(parent_state);
        let child_graph = FutureGraph::new(child_state);
        let score = TransitionScore::from_parts(
            parent_graph.node(parent_graph.root()),
            &[],
            child_graph.node(child_graph.root()),
        );

        let parent = parent_graph.node(parent_graph.root());
        let enemy_a = score
            .for_actor(actor_index(&parent.state, "enemy-a"))
            .unwrap();

        assert!(enemy_a.actor_choice_net <= -TERMINAL_UTILITY);
        assert!(enemy_a.net > -TERMINAL_UTILITY);
        assert!(score.route_delta().abs() < TERMINAL_UTILITY);
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
    fn starvation_pressure_is_a_repeated_survival_harm() {
        let weights = StrategicWeights {
            food: 0,
            hunting: 0,
            survival: 1000,
        };
        let facts = ActorTransitionFacts {
            food_survival_pressure_milli: 600,
            ..ActorTransitionFacts::default()
        };

        let first = score_actor_transition(facts, weights);
        let second = score_actor_transition(facts, weights);

        assert_eq!(first.survival_harm, 600);
        assert_eq!(second.survival_harm, 600);
        assert_eq!(first.net, -600);
        assert_eq!(second.net, -600);
    }

    #[test]
    fn border_pin_risk_is_a_repeated_survival_harm() {
        let weights = StrategicWeights {
            food: 0,
            hunting: 0,
            survival: 1000,
        };
        let facts = ActorTransitionFacts {
            border_pin_risk_milli: 650,
            ..ActorTransitionFacts::default()
        };

        let first = score_actor_transition(facts, weights);
        let second = score_actor_transition(facts, weights);

        assert_eq!(first.survival_harm, 650);
        assert_eq!(second.survival_harm, 650);
        assert_eq!(first.net, -650);
        assert_eq!(second.net, -650);
    }

    #[test]
    fn eating_food_keeps_residual_food_potential_delta() {
        let weights = StrategicWeights {
            food: 1000,
            hunting: 0,
            survival: 0,
        };
        let facts = ActorTransitionFacts {
            ate_food: true,
            consumed_food: Some(Coord { x: 3, y: 3 }),
            food_consumption_factor_milli: 1000,
            food_potential_before: 500,
            food_potential_after: 550,
            ..ActorTransitionFacts::default()
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(
            score.food_benefit,
            FOOD_CONSUMED + 30 * FOOD_POTENTIAL_DELTA_SCALE
        );
        assert_eq!(score.food_harm, 0);
    }

    #[test]
    fn edge_food_consumption_reward_can_be_suppressed_for_large_snake() {
        let weights = StrategicWeights {
            food: 1000,
            hunting: 0,
            survival: 0,
        };
        let facts = ActorTransitionFacts {
            ate_food: true,
            consumed_food: Some(Coord { x: 0, y: 3 }),
            food_consumption_factor_milli: 0,
            food_potential_before: 0,
            food_potential_after: 0,
            ..ActorTransitionFacts::default()
        };

        let score = score_actor_transition(facts, weights);

        assert_eq!(score.food_benefit, 0);
    }

    #[test]
    fn eating_food_is_a_large_food_benefit() {
        let mut graph = FutureGraph::new(state(20, vec![Coord { x: 3, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |edge| {
            !graph
                .node(edge.child)
                .state
                .food
                .contains(&Coord { x: 3, y: 1 })
        });
        let score = &edge.transition;
        let ours = score.for_actor(actor_index(&root.state, "ours")).unwrap();

        assert!(ours.food_benefit > 0);
        assert!(ours.benefit_total >= ours.food_benefit);
    }
}
