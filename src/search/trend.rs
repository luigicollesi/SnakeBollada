#![allow(dead_code)]

use crate::search::graph::SearchNode;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SearchTrend {
    pub(crate) our_control_delta_milli: i16,
    pub(crate) strongest_enemy_control_loss_milli: i16,
    pub(crate) target_control_loss_milli: i16,
    pub(crate) frontier_delta: i16,
    pub(crate) border_risk_delta_milli: i16,
    pub(crate) border_chain_delta: i16,
}

impl SearchTrend {
    pub(crate) fn between(
        parent: &SearchNode,
        child: &SearchNode,
        hunt_target: Option<&str>,
    ) -> Self {
        let (Some(parent_analysis), Some(child_analysis)) =
            (parent.active_analysis(), child.active_analysis())
        else {
            return Self::default();
        };

        let our_id = parent.state.our_snake_id.as_str();
        let our_control_delta_milli = match (
            parent_analysis.territory.competitive_for_snake(our_id),
            child_analysis.territory.competitive_for_snake(our_id),
        ) {
            (Some(before), Some(after)) => {
                signed_delta(after.control_ratio_milli, before.control_ratio_milli)
            }
            _ => 0,
        };

        let frontier_delta = match (
            parent_analysis.territory.competitive_for_snake(our_id),
            child_analysis.territory.competitive_for_snake(our_id),
        ) {
            (Some(before), Some(after)) => {
                let before_frontier =
                    i32::from(before.winning_frontier) - i32::from(before.losing_frontier);
                let after_frontier =
                    i32::from(after.winning_frontier) - i32::from(after.losing_frontier);
                clamp_i16(after_frontier.saturating_sub(before_frontier))
            }
            _ => 0,
        };

        let strongest_enemy_control_loss_milli = parent
            .state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != our_id)
            .filter_map(|snake| {
                let before = parent_analysis
                    .territory
                    .competitive_for_snake(&snake.id)?;
                let after = child_analysis.territory.competitive_for_snake(&snake.id)?;
                Some(signed_delta(
                    before.control_ratio_milli,
                    after.control_ratio_milli,
                ))
            })
            .max()
            .unwrap_or(0);

        let target_control_loss_milli = hunt_target
            .and_then(|target| {
                let before = parent_analysis.territory.competitive_for_snake(target)?;
                let after = child_analysis.territory.competitive_for_snake(target)?;
                Some(signed_delta(
                    before.control_ratio_milli,
                    after.control_ratio_milli,
                ))
            })
            .unwrap_or(0);

        let (border_risk_delta_milli, border_chain_delta) =
            match (parent_analysis.border.ours(), child_analysis.border.ours()) {
                (Some(before), Some(after)) => (
                    signed_delta(
                        after.structural_risk_milli,
                        before.structural_risk_milli,
                    ),
                    signed_delta(after.leading_edge_chain, before.leading_edge_chain),
                ),
                _ => (0, 0),
            };

        Self {
            our_control_delta_milli,
            strongest_enemy_control_loss_milli,
            target_control_loss_milli,
            frontier_delta,
            border_risk_delta_milli,
            border_chain_delta,
        }
    }

    pub(crate) fn tactical_priority_milli(self) -> u16 {
        let our_gain = positive(self.our_control_delta_milli);
        let enemy_denial = positive(
            self.target_control_loss_milli
                .max(self.strongest_enemy_control_loss_milli),
        );
        let frontier_gain = positive(self.frontier_delta);

        u32::from(our_gain)
            .saturating_mul(2)
            .saturating_add(u32::from(enemy_denial).saturating_mul(2))
            .saturating_add(u32::from(frontier_gain).saturating_mul(40))
            .min(1000)
            .try_into()
            .unwrap_or(1000)
    }

    pub(crate) fn danger_priority_milli(self) -> u16 {
        let border_growth = positive(self.border_risk_delta_milli);
        let chain_growth = positive(self.border_chain_delta);

        u32::from(border_growth)
            .saturating_mul(2)
            .saturating_add(u32::from(chain_growth).saturating_mul(150))
            .min(1000)
            .try_into()
            .unwrap_or(1000)
    }
}

fn signed_delta(after: u16, before: u16) -> i16 {
    clamp_i16(i32::from(after).saturating_sub(i32::from(before)))
}

fn clamp_i16(value: i32) -> i16 {
    value
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
        .try_into()
        .unwrap_or(if value.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        })
}

fn positive(value: i16) -> u16 {
    value.max(0).try_into().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn territorial_gain_increases_tactical_priority() {
        let trend = SearchTrend {
            our_control_delta_milli: 80,
            target_control_loss_milli: 120,
            frontier_delta: 2,
            ..SearchTrend::default()
        };

        assert!(trend.tactical_priority_milli() >= 400);
    }

    #[test]
    fn worsening_border_increases_danger_priority() {
        let trend = SearchTrend {
            border_risk_delta_milli: 180,
            border_chain_delta: 2,
            ..SearchTrend::default()
        };

        assert!(trend.danger_priority_milli() >= 600);
    }

    #[test]
    fn improving_border_does_not_create_danger() {
        let trend = SearchTrend {
            border_risk_delta_milli: -200,
            border_chain_delta: -3,
            ..SearchTrend::default()
        };

        assert_eq!(trend.danger_priority_milli(), 0);
    }
}
