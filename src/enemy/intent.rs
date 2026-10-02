use crate::direction::Direction;
use crate::search::graph::FutureGraph;
use crate::simulation::state::ActorIndex;

const MIN_MEANINGFUL_CONTRAST: i32 = 120;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IntentContrast {
    pub(crate) food_milli: i16,
    pub(crate) hunting_milli: i16,
    pub(crate) trapping_milli: i16,
    pub(crate) information_milli: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct IntentEvidence {
    food_milli: u16,
    hunting_milli: u16,
    trapping_milli: u16,
}

struct EdgeEvidenceContext<'a> {
    parent: &'a crate::search::graph::SearchNode,
    child: &'a crate::search::graph::SearchNode,
    actor_score: Option<&'a crate::evaluation::ActorTransitionScore>,
    hypothesis: Option<crate::enemy::tracing::OpponentMoveHypothesis>,
    enemy_actor: ActorIndex,
    our_actor: ActorIndex,
    enemy_food_before: u16,
    our_before: &'a crate::evaluation::ActorSnapshot,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct IntentAccumulator {
    food_total: u32,
    hunting_total: u32,
    trapping_total: u32,
    samples: u16,
}

impl IntentAccumulator {
    fn add(&mut self, evidence: IntentEvidence) {
        self.food_total = self
            .food_total
            .saturating_add(u32::from(evidence.food_milli));
        self.hunting_total = self
            .hunting_total
            .saturating_add(u32::from(evidence.hunting_milli));
        self.trapping_total = self
            .trapping_total
            .saturating_add(u32::from(evidence.trapping_milli));
        self.samples = self.samples.saturating_add(1);
    }

    fn mean(self) -> Option<IntentEvidence> {
        let samples = u32::from(self.samples);
        if samples == 0 {
            return None;
        }

        Some(IntentEvidence {
            food_milli: self
                .food_total
                .saturating_div(samples)
                .min(1000)
                .try_into()
                .unwrap_or(1000),
            hunting_milli: self
                .hunting_total
                .saturating_div(samples)
                .min(1000)
                .try_into()
                .unwrap_or(1000),
            trapping_milli: self
                .trapping_total
                .saturating_div(samples)
                .min(1000)
                .try_into()
                .unwrap_or(1000),
        })
    }
}

pub(crate) fn infer_observed_intent(
    graph: &FutureGraph,
    enemy_actor: ActorIndex,
    observed: Direction,
) -> Option<IntentContrast> {
    let root = graph.node(graph.root());
    let analysis = root.active_analysis()?;
    let enemy_snapshot = analysis.actor_snapshot(enemy_actor)?;
    let our_actor = root.state.actor_index(&root.state.our_snake_id)?;
    let our_snapshot = analysis.actor_snapshot(our_actor)?;
    let move_set = analysis.tracing.for_actor(enemy_actor)?;

    let mut by_direction = [IntentAccumulator::default(); 4];
    for edge in &root.children {
        let Some(direction) = edge.joint_action.direction_for(enemy_actor) else {
            continue;
        };
        let child = graph.node(edge.child);
        let evidence = edge_evidence(EdgeEvidenceContext {
            parent: root,
            child,
            actor_score: edge.transition.for_actor(enemy_actor),
            hypothesis: move_set.hypothesis(direction),
            enemy_actor,
            our_actor,
            enemy_food_before: enemy_snapshot.metrics.food_potential_milli,
            our_before: our_snapshot,
        });
        by_direction[usize::from(direction.rank())].add(evidence);
    }

    contrast_from_accumulators(by_direction, observed, enemy_snapshot.weights.survival)
}

