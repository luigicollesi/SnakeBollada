//! Primary territorial root ordering, isolated from Shadow and conservative V2.
use std::cmp::Ordering;
use std::collections::HashMap;

use crate::analysis::TerritoryDirectionSample;
use crate::direction::Direction;

// Ordering-only adjustment: bounded to 25 milli (2.5% of one territory-ratio
// scale). Unknown recovery and terminal reply prefixes are never penalized.
fn drop_adjustment(sample: &TerritoryDirectionSample) -> i64 {
    if sample.terminal_reply_seen {
        return 0;
    }
    let Some(drop) = sample.territorial_drop.filter(|drop| drop.significant()) else {
        return 0;
    };
    let ply = drop.max_drop_ply.unwrap_or(u8::MAX);
    if ply > sample.min_sampled_depth {
        return 0;
    }
    10 + i64::from(drop.max_relative_drop_milli.saturating_sub(400) / 40).min(8)
        + i64::from(drop.max_absolute_drop.saturating_sub(8) / 5).min(4)
        + i64::from(4_u8.saturating_sub(ply)).min(3)
}

/// The validated primary comparison remains byte-for-byte equivalent when
/// experimental_drop is false. V3 only adjusts the EXPANSION ORDER, never
/// MAX/MIN ranks or adversarial response coverage.
pub(crate) fn sort_primary_root(
    directions: &mut [Direction],
    preferred: Option<Direction>,
    tactical: &HashMap<Direction, (u8, u8)>,
    territorial: &HashMap<Direction, TerritoryDirectionSample>,
    experimental_drop: bool,
) {
    if territorial.is_empty() || territorial.len() != directions.len() {
        return;
    }
    directions.sort_by(|left, right| {
        let a = *left;
        let b = *right;
        let (left_exits, left_enemy) = tactical.get(&a).copied().unwrap_or((0, u8::MAX));
        let (right_exits, right_enemy) = tactical.get(&b).copied().unwrap_or((0, u8::MAX));
        let left_hint = territorial.get(&a);
        let right_hint = territorial.get(&b);
        (left_exits == 0)
            .cmp(&(right_exits == 0))
            .then_with(|| match (left_hint, right_hint) {
                (Some(left_hint), Some(right_hint)) if experimental_drop => {
                    let left_adjusted = left_hint
                        .worst_mean_margin_milli
                        .saturating_sub(drop_adjustment(left_hint));
                    let right_adjusted = right_hint
                        .worst_mean_margin_milli
                        .saturating_sub(drop_adjustment(right_hint));
                    right_adjusted
                        .cmp(&left_adjusted)
                        .then_with(|| right_hint.ordering_key().cmp(&left_hint.ordering_key()))
                }
                (Some(left_hint), Some(right_hint)) => {
                    right_hint.ordering_key().cmp(&left_hint.ordering_key())
                }
                _ => Ordering::Equal,
            })
            .then_with(|| (Some(a) != preferred).cmp(&(Some(b) != preferred)))
            .then_with(|| right_exits.cmp(&left_exits))
            .then_with(|| left_enemy.cmp(&right_enemy))
            .then_with(|| a.rank().cmp(&b.rank()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::TerritorialDrop;
    use crate::search::forecast::ForecastCertainty;
    use crate::search::graph::ResponseCoverage;

    fn hint(mean: i64, drop: Option<TerritorialDrop>) -> TerritoryDirectionSample {
        TerritoryDirectionSample {
            worst_mean_margin_milli: mean,
            worst_minimum_margin_milli: mean,
            examined_replies: 2,
            max_sampled_depth: 3,
            min_sampled_depth: 2,
            coverage: ResponseCoverage::Partial,
            certainty: ForecastCertainty::FoodProvisional,
            adversarially_complete: false,
            territorial_drop: drop,
            terminal_reply_seen: false,
        }
    }

    #[test]
    fn primary_retains_territorial_priority_even_with_sharp_drop() {
        let mut directions = vec![Direction::Right, Direction::Up];
        let tactical = HashMap::from([(Direction::Right, (1, 2)), (Direction::Up, (2, 1))]);
        let territorial = HashMap::from([
            (
                Direction::Right,
                hint(
                    299,
                    Some(TerritorialDrop {
                        max_absolute_drop: 22,
                        max_relative_drop_milli: 733,
                        first_drop_ply: Some(2),
                        max_drop_ply: Some(2),
                        opponent_gain: 15,
                        recovered_next_ply: Some(false),
                    }),
                ),
            ),
            (Direction::Up, hint(290, None)),
        ]);
        sort_primary_root(&mut directions, None, &tactical, &territorial, false);
        assert_eq!(directions[0], Direction::Right);
        sort_primary_root(&mut directions, None, &tactical, &territorial, true);
        assert_eq!(directions[0], Direction::Up);
    }

    #[test]
    fn v3_no_longer_depends_on_current_continuations_or_margin_buckets() {
        let drop = Some(TerritorialDrop {
            max_absolute_drop: 22,
            max_relative_drop_milli: 733,
            first_drop_ply: Some(1),
            max_drop_ply: Some(2),
            opponent_gain: 14,
            recovered_next_ply: Some(false),
        });
        let tactical = HashMap::from([
            (Direction::Right, (4, 4)),
            (Direction::Up, (3, 4)),
        ]);
        let territorial = HashMap::from([
            (Direction::Right, hint(300, drop)),
            (Direction::Up, hint(299, None)),
        ]);
        let mut directions = vec![Direction::Right, Direction::Up];
        sort_primary_root(&mut directions, None, &tactical, &territorial, false);
        assert_eq!(directions[0], Direction::Right);
        sort_primary_root(&mut directions, None, &tactical, &territorial, true);
        assert_eq!(directions[0], Direction::Up);
    }

    #[test]
    fn v3_does_not_penalize_unknown_or_recovered_or_terminal_drops() {
        let mut sample = hint(320, Some(TerritorialDrop {
            max_absolute_drop: 22,
            max_relative_drop_milli: 733,
            first_drop_ply: Some(2),
            max_drop_ply: Some(2),
            opponent_gain: 12,
            recovered_next_ply: None,
        }));
        assert_eq!(drop_adjustment(&sample), 0);
        sample.territorial_drop.as_mut().unwrap().recovered_next_ply = Some(true);
        assert_eq!(drop_adjustment(&sample), 0);
        sample.territorial_drop.as_mut().unwrap().recovered_next_ply = Some(false);
        assert!(drop_adjustment(&sample) > 0);
        sample.terminal_reply_seen = true;
        assert_eq!(drop_adjustment(&sample), 0);
    }

    #[test]
    fn v3_preserves_tactical_no_exit_and_strong_territorial_leads() {
        let drop = Some(TerritorialDrop {
            max_absolute_drop: 25,
            max_relative_drop_milli: 900,
            first_drop_ply: Some(2),
            max_drop_ply: Some(2),
            opponent_gain: 20,
            recovered_next_ply: Some(false),
        });
        let territorial = HashMap::from([
            (Direction::Right, hint(900, drop)),
            (Direction::Up, hint(300, None)),
        ]);
        let mut directions = vec![Direction::Up, Direction::Right];
        sort_primary_root(&mut directions, None, &HashMap::new(), &territorial, true);
        assert_eq!(directions[0], Direction::Right);

        let tactical = HashMap::from([
            (Direction::Right, (0, 4)),
            (Direction::Up, (2, 4)),
        ]);
        sort_primary_root(&mut directions, None, &tactical, &territorial, true);
        assert_eq!(directions[0], Direction::Up);
    }

    #[test]
    fn v3_drop_requires_observation_inside_sampled_horizon() {
        let mut sample = hint(300, Some(TerritorialDrop {
            max_absolute_drop: 20,
            max_relative_drop_milli: 700,
            first_drop_ply: Some(1),
            max_drop_ply: Some(3),
            opponent_gain: 15,
            recovered_next_ply: Some(false),
        }));
        sample.min_sampled_depth = 2;
        assert_eq!(drop_adjustment(&sample), 0);
        sample.min_sampled_depth = 3;
        assert!(drop_adjustment(&sample) > 0);
    }

    #[test]
    fn missing_territorial_samples_never_change_tactical_order() {
        let mut directions = vec![Direction::Left, Direction::Down];
        let territorial = HashMap::from([(Direction::Left, hint(900, None))]);
        sort_primary_root(&mut directions, None, &HashMap::new(), &territorial, true);
        assert_eq!(directions, vec![Direction::Left, Direction::Down]);
    }
}
