#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};

use crate::direction::Direction;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

const MAX_CHOKES_PER_SNAKE: usize = 8;
const COMPETITIVE_HORIZON: u16 = 5;
const EMPTY_CONTROL_WEIGHT: u32 = 5;
const FOOD_CONTROL_WEIGHT: u32 = 20;
const HAZARD_CONTROL_WEIGHT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChokePoint {
    pub(crate) coord: Coord,
    pub(crate) distance: u16,
    pub(crate) trapped_space: u32,
    pub(crate) cut_gain: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompetitiveTerritorySnapshot {
    pub(crate) snake_id: String,
    pub(crate) controlled_cells: u16,
    pub(crate) controlled_weight: u32,
    pub(crate) contested_cells: u16,
    pub(crate) contested_weight: u32,
    pub(crate) control_ratio_milli: u16,
    pub(crate) controlled_food: u8,
    pub(crate) contested_food: u8,
    pub(crate) winning_frontier: u16,
    pub(crate) losing_frontier: u16,
    pub(crate) dominance_claim_cells: u16,
    pub(crate) dominance_frontier_cells: u16,
    pub(crate) favorable_head_frontier: u16,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreSnakeTerritorySnapshot {
    snake_id: String,
    reachable_space: u32,
    exclusive_space: u32,
    contested_space: u32,
    escape_frontier: u8,
    edge_distance: u16,
}

#[derive(Debug, Clone, Default)]
struct TerritoryCore {
    width: u16,
    height: u16,
    open: Vec<bool>,
    distances: HashMap<String, Vec<u16>>,
    snakes: HashMap<String, CoreSnakeTerritorySnapshot>,
    competitive: HashMap<String, CompetitiveTerritorySnapshot>,
    competitive_ids: Vec<String>,
    competitive_claims: Vec<CompetitiveClaim>,
}

impl TerritoryCore {
    fn from_state(state: &SimulatedGameState) -> Self {
        let width = state.width as u16;
        let height = state.height as u16;
        let cells = usize::from(width).saturating_mul(usize::from(height));
        if cells == 0 {
            return Self::default();
        }

        let occupied = retained_occupancy(state, width, height);
        let open = (0..cells).map(|index| !occupied[index]).collect::<Vec<_>>();

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

        let (competitive, competitive_claims, competitive_ids) =
            competitive_snapshots(state, width, height, &open, &living, &distances);

        let mut snakes = HashMap::new();
        for (snake, head) in living {
            let (reachable_space, exclusive_space, contested_space) =
                ownership.get(&snake.id).copied().unwrap_or_default();

            snakes.insert(
                snake.id.clone(),
                CoreSnakeTerritorySnapshot {
                    snake_id: snake.id.clone(),
                    reachable_space,
                    exclusive_space,
                    contested_space,
                    escape_frontier: escape_frontier(width, height, &open, head, snake.length()),
                    edge_distance: edge_distance(width, height, head),
                },
            );
        }

        Self {
            width,
            height,
            open,
            distances,
            snakes,
            competitive,
            competitive_ids,
            competitive_claims,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TerritoryStructural {
    useful_chokes: HashMap<String, Vec<ChokePoint>>,
}

impl TerritoryStructural {
    fn from_core(state: &SimulatedGameState, core: &TerritoryCore) -> Self {
        if core.open.is_empty() {
            return Self::default();
        }

        let articulation = articulation_points(core.width, core.height, &core.open);
        let mut useful_chokes_by_snake = HashMap::new();

        for snake in state.snakes.iter().filter(|snake| snake.alive) {
            let Some(head) = snake.head() else {
                continue;
            };
            let Some(field) = core.distances.get(&snake.id) else {
                continue;
            };
            let Some(snapshot) = core.snakes.get(&snake.id) else {
                continue;
            };

            let mut useful_chokes = articulation
                .iter()
                .filter_map(|coord| {
                    let index = index_of(core.width, core.height, *coord)?;
                    let distance = field[index];
                    if distance == u16::MAX || *coord == head || distance > 8 {
                        return None;
                    }

                    let trapped_space = reachable_count(
                        core.width,
                        core.height,
                        &core.open,
                        head,
                        Some(*coord),
                    );
                    let cut_gain = snapshot.reachable_space.saturating_sub(trapped_space);
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

            useful_chokes_by_snake.insert(snake.id.clone(), useful_chokes);
        }

        Self {
            useful_chokes: useful_chokes_by_snake,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TerritoryAnalysis {
    width: u16,
    height: u16,
    snakes: HashMap<String, SnakeTerritorySnapshot>,
    competitive: HashMap<String, CompetitiveTerritorySnapshot>,
    competitive_ids: Vec<String>,
    competitive_claims: Vec<CompetitiveClaim>,
}

impl TerritoryAnalysis {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let core = TerritoryCore::from_state(state);
        let structural = TerritoryStructural::from_core(state, &core);
        Self::from_parts(core, structural)
    }

    fn from_parts(core: TerritoryCore, mut structural: TerritoryStructural) -> Self {
        let TerritoryCore {
            width,
            height,
            snakes: core_snakes,
            competitive,
            competitive_ids,
            competitive_claims,
            ..
        } = core;

        let snakes = core_snakes
            .into_iter()
            .map(|(snake_id, snapshot)| {
                let useful_chokes = structural
                    .useful_chokes
                    .remove(&snake_id)
                    .unwrap_or_default();
                (
                    snake_id,
                    SnakeTerritorySnapshot {
                        snake_id: snapshot.snake_id,
                        reachable_space: snapshot.reachable_space,
                        exclusive_space: snapshot.exclusive_space,
                        contested_space: snapshot.contested_space,
                        escape_frontier: snapshot.escape_frontier,
                        edge_distance: snapshot.edge_distance,
                        useful_chokes,
                    },
                )
            })
            .collect();

        Self {
            width,
            height,
            snakes,
            competitive,
            competitive_ids,
            competitive_claims,
        }
    }

    pub(crate) fn for_snake(&self, snake_id: &str) -> Option<&SnakeTerritorySnapshot> {
        self.snakes.get(snake_id)
    }

    pub(crate) fn competitive_for_snake(
        &self,
        snake_id: &str,
    ) -> Option<&CompetitiveTerritorySnapshot> {
        self.competitive.get(snake_id)
    }

    pub(crate) fn competitive_owner_at(&self, coord: Coord) -> Option<&str> {
        let index = index_of(self.width, self.height, coord)?;
        match self.competitive_claims.get(index)? {
            CompetitiveClaim::Owned(owner) => self.competitive_ids.get(*owner).map(String::as_str),
            CompetitiveClaim::Unclaimed | CompetitiveClaim::Contested(_) => None,
        }
    }

    pub(crate) fn competitive_is_contested_at(&self, coord: Coord) -> bool {
        let Some(index) = index_of(self.width, self.height, coord) else {
            return false;
        };
        matches!(
            self.competitive_claims.get(index),
            Some(CompetitiveClaim::Contested(_))
        )
    }
}

#[derive(Debug, Clone)]
enum CompetitiveClaim {
    Unclaimed,
    Owned(usize),
    Contested(Vec<usize>),
}

#[derive(Debug, Clone, Default)]
struct CompetitiveBuilder {
    controlled_cells: u16,
    controlled_weight: u32,
    contested_cells: u16,
    contested_weight: u32,
    controlled_food: u8,
    contested_food: u8,
    winning_frontier: u16,
    losing_frontier: u16,
    dominance_claim_cells: u16,
    dominance_frontier_cells: u16,
    favorable_head_frontier: u16,
}

fn competitive_snapshots(
    state: &SimulatedGameState,
    width: u16,
    height: u16,
    open: &[bool],
    living: &[(&crate::simulation::state::SimulatedSnake, Coord)],
    distances: &HashMap<String, Vec<u16>>,
) -> (
    HashMap<String, CompetitiveTerritorySnapshot>,
    Vec<CompetitiveClaim>,
    Vec<String>,
) {
    let cells = open.len();
    let mut claims = vec![CompetitiveClaim::Unclaimed; cells];
    let mut dominance_owner = vec![None; cells];
    let mut favorable_head_owner = vec![None; cells];

    for (snake_index, (snake, _)) in living.iter().enumerate() {
        for segment in snake.body.iter().take(snake.body.len().saturating_sub(1)) {
            if let Some(index) = index_of(width, height, *segment) {
                claims[index] = CompetitiveClaim::Owned(snake_index);
            }
        }
    }

    for (index, is_open) in open.iter().copied().enumerate() {
        if !is_open {
            continue;
        }

        let mut best_distance = u16::MAX;
        let mut arrivals = Vec::new();
        for (snake_index, (snake, _)) in living.iter().enumerate() {
            let distance = distances
                .get(&snake.id)
                .and_then(|field| field.get(index))
                .copied()
                .unwrap_or(u16::MAX);
            if distance > COMPETITIVE_HORIZON {
                continue;
            }
            if distance < best_distance {
                best_distance = distance;
                arrivals.clear();
                arrivals.push(snake_index);
            } else if distance == best_distance {
                arrivals.push(snake_index);
            }
        }

        if arrivals.is_empty() {
            continue;
        }

        let arrival_count = arrivals.len();
        let best_length = arrivals
            .iter()
            .map(|snake_index| living[*snake_index].0.length())
            .max()
            .unwrap_or(0);
        let winners = arrivals
            .into_iter()
            .filter(|snake_index| living[*snake_index].0.length() == best_length)
            .collect::<Vec<_>>();

        if winners.len() == 1 && arrival_count > 1 {
            dominance_owner[index] = Some(winners[0]);
            if best_distance == 1 {
                favorable_head_owner[index] = Some(winners[0]);
            }
        }

        claims[index] = if winners.len() == 1 {
            CompetitiveClaim::Owned(winners[0])
        } else {
            CompetitiveClaim::Contested(winners)
        };
    }

    let mut builders = vec![CompetitiveBuilder::default(); living.len()];
    let mut total_weight = 0_u32;

    for (index, claim) in claims.iter().enumerate() {
        let coord = coord_of(width, index);
        let weight = control_weight(state, coord);
        match claim {
            CompetitiveClaim::Unclaimed => {}
            CompetitiveClaim::Owned(owner) => {
                total_weight = total_weight.saturating_add(weight);
                let builder = &mut builders[*owner];
                builder.controlled_cells = builder.controlled_cells.saturating_add(1);
                builder.controlled_weight = builder.controlled_weight.saturating_add(weight);
                if state.food.contains(&coord) {
                    builder.controlled_food = builder.controlled_food.saturating_add(1);
                }
                if dominance_owner[index] == Some(*owner) {
                    builder.dominance_claim_cells = builder.dominance_claim_cells.saturating_add(1);
                }
                if favorable_head_owner[index] == Some(*owner) {
                    builder.favorable_head_frontier =
                        builder.favorable_head_frontier.saturating_add(1);
                }
            }
            CompetitiveClaim::Contested(winners) => {
                total_weight = total_weight.saturating_add(weight);
                for winner in winners {
                    let builder = &mut builders[*winner];
                    builder.contested_cells = builder.contested_cells.saturating_add(1);
                    builder.contested_weight = builder.contested_weight.saturating_add(weight);
                    if state.food.contains(&coord) {
                        builder.contested_food = builder.contested_food.saturating_add(1);
                    }
                }
            }
        }
    }

    for (index, claim) in claims.iter().enumerate() {
        let CompetitiveClaim::Owned(owner) = claim else {
            continue;
        };
        let coord = coord_of(width, index);
        let own_length = living[*owner].0.length();
        let mut winning = false;
        let mut losing = false;
        let mut dominance_frontier = false;

        for direction in Direction::ALL {
            let neighbor = direction.apply(coord);
            let Some(neighbor_index) = index_of(width, height, neighbor) else {
                continue;
            };
            match &claims[neighbor_index] {
                CompetitiveClaim::Owned(other) if other != owner => {
                    let other_length = living[*other].0.length();
                    winning |= own_length > other_length;
                    losing |= own_length < other_length;
                    dominance_frontier |= dominance_owner[index] == Some(*owner);
                }
                CompetitiveClaim::Contested(winners) => {
                    dominance_frontier |= dominance_owner[index] == Some(*owner)
                        && winners.iter().any(|winner| winner != owner);
                }
                CompetitiveClaim::Unclaimed | CompetitiveClaim::Owned(_) => {}
            }
        }

        if winning {
            builders[*owner].winning_frontier = builders[*owner].winning_frontier.saturating_add(1);
        }
        if losing {
            builders[*owner].losing_frontier = builders[*owner].losing_frontier.saturating_add(1);
        }
        if dominance_frontier {
            builders[*owner].dominance_frontier_cells =
                builders[*owner].dominance_frontier_cells.saturating_add(1);
        }
    }

    let snapshots = living
        .iter()
        .enumerate()
        .map(|(index, (snake, _))| {
            let builder = &builders[index];
            let control_ratio_milli = if total_weight == 0 {
                0
            } else {
                builder
                    .controlled_weight
                    .saturating_mul(1000)
                    .saturating_div(total_weight)
                    .min(1000)
                    .try_into()
                    .unwrap_or(1000)
            };

            (
                snake.id.clone(),
                CompetitiveTerritorySnapshot {
                    snake_id: snake.id.clone(),
                    controlled_cells: builder.controlled_cells,
                    controlled_weight: builder.controlled_weight,
                    contested_cells: builder.contested_cells,
                    contested_weight: builder.contested_weight,
                    control_ratio_milli,
                    controlled_food: builder.controlled_food,
                    contested_food: builder.contested_food,
                    winning_frontier: builder.winning_frontier,
                    losing_frontier: builder.losing_frontier,
                    dominance_claim_cells: builder.dominance_claim_cells,
                    dominance_frontier_cells: builder.dominance_frontier_cells,
                    favorable_head_frontier: builder.favorable_head_frontier,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let ids = living
        .iter()
        .map(|(snake, _)| snake.id.clone())
        .collect::<Vec<_>>();

    (snapshots, claims, ids)
}

fn control_weight(state: &SimulatedGameState, coord: Coord) -> u32 {
    if state.food.contains(&coord) {
        FOOD_CONTROL_WEIGHT
    } else if state.hazards.contains(&coord) {
        HAZARD_CONTROL_WEIGHT
    } else {
        EMPTY_CONTROL_WEIGHT
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
    fn competitive_tie_is_won_by_longer_snake() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 3), (1, 2), (1, 1), (0, 1)]),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
        );

        let territory = TerritoryAnalysis::from_state(&state);
        let ours = territory.competitive_for_snake("ours").unwrap();
        let enemy = territory.competitive_for_snake("enemy").unwrap();

        assert!(ours.controlled_weight > enemy.controlled_weight);
        assert!(ours.control_ratio_milli > enemy.control_ratio_milli);
        assert!(ours.dominance_claim_cells > 0);
    }

    #[test]
    fn competitive_owner_lookup_uses_length_tiebreak() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 3), (1, 2), (1, 1), (0, 1)]),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
        );

        let territory = TerritoryAnalysis::from_state(&state);

        assert_eq!(
            territory.competitive_owner_at(Coord { x: 3, y: 3 }),
            Some("ours")
        );
        assert!(!territory.competitive_is_contested_at(Coord { x: 3, y: 3 }));
    }

    #[test]
    fn immediate_equal_eta_cell_is_favorable_head_frontier_for_longer_snake() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(2, 1), (1, 1), (1, 0), (0, 0)]),
                snake("enemy", &[(2, 3), (3, 3)]),
            ],
        );

        let territory = TerritoryAnalysis::from_state(&state);
        let ours = territory.competitive_for_snake("ours").unwrap();
        let enemy = territory.competitive_for_snake("enemy").unwrap();

        assert!(ours.favorable_head_frontier >= 1);
        assert_eq!(enemy.favorable_head_frontier, 0);
    }

    #[test]
    fn competitive_food_is_weighted_as_controlled_territory() {
        let mut state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 3), (1, 2), (1, 1)]),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
        );
        state.food = vec![Coord { x: 2, y: 3 }];

        let territory = TerritoryAnalysis::from_state(&state);
        let ours = territory.competitive_for_snake("ours").unwrap();

        assert_eq!(ours.controlled_food, 1);
        assert!(ours.controlled_weight >= FOOD_CONTROL_WEIGHT);
    }

    #[test]
    fn wall_creates_useful_single_cell_choke() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(6, 6), (6, 5), (6, 4), (5, 4), (4, 4), (3, 4)]),
                snake("enemy", &[(1, 2), (1, 1), (1, 0)]),
                snake(
                    "wall",
                    &[
                        (4, 6),
                        (2, 6),
                        (2, 5),
                        (2, 4),
                        (2, 3),
                        (2, 1),
                        (2, 0),
                        (4, 5),
                    ],
                ),
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

    #[test]
    fn core_and_structural_parts_recompose_public_analysis() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(6, 6), (6, 5), (6, 4), (5, 4), (4, 4), (3, 4)]),
                snake("enemy", &[(1, 2), (1, 1), (1, 0)]),
                snake(
                    "wall",
                    &[
                        (4, 6),
                        (2, 6),
                        (2, 5),
                        (2, 4),
                        (2, 3),
                        (2, 1),
                        (2, 0),
                        (4, 5),
                    ],
                ),
            ],
        );

        let public = TerritoryAnalysis::from_state(&state);
        let core = TerritoryCore::from_state(&state);
        let structural = TerritoryStructural::from_core(&state, &core);
        let recomposed = TerritoryAnalysis::from_parts(core, structural);

        for snake_id in ["ours", "enemy", "wall"] {
            assert_eq!(
                public.for_snake(snake_id),
                recomposed.for_snake(snake_id)
            );
            assert_eq!(
                public.competitive_for_snake(snake_id),
                recomposed.competitive_for_snake(snake_id)
            );
        }

        for y in 0..7 {
            for x in 0..7 {
                let coord = Coord { x, y };
                assert_eq!(
                    public.competitive_owner_at(coord),
                    recomposed.competitive_owner_at(coord)
                );
                assert_eq!(
                    public.competitive_is_contested_at(coord),
                    recomposed.competitive_is_contested_at(coord)
                );
            }
        }
    }

}