fn edge_evidence(context: EdgeEvidenceContext<'_>) -> IntentEvidence {
    let child_analysis = context.child.active_analysis();
    let enemy_after =
        child_analysis.and_then(|analysis| analysis.actor_snapshot(context.enemy_actor));
    let our_after =
        child_analysis.and_then(|analysis| analysis.actor_snapshot(context.our_actor));

    let consumed_food = context
        .child
        .state
        .snake_at(context.enemy_actor)
        .filter(|snake| snake.alive)
        .and_then(|snake| snake.head())
        .is_some_and(|head| context.parent.state.food.contains(&head));
    let food_gain = enemy_after
        .map(|snapshot| {
            snapshot
                .metrics
                .food_potential_milli
                .saturating_sub(context.enemy_food_before)
        })
        .unwrap_or(0);
    let food_support = context
        .hypothesis
        .is_some_and(|candidate| candidate.support.food);
    let food_milli = u32::from(food_gain)
        .min(350)
        .saturating_add(u32::from(food_support) * 300)
        .saturating_add(u32::from(consumed_food) * 700)
        .min(1000)
        .try_into()
        .unwrap_or(1000);

    let direct_hunting = context
        .actor_score
        .map(|score| {
            score
                .hunting_benefit
                .saturating_sub(score.hunting_harm)
                .max(0)
        })
        .unwrap_or(0)
        .min(700)
        .try_into()
        .unwrap_or(700_u16);
    let head_threat = context
        .hypothesis
        .is_some_and(|candidate| candidate.support.head_threat);
    let hunting_milli = u32::from(direct_hunting)
        .saturating_add(u32::from(head_threat) * 350)
        .min(1000)
        .try_into()
        .unwrap_or(1000);

    let trapping_milli = trapping_effect_milli(context.our_before, our_after);

    IntentEvidence {
        food_milli,
        hunting_milli,
        trapping_milli,
    }
}

fn trapping_effect_milli(
    before: &crate::evaluation::ActorSnapshot,
    after: Option<&crate::evaluation::ActorSnapshot>,
) -> u16 {
    let Some(after) = after else {
        return 1000;
    };

    let move_loss = before
        .metrics
        .safe_non_reverse_moves
        .saturating_sub(after.metrics.safe_non_reverse_moves);
    let enclosure_gain = after
        .metrics
        .enclosure_risk
        .saturating_sub(before.metrics.enclosure_risk);
    let space_loss = before
        .metrics
        .space_capacity_milli
        .saturating_sub(after.metrics.space_capacity_milli);
    let territory_loss = before
        .metrics
        .territory_control_milli
        .saturating_sub(after.metrics.territory_control_milli);
    let pin_gain = after
        .metrics
        .border_pin_risk_milli
        .saturating_sub(before.metrics.border_pin_risk_milli);
    let escape_gain = after
        .metrics
        .border_escape_pressure_milli
        .saturating_sub(before.metrics.border_escape_pressure_milli);

    u32::from(move_loss)
        .saturating_mul(260)
        .saturating_add(u32::from(enclosure_gain).saturating_mul(240))
        .saturating_add(u32::from(space_loss).saturating_div(2))
        .saturating_add(u32::from(territory_loss).saturating_div(5))
        .saturating_add(u32::from(pin_gain).saturating_div(2))
        .saturating_add(u32::from(escape_gain).saturating_div(2))
        .min(1000)
        .try_into()
        .unwrap_or(1000)
}

