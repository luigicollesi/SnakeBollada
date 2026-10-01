use crate::simulation::state::SimulatedGameState;
use crate::Coord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpatialOccupancy {
    width: u16,
    height: u16,
    retained_body: Vec<bool>,
}

impl SpatialOccupancy {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let width = state.width as u16;
        let height = state.height as u16;
        let mut retained_body =
            vec![false; usize::from(width).saturating_mul(usize::from(height))];

        for snake in state.snakes.iter().filter(|snake| snake.alive) {
            let retained_len = snake.body.len().saturating_sub(1);
            for segment in snake.body.iter().take(retained_len) {
                if let Some(index) = index_of(width, height, *segment) {
                    retained_body[index] = true;
                }
            }
        }

        Self {
            width,
            height,
            retained_body,
        }
    }

    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    pub(crate) fn height(&self) -> u16 {
        self.height
    }

    pub(crate) fn in_bounds(&self, coord: Coord) -> bool {
        index_of(self.width, self.height, coord).is_some()
    }

    pub(crate) fn retained_contains(&self, coord: Coord) -> bool {
        index_of(self.width, self.height, coord)
            .and_then(|index| self.retained_body.get(index))
            .copied()
            .unwrap_or(false)
    }

    pub(crate) fn territory_open(&self, state: &SimulatedGameState) -> Vec<bool> {
        let mut open = self
            .retained_body
            .iter()
            .map(|occupied| !occupied)
            .collect::<Vec<_>>();

        for snake in state.snakes.iter().filter(|snake| snake.alive) {
            let Some(head) = snake.head() else {
                continue;
            };
            let Some(index) = index_of(self.width, self.height, head) else {
                continue;
            };
            open[index] = true;
        }

        open
    }
}

fn index_of(width: u16, height: u16, coord: Coord) -> Option<usize> {
    if coord.x < 0 || coord.y < 0 || coord.x >= i32::from(width) || coord.y >= i32::from(height) {
        return None;
    }

    Some(coord.y as usize * usize::from(width) + coord.x as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes,
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
    fn retained_base_keeps_heads_but_vacates_tails() {
        let state = state(vec![
            snake("ours", &[(2, 2)]),
            snake("enemy", &[(3, 2), (3, 1)]),
        ]);
        let spatial = SpatialOccupancy::from_state(&state);

        assert!(spatial.retained_contains(Coord { x: 3, y: 2 }));
        assert!(!spatial.retained_contains(Coord { x: 3, y: 1 }));
    }

    #[test]
    fn territory_view_releases_all_alive_heads() {
        let state = state(vec![
            snake("ours", &[(2, 2)]),
            snake("enemy", &[(3, 2), (3, 1)]),
        ]);
        let spatial = SpatialOccupancy::from_state(&state);
        let open = spatial.territory_open(&state);
        let enemy_head = index_of(7, 7, Coord { x: 3, y: 2 }).unwrap();

        assert!(open[enemy_head]);
    }
}
