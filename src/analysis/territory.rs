#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};

use crate::direction::Direction;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

const MAX_CHOKES_PER_SNAKE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChokePoint {
    pub(crate) coord: Coord,
    pub(crate) distance: u16,
    pub(crate) trapped_space: u32,
    pub(crate) cut_gain: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnakeTerritorySnapshot {
    pub(crate) snake_id: String,
    pub(crate) reachable_space: u32,
    pub(crate) exclusive_space: u32,
    pub(crate) contested_space: u32,
    pub(crate) escape_frontier: u8,
    pub(crate) edge_distance: u16,
    pub(crate) useful_chokes: Vec<ChokePoint>,
}

impl SnakeTerritorySnapshot {
    pub(crate) fn space_to_length_milli(&self, length: usize) -> u32 {
        if length == 0 {
            return u32::MAX;
        }
        self.reachable_space
            .saturating_mul(1000)
            .saturating_div(length.try_into().unwrap_or(u32::MAX).max(1))
    }

    pub(crate) fn nearest_choke(&self) -> Option<ChokePoint> {
        self.useful_chokes.first().copied()
    }

    pub(crate) fn boundary_support(&self) -> u8 {
        let edge = match self.edge_distance {
            0 => 2,
            1 => 1,
            _ => 0,
        };
        edge + u8::from(!self.useful_chokes.is_empty())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TerritoryAnalysis {
    snakes: HashMap<String, SnakeTerritorySnapshot>,
}

impl TerritoryAnalysis {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let width = state.width as u16;
        let height = state.height as u16;
        let cells = usize::from(width).saturating_mul(usize::from(height));
        if cells == 0 {
            return Self::default();
        }

        let occupied = retained_occupancy(state, width, height);
        let open = (0..cells).map(|index| !occupied[index]).collect::<Vec<_>>();
        let articulation = articulation_points(width, height, &open);

        let living = state
            .snakes
            .iter()
            .filter(|snake| snake.alive)
            .filter_map(|snake| snake.head().map(|head| (snake, head)))
            .collect::<Vec<_>>();

        let mut distances = HashMap::new();
        for (snake, head) in &living {
            distances.insert(
                snake.id.clone(),
                bfs_distances(width, height, &open, *head, None),
            );
        }

        let mut ownership = HashMap::<String, (u32, u32, u32)>::new();
        for (snake, _) in &living {
            ownership.insert(snake.id.clone(), (0, 0, 0));
        }

        for (index, is_open) in open.iter().copied().enumerate().take(cells) {
            if !is_open {
                continue;
            }

            let mut best = u16::MAX;
            let mut winners = Vec::new();
            for (snake, _) in &living {
                let distance = distances
                    .get(&snake.id)
                    .and_then(|field| field.get(index))
                    .copied()
                    .unwrap_or(u16::MAX);
                if distance == u16::MAX {
                    continue;
                }
                if distance < best {
                    best = distance;
                    winners.clear();
                    winners.push(snake.id.as_str());
                } else if distance == best {
                    winners.push(snake.id.as_str());
                }
            }

            for (snake, _) in &living {
                let distance = distances
                    .get(&snake.id)
                    .and_then(|field| field.get(index))
                    .copied()
                    .unwrap_or(u16::MAX);
                if distance != u16::MAX {
                    if let Some(entry) = ownership.get_mut(&snake.id) {
                        entry.0 = entry.0.saturating_add(1);
                    }
                }
            }

            if winners.len() == 1 {
                if let Some(entry) = ownership.get_mut(winners[0]) {
                    entry.1 = entry.1.saturating_add(1);
                }
            } else if winners.len() > 1 {
                for winner in winners {
                    if let Some(entry) = ownership.get_mut(winner) {
                        entry.2 = entry.2.saturating_add(1);
                    }
                }
            }
        }

        let mut snapshots = HashMap::new();
        for (snake, head) in living {
            let field = distances
                .get(&snake.id)
                .expect("living snake must have a distance field");
            let (reachable_space, exclusive_space, contested_space) =
                ownership.get(&snake.id).copied().unwrap_or_default();

            let mut useful_chokes = articulation
                .iter()
                .filter_map(|coord| {
                    let index = index_of(width, height, *coord)?;
                    let distance = field[index];
                    if distance == u16::MAX || *coord == head || distance > 8 {
                        return None;
                    }

                    let trapped_space = reachable_count(width, height, &open, head, Some(*coord));
                    let cut_gain = reachable_space.saturating_sub(trapped_space);
                    let minimum_gain = u32::try_from(snake.length()).unwrap_or(u32::MAX).max(4);
                    (cut_gain >= minimum_gain).then_some(ChokePoint {
                        coord: *coord,
                        distance,
                        trapped_space,
                        cut_gain,
                    })
                })
                .collect::<Vec<_>>();
            useful_chokes.sort_by(|left, right| {
                left.distance
                    .cmp(&right.distance)
                    .then_with(|| right.cut_gain.cmp(&left.cut_gain))
                    .then_with(|| left.coord.cmp(&right.coord))
            });
            useful_chokes.truncate(MAX_CHOKES_PER_SNAKE);

            snapshots.insert(
                snake.id.clone(),
                SnakeTerritorySnapshot {
                    snake_id: snake.id.clone(),
                    reachable_space,
                    exclusive_space,
                    contested_space,
                    escape_frontier: escape_frontier(width, height, &open, head, snake.length()),
                    edge_distance: edge_distance(width, height, head),
                    useful_chokes,
                },
            );
        }

        Self { snakes: snapshots }
    }

    pub(crate) fn for_snake(&self, snake_id: &str) -> Option<&SnakeTerritorySnapshot> {
        self.snakes.get(snake_id)
    }
}

fn retained_occupancy(state: &SimulatedGameState, width: u16, height: u16) -> Vec<bool> {
    let mut occupied = vec![false; usize::from(width).saturating_mul(usize::from(height))];

    for snake in state.snakes.iter().filter(|snake| snake.alive) {
        let retained_len = snake.body.len().saturating_sub(1);
        for segment in snake.body.iter().take(retained_len) {
            if let Some(index) = index_of(width, height, *segment) {
                occupied[index] = true;
            }
        }
    }

    for snake in state.snakes.iter().filter(|snake| snake.alive) {
        if let Some(head) = snake.head() {
            if let Some(index) = index_of(width, height, head) {
                occupied[index] = false;
            }
        }
    }

    occupied
}

fn bfs_distances(
    width: u16,
    height: u16,
    open: &[bool],
    start: Coord,
    blocked: Option<Coord>,
) -> Vec<u16> {
    let mut distances = vec![u16::MAX; open.len()];
    let Some(start_index) = index_of(width, height, start) else {
        return distances;
    };
    if !open[start_index] || blocked == Some(start) {
        return distances;
    }

    let mut queue = VecDeque::from([start]);
    distances[start_index] = 0;

    while let Some(current) = queue.pop_front() {
        let current_index = index_of(width, height, current).expect("queued coord must be valid");
        let next_distance = distances[current_index].saturating_add(1);

        for direction in Direction::ALL {
            let next = direction.apply(current);
            if blocked == Some(next) {
                continue;
            }
            let Some(next_index) = index_of(width, height, next) else {
                continue;
            };
            if !open[next_index] || distances[next_index] != u16::MAX {
                continue;
            }
            distances[next_index] = next_distance;
            queue.push_back(next);
        }
    }

    distances
}

fn reachable_count(
    width: u16,
    height: u16,
    open: &[bool],
    start: Coord,
    blocked: Option<Coord>,
) -> u32 {
    bfs_distances(width, height, open, start, blocked)
        .into_iter()
        .filter(|distance| *distance != u16::MAX)
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

fn escape_frontier(width: u16, height: u16, open: &[bool], head: Coord, length: usize) -> u8 {
    let comfortable_space = u32::try_from(length)
        .unwrap_or(u32::MAX)
        .saturating_mul(2)
        .max(4);

    Direction::ALL
        .into_iter()
        .filter(|direction| {
            let next = direction.apply(head);
            let Some(index) = index_of(width, height, next) else {
                return false;
            };
            open[index]
                && reachable_count(width, height, open, next, Some(head)) >= comfortable_space
        })
        .count()
        .try_into()
        .unwrap_or(u8::MAX)
}

fn articulation_points(width: u16, height: u16, open: &[bool]) -> HashSet<Coord> {
    struct Tarjan<'a> {
        width: u16,
        height: u16,
        open: &'a [bool],
        time: u16,
        disc: Vec<u16>,
        low: Vec<u16>,
        parent: Vec<Option<usize>>,
        points: HashSet<Coord>,
    }

    fn visit(tarjan: &mut Tarjan<'_>, current: Coord) {
        let current_index =
            index_of(tarjan.width, tarjan.height, current).expect("valid DFS coordinate");
        tarjan.time = tarjan.time.saturating_add(1);
        tarjan.disc[current_index] = tarjan.time;
        tarjan.low[current_index] = tarjan.time;
        let mut children = 0_u8;

        for direction in Direction::ALL {
            let next = direction.apply(current);
            let Some(next_index) = index_of(tarjan.width, tarjan.height, next) else {
                continue;
            };
            if !tarjan.open[next_index] {
                continue;
            }

            if tarjan.disc[next_index] == 0 {
                children = children.saturating_add(1);
                tarjan.parent[next_index] = Some(current_index);
                visit(tarjan, next);
                tarjan.low[current_index] = tarjan.low[current_index].min(tarjan.low[next_index]);

                let is_root = tarjan.parent[current_index].is_none();
                if (is_root && children > 1)
                    || (!is_root && tarjan.low[next_index] >= tarjan.disc[current_index])
                {
                    tarjan.points.insert(current);
                }
            } else if tarjan.parent[current_index] != Some(next_index) {
                tarjan.low[current_index] = tarjan.low[current_index].min(tarjan.disc[next_index]);
            }
        }
    }

    let mut tarjan = Tarjan {
        width,
        height,
        open,
        time: 0,
        disc: vec![0; open.len()],
        low: vec![0; open.len()],
        parent: vec![None; open.len()],
        points: HashSet::new(),
    };

    for (index, is_open) in open.iter().copied().enumerate() {
        if !is_open || tarjan.disc[index] != 0 {
            continue;
        }
        let coord = coord_of(width, index);
        visit(&mut tarjan, coord);
    }

    tarjan.points
}

fn edge_distance(width: u16, height: u16, coord: Coord) -> u16 {
    let right = i32::from(width).saturating_sub(1).saturating_sub(coord.x);
    let top = i32::from(height).saturating_sub(1).saturating_sub(coord.y);
    coord
        .x
        .min(coord.y)
        .min(right)
        .min(top)
        .max(0)
        .try_into()
        .unwrap_or(u16::MAX)
}

fn index_of(width: u16, height: u16, coord: Coord) -> Option<usize> {
    if coord.x < 0 || coord.y < 0 || coord.x >= i32::from(width) || coord.y >= i32::from(height) {
        return None;
    }

    Some(coord.y as usize * usize::from(width) + coord.x as usize)
}

fn coord_of(width: u16, index: usize) -> Coord {
    Coord {
        x: (index % usize::from(width)) as i32,
        y: (index / usize::from(width)) as i32,
    }
}

#[cfg(test)]
mod tests {
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};

    use super::*;

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(width: u32, height: u32, snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width,
            height,
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
    fn open_board_partitions_space_between_snakes() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 3), (1, 2), (1, 1)]),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
        );

        let territory = TerritoryAnalysis::from_state(&state);
        let ours = territory.for_snake("ours").unwrap();
        let enemy = territory.for_snake("enemy").unwrap();

        assert!(ours.reachable_space > 0);
        assert!(enemy.reachable_space > 0);
        assert!(ours.exclusive_space > 0);
        assert!(enemy.exclusive_space > 0);
        assert!(ours.contested_space > 0);
    }

    #[test]
    fn wall_creates_useful_single_cell_choke() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(6, 6), (6, 5), (6, 4), (5, 4), (4, 4), (3, 4)]),
                snake("enemy", &[(1, 2), (1, 1), (1, 0)]),
                snake("wall", &[(2, 6), (2, 5), (2, 4), (2, 3), (2, 1), (2, 0)]),
            ],
        );

        let territory = TerritoryAnalysis::from_state(&state);
        let enemy = territory.for_snake("enemy").unwrap();

        assert!(enemy
            .useful_chokes
            .iter()
            .any(|choke| choke.coord == Coord { x: 2, y: 2 }));
    }

    #[test]
    fn edge_distance_is_zero_on_wall() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(0, 3), (0, 2), (0, 1)]),
                snake("enemy", &[(5, 5), (5, 4)]),
            ],
        );

        let territory = TerritoryAnalysis::from_state(&state);
        assert_eq!(territory.for_snake("ours").unwrap().edge_distance, 0);
    }
}