fn contrast_from_accumulators(
    by_direction: [IntentAccumulator; 4],
    observed: Direction,
    enemy_survival_weight: u16,
) -> Option<IntentContrast> {
    let observed_index = usize::from(observed.rank());
    let observed = by_direction[observed_index].mean()?;

    let alternatives = by_direction
        .iter()
        .enumerate()
        .filter(|(index, accumulator)| *index != observed_index && accumulator.samples > 0)
        .filter_map(|(_, accumulator)| accumulator.mean())
        .collect::<Vec<_>>();
    if alternatives.is_empty() {
        return None;
    }

    let alternative_count = u32::try_from(alternatives.len()).unwrap_or(u32::MAX).max(1);
    let alternative_mean = IntentEvidence {
        food_milli: alternatives
            .iter()
            .map(|evidence| u32::from(evidence.food_milli))
            .sum::<u32>()
            .saturating_div(alternative_count)
            .min(1000)
            .try_into()
            .unwrap_or(1000),
        hunting_milli: alternatives
            .iter()
            .map(|evidence| u32::from(evidence.hunting_milli))
            .sum::<u32>()
            .saturating_div(alternative_count)
            .min(1000)
            .try_into()
            .unwrap_or(1000),
        trapping_milli: alternatives
            .iter()
            .map(|evidence| u32::from(evidence.trapping_milli))
            .sum::<u32>()
            .saturating_div(alternative_count)
            .min(1000)
            .try_into()
            .unwrap_or(1000),
    };

    let food = signed_contrast(observed.food_milli, alternative_mean.food_milli);
    let hunting = signed_contrast(observed.hunting_milli, alternative_mean.hunting_milli);
    let trapping = signed_contrast(observed.trapping_milli, alternative_mean.trapping_milli);
    let strongest = [food, hunting, trapping]
        .into_iter()
        .map(i32::abs)
        .max()
        .unwrap_or(0);
    if strongest < MIN_MEANINGFUL_CONTRAST {
        return None;
    }

    let survival_information_factor = match enemy_survival_weight {
        800..=u16::MAX => 250_u32,
        600..=799 => 500,
        _ => 1000,
    };
    let information_milli = u32::try_from(strongest)
        .unwrap_or(1000)
        .min(1000)
        .saturating_mul(survival_information_factor)
        .saturating_div(1000)
        .try_into()
        .unwrap_or(1000);

    Some(IntentContrast {
        food_milli: signed_i16(food),
        hunting_milli: signed_i16(hunting),
        trapping_milli: signed_i16(trapping),
        information_milli,
    })
}

fn signed_contrast(observed: u16, alternative: u16) -> i32 {
    i32::from(observed)
        .saturating_sub(i32::from(alternative))
        .clamp(-1000, 1000)
}

fn signed_i16(value: i32) -> i16 {
    value
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
        .try_into()
        .unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accumulator(food: u16, hunting: u16, trapping: u16) -> IntentAccumulator {
        let mut accumulator = IntentAccumulator::default();
        accumulator.add(IntentEvidence {
            food_milli: food,
            hunting_milli: hunting,
            trapping_milli: trapping,
        });
        accumulator
    }

    #[test]
    fn observed_trap_over_food_alternative_yields_trapping_evidence() {
        let mut directions = [IntentAccumulator::default(); 4];
        directions[usize::from(Direction::Down.rank())] = accumulator(80, 300, 850);
        directions[usize::from(Direction::Left.rank())] = accumulator(900, 50, 40);

        let contrast = contrast_from_accumulators(directions, Direction::Down, 200).unwrap();

        assert!(contrast.trapping_milli > 700);
        assert!(contrast.food_milli < -700);
        assert!(contrast.information_milli >= 700);
    }

    #[test]
    fn forced_single_direction_produces_no_intent_learning() {
        let mut directions = [IntentAccumulator::default(); 4];
        directions[usize::from(Direction::Down.rank())] = accumulator(100, 100, 900);

        assert!(contrast_from_accumulators(directions, Direction::Down, 200).is_none());
    }

    #[test]
    fn survival_emergency_reduces_information_gain() {
        let mut directions = [IntentAccumulator::default(); 4];
        directions[usize::from(Direction::Down.rank())] = accumulator(100, 200, 900);
        directions[usize::from(Direction::Left.rank())] = accumulator(800, 100, 100);

        let normal = contrast_from_accumulators(directions, Direction::Down, 200).unwrap();
        let emergency = contrast_from_accumulators(directions, Direction::Down, 850).unwrap();

        assert!(emergency.information_milli < normal.information_milli);
    }
}
