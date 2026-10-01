use crate::simulation::state::SimulatedGameState;

const MIN_OPPORTUNISTIC_FOOD_DRIVE: u16 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StrategicPhase {
    #[default]
    Growth,
    Balanced,
    Dominant,
    Apex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrategicPosture {
    pub(crate) phase: StrategicPhase,
    pub(crate) our_length: usize,
    pub(crate) largest_enemy_length: usize,
    pub(crate) lead_over_largest_enemy: i16,
    pub(crate) unique_largest: bool,
    pub(crate) growth_aggression_milli: u16,
    pub(crate) size_dominance_milli: u16,
    pub(crate) hunt_drive_milli: u16,
    pub(crate) food_urgency_milli: u16,
}

impl Default for StrategicPosture {
    fn default() -> Self {
        Self {
            phase: StrategicPhase::Growth,
            our_length: 0,
            largest_enemy_length: 0,
            lead_over_largest_enemy: 0,
            unique_largest: false,
            growth_aggression_milli: 0,
            size_dominance_milli: 0,
            hunt_drive_milli: 0,
            food_urgency_milli: 0,
        }
    }
}

impl StrategicPosture {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
            return Self {
                phase: StrategicPhase::Growth,
                our_length: 0,
                largest_enemy_length: 0,
                lead_over_largest_enemy: 0,
                unique_largest: false,
                growth_aggression_milli: 0,
                size_dominance_milli: 0,
                hunt_drive_milli: 0,
                food_urgency_milli: 1000,
            };
        };

        let largest_enemy_length = state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != state.our_snake_id)
            .map(|snake| snake.length())
            .max()
            .unwrap_or(0);

        let lead = i32::try_from(ours.length())
            .unwrap_or(i32::MAX)
            .saturating_sub(i32::try_from(largest_enemy_length).unwrap_or(i32::MAX));
        let lead_over_largest_enemy = lead
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
            .try_into()
            .unwrap_or(if lead.is_negative() {
                i16::MIN
            } else {
                i16::MAX
            });
        let unique_largest = largest_enemy_length > 0 && ours.length() > largest_enemy_length;

        let growth_aggression_milli = (state.aggression.value.clamp(0.0, 1.0) * 1000.0)
            .round()
            .clamp(0.0, 1000.0) as u16;
        let size_dominance_milli = size_dominance_milli(lead_over_largest_enemy, unique_largest);
        let hunt_drive_milli = growth_aggression_milli.max(size_dominance_milli);
        let food_urgency_milli = food_urgency_milli(ours.health, state.rules.max_health);

        let phase = if lead_over_largest_enemy < 0 {
            StrategicPhase::Growth
        } else if lead_over_largest_enemy == 0 {
            StrategicPhase::Balanced
        } else if lead_over_largest_enemy >= 3 {
            StrategicPhase::Apex
        } else {
            StrategicPhase::Dominant
        };

        Self {
            phase,
            our_length: ours.length(),
            largest_enemy_length,
            lead_over_largest_enemy,
            unique_largest,
            growth_aggression_milli,
            size_dominance_milli,
            hunt_drive_milli,
            food_urgency_milli,
        }
    }

    pub(crate) fn food_drive_milli(self) -> u16 {
        self.food_urgency_milli.max(
            1000_u16
                .saturating_sub(self.hunt_drive_milli)
                .max(MIN_OPPORTUNISTIC_FOOD_DRIVE),
        )
    }

    pub(crate) fn food_is_critical(self) -> bool {
        self.food_urgency_milli >= 800
    }

    pub(crate) fn favors_dominant_hunt(self) -> bool {
        matches!(self.phase, StrategicPhase::Dominant | StrategicPhase::Apex)
            && self.unique_largest
            && self.food_urgency_milli < 450
            && self.hunt_drive_milli >= 550
    }
}

fn size_dominance_milli(lead: i16, unique_largest: bool) -> u16 {
    let base: u16 = match lead {
        i16::MIN..=-2 => 0,
        -1 => 100,
        0 => 250,
        1 => 550,
        2 => 675,
        3 => 775,
        4 => 850,
        _ => 900,
    };

    if unique_largest {
        base.saturating_add(25).min(950)
    } else {
        base
    }
}

fn food_urgency_milli(health: i32, max_health: i32) -> u16 {
    let max_health = max_health.max(1);
    let health_milli = health
        .max(0)
        .saturating_mul(1000)
        .saturating_div(max_health)
        .clamp(0, 1000);

    match health_milli {
        650..=1000 => 0,
        550..=649 => interpolate_descending(health_milli, 550, 650, 150, 0),
        450..=549 => interpolate_descending(health_milli, 450, 550, 350, 150),
        350..=449 => interpolate_descending(health_milli, 350, 450, 600, 350),
        250..=349 => interpolate_descending(health_milli, 250, 350, 800, 600),
        150..=249 => interpolate_descending(health_milli, 150, 250, 1000, 800),
        _ => 1000,
    }
}

fn interpolate_descending(value: i32, low: i32, high: i32, low_score: u16, high_score: u16) -> u16 {
    let span = high.saturating_sub(low).max(1);
    let offset = value.saturating_sub(low).clamp(0, span);
    let score_span = i32::from(low_score).saturating_sub(i32::from(high_score));
    i32::from(low_score)
        .saturating_sub(score_span.saturating_mul(offset).saturating_div(span))
        .clamp(0, 1000)
        .try_into()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};
    use crate::Coord;

    use super::*;

    fn snake(id: &str, length: usize, health: i32) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: (0..length)
                .map(|index| Coord {
                    x: i32::try_from(index).unwrap_or(0),
                    y: 0,
                })
                .collect(),
            alive: true,
        }
    }

    fn state(ours: usize, enemy: usize, health: i32, aggression: f32) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 11,
            height: 11,
            food: vec![],
            hazards: vec![],
            snakes: vec![snake("ours", ours, health), snake("enemy", enemy, 100)],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState {
                fruits_eaten: 0,
                value: aggression,
            },
        }
    }

    #[test]
    fn large_length_lead_creates_apex_hunt_drive_without_food_history() {
        let posture = StrategicPosture::from_state(&state(12, 6, 90, 0.0));

        assert_eq!(posture.phase, StrategicPhase::Apex);
        assert!(posture.unique_largest);
        assert!(posture.hunt_drive_milli >= 900);
        assert!(posture.food_drive_milli() <= 150);
    }

    #[test]
    fn low_health_keeps_food_critical_even_when_apex() {
        let posture = StrategicPosture::from_state(&state(12, 6, 18, 0.0));

        assert_eq!(posture.phase, StrategicPhase::Apex);
        assert!(posture.food_is_critical());
        assert!(posture.food_drive_milli() >= 900);
    }

    #[test]
    fn shorter_snake_stays_in_growth_phase() {
        let posture = StrategicPosture::from_state(&state(6, 10, 90, 0.0));

        assert_eq!(posture.phase, StrategicPhase::Growth);
        assert_eq!(posture.size_dominance_milli, 0);
        assert!(posture.food_drive_milli() > posture.hunt_drive_milli);
    }
}
