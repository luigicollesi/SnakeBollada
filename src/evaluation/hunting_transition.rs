use crate::analysis::{DominationAnalysis, DominationPhase};
use crate::search::graph::SearchNode;
use crate::simulation::state::ActorIndex;

const TERRITORY_ADVANTAGE_DELTA_SCALE: i64 = 1;
const LENGTH_SECURITY_DELTA_SCALE: i64 = 1;
const MOBILITY_PRESSURE_DELTA_SCALE: i64 = 2;
const ESCAPE_PRESSURE_DELTA_SCALE: i64 = 2;
const DOMINATION_PROGRESS_DELTA_SCALE: i64 = 2;
const COUNTER_DOMINATION_DELTA_SCALE: i64 = 1;
const PHASE_STEP: i64 = 180;
const SEARCH_DOMINATION_DELTA_SCALE: i64 = 2;
const SEARCH_PHASE_STEP: i64 = 250;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HuntingTransitionScore {
    pub(crate) target: Option<ActorIndex>,
    pub(crate) threat: Option<ActorIndex>,
    pub(crate) territory_benefit: i64,
    pub(crate) territory_harm: i64,
    pub(crate) length_benefit: i64,
    pub(crate) length_harm: i64,
    pub(crate) mobility_benefit: i64,
    pub(crate) mobility_harm: i64,
    pub(crate) escape_benefit: i64,
    pub(crate) escape_harm: i64,
    pub(crate) domination_benefit: i64,
    pub(crate) domination_harm: i64,
    pub(crate) counter_domination_benefit: i64,
    pub(crate) counter_domination_harm: i64,
    pub(crate) phase_benefit: i64,
    pub(crate) phase_harm: i64,
    pub(crate) capture_benefit: i64,
    pub(crate) kill_benefit: i64,
    pub(crate) total_benefit: i64,
    pub(crate) total_harm: i64,
    pub(crate) net: i64,
    pub(crate) search_priority: i64,
}

pub(crate) fn evaluate_hunting_transition(
    parent: &SearchNode,
    child: &SearchNode,
    actor: ActorIndex,
    capture_benefit: i64,
    kill_benefit: i64,
) -> HuntingTransitionScore {
    let mut score = HuntingTransitionScore {
        capture_benefit: capture_benefit.max(0),
        kill_benefit: kill_benefit.max(0),
        ..HuntingTransitionScore::default()
    };

    let (Some(parent_analysis), Some(child_analysis)) =
        (parent.active_analysis(), child.active_analysis())
    else {
        return finish(score, 0, 0);
    };

    let Some(parent_domination) = DominationAnalysis::best_target_for(
        &parent.state,
        &parent_analysis.territory,
        &parent_analysis.actor_snapshots,
        actor,
    ) else {
        return finish(score, 0, 0);
    };
    score.target = Some(parent_domination.target);

    if let Some((threat_actor, parent_threat)) = DominationAnalysis::strongest_threat_against(
        &parent.state,
        &parent_analysis.territory,
        &parent_analysis.actor_snapshots,
        actor,
    ) {
        score.threat = Some(threat_actor);
        if let Some(child_threat) = DominationAnalysis::against(
            &child.state,
            &child_analysis.territory,
            &child_analysis.actor_snapshots,
            threat_actor,
            actor,
        ) {
            let counter_delta = i64::from(child_threat.progress_milli)
                .saturating_sub(i64::from(parent_threat.progress_milli))
                .saturating_mul(COUNTER_DOMINATION_DELTA_SCALE);
            add_signed_component(
                counter_delta.saturating_neg(),
                &mut score.counter_domination_benefit,
                &mut score.counter_domination_harm,
            );
        }
    }

    let Some(child_domination) = DominationAnalysis::against(
        &child.state,
        &child_analysis.territory,
        &child_analysis.actor_snapshots,
        actor,
        parent_domination.target,
    ) else {
        return finish(score, 0, 0);
    };

    add_signed_component(
        i64::from(child_domination.territory_advantage_milli)
            .saturating_sub(i64::from(parent_domination.territory_advantage_milli))
            .saturating_mul(TERRITORY_ADVANTAGE_DELTA_SCALE),
        &mut score.territory_benefit,
        &mut score.territory_harm,
    );
    add_signed_component(
        i64::from(child_domination.length_security_milli)
            .saturating_sub(i64::from(parent_domination.length_security_milli))
            .saturating_mul(LENGTH_SECURITY_DELTA_SCALE),
        &mut score.length_benefit,
        &mut score.length_harm,
    );
    add_signed_component(
        i64::from(child_domination.mobility_pressure_milli)
            .saturating_sub(i64::from(parent_domination.mobility_pressure_milli))
            .saturating_mul(MOBILITY_PRESSURE_DELTA_SCALE),
        &mut score.mobility_benefit,
        &mut score.mobility_harm,
    );
    add_signed_component(
        i64::from(child_domination.escape_pressure_milli)
            .saturating_sub(i64::from(parent_domination.escape_pressure_milli))
            .saturating_mul(ESCAPE_PRESSURE_DELTA_SCALE),
        &mut score.escape_benefit,
        &mut score.escape_harm,
    );

    let domination_delta = i64::from(child_domination.progress_milli)
        .saturating_sub(i64::from(parent_domination.progress_milli));
    add_signed_component(
        domination_delta.saturating_mul(DOMINATION_PROGRESS_DELTA_SCALE),
        &mut score.domination_benefit,
        &mut score.domination_harm,
    );

    let phase_delta =
        phase_rank(child_domination.phase).saturating_sub(phase_rank(parent_domination.phase));
    add_signed_component(
        phase_delta.saturating_mul(PHASE_STEP),
        &mut score.phase_benefit,
        &mut score.phase_harm,
    );

    let search_priority = domination_delta
        .saturating_mul(SEARCH_DOMINATION_DELTA_SCALE)
        .saturating_add(phase_delta.saturating_mul(SEARCH_PHASE_STEP))
        .saturating_add(
            i64::from(child_domination.mobility_pressure_milli)
                .saturating_sub(i64::from(parent_domination.mobility_pressure_milli)),
        )
        .saturating_add(
            i64::from(child_domination.escape_pressure_milli)
                .saturating_sub(i64::from(parent_domination.escape_pressure_milli)),
        );

    finish(score, search_priority, domination_delta)
}

