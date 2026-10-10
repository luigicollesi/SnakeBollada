//! Primary territorial root ordering, isolated from Shadow and conservative V2.
use std::cmp::Ordering;
use std::collections::HashMap;

use crate::analysis::{TerritorialDrop, TerritoryDirectionSample};
use crate::direction::Direction;

fn risky_drop(drop: Option<TerritorialDrop>, continuations: Option<u8>) -> bool {
    drop.is_some_and(|signal| signal.significant()) && continuations.is_some_and(|count| count <= 2)
}

/// Stable winning comparator is retained when experimental_drop is false.
/// V3 only uses a sharp drop as a tiebreak inside a 50-milli margin bucket.
/// Samples are partial and can never change MAX/MIN score or reply coverage.
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
                    let left_bucket = left_hint.worst_mean_margin_milli.div_euclid(50);
                    let right_bucket = right_hint.worst_mean_margin_milli.div_euclid(50);
                    right_bucket
                        .cmp(&left_bucket)
                        .then_with(|| {
                            let risk = |hint: &TerritoryDirectionSample, direction: Direction| {
                                let continuations = tactical.get(&direction).map(|(ours, _)| *ours);
                                risky_drop(hint.territorial_drop, continuations)
                            };
                            risk(left_hint, a).cmp(&risk(right_hint, b))
                        })
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
    fn missing_territorial_samples_never_change_tactical_order() {
        let mut directions = vec![Direction::Left, Direction::Down];
        let territorial = HashMap::from([(Direction::Left, hint(900, None))]);
        sort_primary_root(&mut directions, None, &HashMap::new(), &territorial, true);
        assert_eq!(directions, vec![Direction::Left, Direction::Down]);
    }
}