fn finish(
    mut score: HuntingTransitionScore,
    search_priority: i64,
    _domination_delta: i64,
) -> HuntingTransitionScore {
    score.total_benefit = score
        .capture_benefit
        .saturating_add(score.kill_benefit)
        .saturating_add(score.territory_benefit)
        .saturating_add(score.length_benefit)
        .saturating_add(score.mobility_benefit)
        .saturating_add(score.escape_benefit)
        .saturating_add(score.domination_benefit)
        .saturating_add(score.counter_domination_benefit)
        .saturating_add(score.phase_benefit);
    score.total_harm = score
        .territory_harm
        .saturating_add(score.length_harm)
        .saturating_add(score.mobility_harm)
        .saturating_add(score.escape_harm)
        .saturating_add(score.domination_harm)
        .saturating_add(score.counter_domination_harm)
        .saturating_add(score.phase_harm);
    score.net = score.total_benefit.saturating_sub(score.total_harm);
    score.search_priority = search_priority
        .saturating_add(score.capture_benefit.saturating_div(2))
        .saturating_add(score.kill_benefit);
    score
}

fn add_signed_component(value: i64, benefit: &mut i64, harm: &mut i64) {
    if value > 0 {
        *benefit = benefit.saturating_add(value);
    } else if value < 0 {
        *harm = harm.saturating_add(value.saturating_neg());
    }
}

const fn phase_rank(phase: DominationPhase) -> i64 {
    match phase {
        DominationPhase::Neutral => 0,
        DominationPhase::Pressure => 1,
        DominationPhase::Dominance => 2,
        DominationPhase::Closure => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_progression_is_rewarded_and_regression_is_harm() {
        assert!(phase_rank(DominationPhase::Closure) > phase_rank(DominationPhase::Dominance));
        assert!(phase_rank(DominationPhase::Dominance) > phase_rank(DominationPhase::Pressure));
    }

    #[test]
    fn finish_keeps_capture_and_kill_as_hunting_benefit() {
        let score = finish(
            HuntingTransitionScore {
                capture_benefit: 300,
                kill_benefit: 1400,
                ..HuntingTransitionScore::default()
            },
            0,
            0,
        );

        assert_eq!(score.total_benefit, 1700);
        assert_eq!(score.total_harm, 0);
        assert_eq!(score.net, 1700);
        assert!(score.search_priority >= 1400);
    }
}
