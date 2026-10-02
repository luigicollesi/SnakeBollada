#![allow(dead_code)]

use std::collections::{HashSet, VecDeque};

use rayon::prelude::*;

use crate::direction::Direction;
use crate::evaluation::ActorVec;
use crate::simulation::state::{ActorIndex, SimulatedGameState};
use crate::spatial::SpatialOccupancy;
use crate::Coord;

const MAX_CHOKES_PER_SNAKE: usize = 8;
const COMPETITIVE_HORIZON: u16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChokePoint {
    coord: Coord,
    distance: u16,
    trapped_space: u32,
    cut_gain: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnakeTerritorySnapshot {
    pub(crate) snake_id: String,
    pub(crate) reachable_space: u32,
    pub(crate) exclusive_space: u32,
    pub(crate) contested_space: u32,
    pub(crate) competitive_control_milli: u16,
    pub(crate) escape_frontier: u8,
    pub(crate) edge_distance: u16,
    pub(crate) useful_choke_count: u8,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreSnakeTerritorySnapshot {
    snake_id: String,
    reachable_space: u32,
    exclusive_space: u32,
    contested_space: u32,
    competitive_control_milli: u16,
    escape_frontier: u8,
    edge_distance: u16,
}

#[derive(Debug, Clone, Default)]
struct TerritoryCore {
    width: u16,
    height: u16,
    open: Vec<bool>,
    distances: ActorVec<Vec<u16>>,
    snakes: ActorVec<CoreSnakeTerritorySnapshot>,
    competitive_claims: Vec<CompetitiveClaim>,
}

impl TerritoryCore {
    fn from_state(state: &SimulatedGameState) -> Self {
        let spatial = SpatialOccupancy::from_state(state);
        Self::from_spatial(state, &spatial)
    }

    fn from_spatial(state: &SimulatedGameState, spatial: &SpatialOccupancy) -> Self {
        let width = spatial.width();
        let height = spatial.height();
        let cells = usize::from(width).saturating_mul(usize::from(height));
        if cells == 0 {
            return Self::default();
        }

        let open = spatial.territory_open(state);

        let living = state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
            .filter_map(|(index, snake)| Some((ActorIndex::new(index)?, snake, snake.head()?)))
            .collect::<Vec<_>>();

        let distances = living
            .par_iter()
            .map(|(actor, _, head)| (*actor, bfs_distances(width, height, &open, *head, None)))
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<ActorVec<_>>();

        let mut ownership = ActorVec::with_capacity(state.snakes.len());
        for (actor, _, _) in &living {
            ownership.insert(*actor, (0_u32, 0_u32, 0_u32));
        }

        for (index, is_open) in open.iter().copied().enumerate().take(cells) {
            if !is_open {
                continue;
            }

            let mut best = u16::MAX;
            let mut winners = Vec::new();
            for (actor, _, _) in &living {
                let distance = distances
                    .get(*actor)
                    .and_then(|field| field.get(index))
                    .copied()
                    .unwrap_or(u16::MAX);
                if distance == u16::MAX {
                    continue;
                }
                if distance < best {
                    best = distance;
                    winners.clear();
                    winners.push(*actor);
                } else if distance == best {
                    winners.push(*actor);
                }
            }

            for (actor, _, _) in &living {
                let distance = distances
                    .get(*actor)
                    .and_then(|field| field.get(index))
                    .copied()
                    .unwrap_or(u16::MAX);
                if distance != u16::MAX {
                    if let Some(entry) = ownership.get_mut(*actor) {
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

        let competitive_claims =
            competitive_claims(state, width, height, &open, &living, &distances);
        let competitive_controls =
            competitive_control_by_actor(state.snakes.len(), &open, &competitive_claims);

        let mut snakes = ActorVec::with_capacity(state.snakes.len());
        for (actor, snake, head) in living {
            let (reachable_space, exclusive_space, contested_space) =
                ownership.get(actor).copied().unwrap_or_default();

            snakes.insert(
                actor,
                CoreSnakeTerritorySnapshot {
                    snake_id: snake.id.clone(),
                    reachable_space,
                    exclusive_space,
                    contested_space,
                    competitive_control_milli: competitive_controls
                        .get(actor)
                        .copied()
                        .unwrap_or(0),
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
            competitive_claims,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TerritoryStructural {
    useful_choke_counts: ActorVec<u8>,
}

impl TerritoryStructural {
    fn from_core_actor_relative(state: &SimulatedGameState, core: &TerritoryCore) -> Self {
        if core.open.is_empty() {
            return Self::default();
        }

        let relevant = state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
            .filter_map(|(index, _)| {
                let actor = ActorIndex::new(index)?;
                let snapshot = core.snakes.get(actor)?;
                structural_relevant(
                    state,
                    snapshot.exclusive_space,
                    snapshot.contested_space,
                    snapshot.escape_frontier,
                )
                .then_some(actor)
            })
            .collect::<Vec<_>>();

        if relevant.is_empty() {
            return Self::default();
        }

        let articulation = articulation_points(core.width, core.height, &core.open);
        let useful_choke_counts = state
            .snakes
            .par_iter()
            .enumerate()
            .filter_map(|(index, snake)| {
                let actor = ActorIndex::new(index)?;
                if !snake.alive || !relevant.contains(&actor) {
                    return None;
                }
                useful_choke_count_for_actor(actor, snake, core, &articulation)
                    .map(|count| (actor, count))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<ActorVec<_>>();

        Self {
            useful_choke_counts,
        }
    }

    fn from_core(state: &SimulatedGameState, core: &TerritoryCore) -> Self {
        if core.open.is_empty() {
            return Self::default();
        }

        let articulation = articulation_points(core.width, core.height, &core.open);
        let useful_choke_counts = state
            .snakes
            .par_iter()
            .enumerate()
            .filter_map(|(index, snake)| {
                let actor = ActorIndex::new(index)?;
                if !snake.alive {
                    return None;
                }
                useful_choke_count_for_actor(actor, snake, core, &articulation)
                    .map(|count| (actor, count))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<ActorVec<_>>();

        Self {
            useful_choke_counts,
        }
    }
}

fn useful_choke_count_for_actor(
    actor: ActorIndex,
    snake: &crate::simulation::state::SimulatedSnake,
    core: &TerritoryCore,
    articulation: &HashSet<Coord>,
) -> Option<u8> {
    let head = snake.head()?;
    let field = core.distances.get(actor)?;
    let snapshot = core.snakes.get(actor)?;

    let mut useful_chokes = articulation
        .iter()
        .filter_map(|coord| {
            let index = index_of(core.width, core.height, *coord)?;
            let distance = field[index];
            if distance == u16::MAX || *coord == head || distance > 8 {
                return None;
            }

            let trapped_space =
                reachable_count(core.width, core.height, &core.open, head, Some(*coord));
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
    Some(useful_chokes.len().try_into().unwrap_or(u8::MAX))
}

fn structural_relevant(
    state: &SimulatedGameState,
    exclusive_space: u32,
    contested_space: u32,
    escape_frontier: u8,
) -> bool {
    if escape_frontier <= 1 {
        return true;
    }

    let board_cells = state.width.saturating_mul(state.height).max(1);
    let effective_control = exclusive_space.saturating_add(contested_space / 2);
    let share_milli = effective_control
        .saturating_mul(1000)
        .saturating_div(board_cells)
        .min(1000);

    share_milli <= 400
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TerritoryAnalysis {
    width: u16,
    height: u16,
    open: Vec<bool>,
    distances: ActorVec<Vec<u16>>,
    snakes: ActorVec<SnakeTerritorySnapshot>,
    competitive_claims: Vec<CompetitiveClaim>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompetitiveTransition {
    pub(crate) captures: ActorVec<u32>,
    pub(crate) denials: ActorVec<u32>,
}

impl TerritoryAnalysis {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let spatial = SpatialOccupancy::from_state(state);
        Self::from_spatial(state, &spatial)
    }

    pub(crate) fn from_spatial(state: &SimulatedGameState, spatial: &SpatialOccupancy) -> Self {
        let core = TerritoryCore::from_spatial(state, spatial);
        let structural = TerritoryStructural::from_core(state, &core);
        Self::from_parts(state, core, structural)
    }

    pub(crate) fn from_spatial_actor_relative(
        state: &SimulatedGameState,
        spatial: &SpatialOccupancy,
    ) -> Self {
        let core = TerritoryCore::from_spatial(state, spatial);
        let structural = TerritoryStructural::from_core_actor_relative(state, &core);
        Self::from_parts(state, core, structural)
    }

    fn from_parts(
        state: &SimulatedGameState,
        core: TerritoryCore,
        mut structural: TerritoryStructural,
    ) -> Self {
        let TerritoryCore {
            width,
            height,
            open,
            distances,
            snakes: core_snakes,
            competitive_claims,
        } = core;

        let mut snakes = ActorVec::with_capacity(state.snakes.len());
        for (actor, snapshot) in core_snakes.iter() {
            let useful_choke_count = structural
                .useful_choke_counts
                .take(actor)
                .unwrap_or_default();
            snakes.insert(
                actor,
                SnakeTerritorySnapshot {
                    snake_id: snapshot.snake_id.clone(),
                    reachable_space: snapshot.reachable_space,
                    exclusive_space: snapshot.exclusive_space,
                    contested_space: snapshot.contested_space,
                    competitive_control_milli: snapshot.competitive_control_milli,
                    escape_frontier: snapshot.escape_frontier,
                    edge_distance: snapshot.edge_distance,
                    useful_choke_count,
                },
            );
        }

        Self {
            width,
            height,
            open,
            distances,
            snakes,
            competitive_claims,
        }
    }

    fn actor_for_id(&self, snake_id: &str) -> Option<ActorIndex> {
        self.snakes
            .iter()
            .find_map(|(actor, snapshot)| (snapshot.snake_id == snake_id).then_some(actor))
    }

    pub(crate) fn distance_for_actor(&self, actor: ActorIndex, coord: Coord) -> Option<u16> {
        let index = index_of(self.width, self.height, coord)?;
        let distance = *self.distances.get(actor)?.get(index)?;
        (distance != u16::MAX).then_some(distance)
    }

    pub(crate) fn distance_for(&self, snake_id: &str, coord: Coord) -> Option<u16> {
        let actor = self.actor_for_id(snake_id)?;
        self.distance_for_actor(actor, coord)
    }

    pub(crate) fn for_actor(&self, actor: ActorIndex) -> Option<&SnakeTerritorySnapshot> {
        self.snakes.get(actor)
    }

    pub(crate) fn for_snake(&self, snake_id: &str) -> Option<&SnakeTerritorySnapshot> {
        self.actor_for_id(snake_id)
            .and_then(|actor| self.for_actor(actor))
    }

    pub(crate) fn competitive_control_milli(&self, actor: ActorIndex) -> u16 {
        self.for_actor(actor)
            .map_or(0, |snapshot| snapshot.competitive_control_milli)
    }

    pub(crate) fn competitive_transition_to(&self, child: &Self) -> CompetitiveTransition {
        let mut transition = CompetitiveTransition::default();
        let cells = self
            .competitive_claims
            .len()
            .min(child.competitive_claims.len())
            .min(self.open.len())
            .min(child.open.len());

        for index in 0..cells {
            if !self.open[index] || !child.open[index] {
                continue;
            }

            let CompetitiveClaim::Owned(previous_owner) = self.competitive_claims[index] else {
                continue;
            };

            match child.competitive_claims[index] {
                CompetitiveClaim::Owned(new_owner) if new_owner != previous_owner => {
                    transition.captures.add(new_owner, 1);
                }
                CompetitiveClaim::Contested(contenders) => {
                    for contender in contenders.iter() {
                        if contender != previous_owner {
                            transition.denials.add(contender, 1);
                        }
                    }
                }
                CompetitiveClaim::Unclaimed
                | CompetitiveClaim::Owned(_)
                | CompetitiveClaim::Contested(_) => {}
            }
        }

        transition
    }

    pub(crate) fn competitive_owner_actor_at(&self, coord: Coord) -> Option<ActorIndex> {
        let index = index_of(self.width, self.height, coord)?;
        match self.competitive_claims.get(index)? {
            CompetitiveClaim::Owned(owner) => Some(*owner),
            CompetitiveClaim::Unclaimed | CompetitiveClaim::Contested(_) => None,
        }
    }

    pub(crate) fn competitive_owner_at(&self, coord: Coord) -> Option<&str> {
        let owner = self.competitive_owner_actor_at(coord)?;
        self.snakes
            .get(owner)
            .map(|snapshot| snapshot.snake_id.as_str())
    }

    pub(crate) fn competitive_contested_by_actor(&self, coord: Coord, actor: ActorIndex) -> bool {
        let Some(index) = index_of(self.width, self.height, coord) else {
            return false;
        };
        match self.competitive_claims.get(index) {
            Some(CompetitiveClaim::Contested(contenders)) => contenders.contains(actor),
            _ => false,
        }
    }

    pub(crate) fn competitive_contested_by(&self, coord: Coord, actor_id: &str) -> bool {
        let Some(actor) = self.actor_for_id(actor_id) else {
            return false;
        };
        self.competitive_contested_by_actor(coord, actor)
    }

    pub(crate) fn competitive_contender_count_at(&self, coord: Coord) -> usize {
        let Some(index) = index_of(self.width, self.height, coord) else {
            return 0;
        };
        match self.competitive_claims.get(index) {
            Some(CompetitiveClaim::Contested(contenders)) => contenders.len(),
            _ => 0,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActorMask(u16);

impl ActorMask {
    fn insert(&mut self, actor: ActorIndex) {
        let bit = u32::try_from(actor.as_usize()).unwrap_or(u32::MAX);
        if bit < u16::BITS {
            self.0 |= 1_u16 << bit;
        }
    }

    fn contains(self, actor: ActorIndex) -> bool {
        let bit = u32::try_from(actor.as_usize()).unwrap_or(u32::MAX);
        bit < u16::BITS && (self.0 & (1_u16 << bit)) != 0
    }

    fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    fn iter(self) -> impl Iterator<Item = ActorIndex> {
        (0..u16::BITS).filter_map(move |bit| {
            ((self.0 & (1_u16 << bit)) != 0)
                .then(|| ActorIndex::new(bit as usize))
                .flatten()
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum CompetitiveClaim {
    Unclaimed,
    Owned(ActorIndex),
    Contested(ActorMask),
}

fn competitive_claims(
    state: &SimulatedGameState,
    width: u16,
    height: u16,
    open: &[bool],
    living: &[(ActorIndex, &crate::simulation::state::SimulatedSnake, Coord)],
    distances: &ActorVec<Vec<u16>>,
) -> Vec<CompetitiveClaim> {
    let mut claims = vec![CompetitiveClaim::Unclaimed; open.len()];

    for (actor, snake, _) in living {
        for segment in snake.body.iter().take(snake.body.len().saturating_sub(1)) {
            if let Some(index) = index_of(width, height, *segment) {
                claims[index] = CompetitiveClaim::Owned(*actor);
            }
        }
    }

    for (index, is_open) in open.iter().copied().enumerate() {
        if !is_open {
            continue;
        }

        let mut best_distance = u16::MAX;
        let mut arrivals = Vec::new();
        for (actor, _, _) in living {
            let distance = distances
                .get(*actor)
                .and_then(|field| field.get(index))
                .copied()
                .unwrap_or(u16::MAX);
            if distance > COMPETITIVE_HORIZON {
                continue;
            }
            if distance < best_distance {
                best_distance = distance;
                arrivals.clear();
                arrivals.push(*actor);
            } else if distance == best_distance {
                arrivals.push(*actor);
            }
        }

        if arrivals.is_empty() {
            continue;
        }

        let best_length = arrivals
            .iter()
            .filter_map(|actor| state.snake_at(*actor))
            .map(|snake| snake.length())
            .max()
            .unwrap_or(0);
        let winners = arrivals
            .into_iter()
            .filter(|actor| {
                state
                    .snake_at(*actor)
                    .is_some_and(|snake| snake.length() == best_length)
            })
            .collect::<Vec<_>>();

        claims[index] = if winners.len() == 1 {
            CompetitiveClaim::Owned(winners[0])
        } else {
            let mut mask = ActorMask::default();
            for actor in winners {
                mask.insert(actor);
            }
            CompetitiveClaim::Contested(mask)
        };
    }

    claims
}

fn competitive_control_by_actor(
    actor_capacity: usize,
    open: &[bool],
    claims: &[CompetitiveClaim],
) -> ActorVec<u16> {
    let mut units = vec![0_u32; actor_capacity];
    let mut relevant_units = 0_u32;

    for (index, claim) in claims.iter().enumerate() {
        if !open.get(index).copied().unwrap_or(false) {
            continue;
        }

        match claim {
            CompetitiveClaim::Unclaimed => {}
            CompetitiveClaim::Owned(owner) => {
                relevant_units = relevant_units.saturating_add(1000);
                if let Some(actor_units) = units.get_mut(owner.as_usize()) {
                    *actor_units = actor_units.saturating_add(1000);
                }
            }
            CompetitiveClaim::Contested(contenders) => {
                relevant_units = relevant_units.saturating_add(1000);
                let count = u32::try_from(contenders.len()).unwrap_or(u32::MAX).max(1);
                let split = 1000_u32.saturating_div(count);
                for actor in contenders.iter() {
                    if let Some(actor_units) = units.get_mut(actor.as_usize()) {
                        *actor_units = actor_units.saturating_add(split);
                    }
                }
            }
        }
    }

    if relevant_units == 0 {
        return ActorVec::new();
    }

    units
        .into_iter()
        .enumerate()
        .filter_map(|(index, actor_units)| {
            let actor = ActorIndex::new(index)?;
            let control = actor_units
                .saturating_mul(1000)
                .saturating_div(relevant_units)
                .min(1000)
                .try_into()
                .unwrap_or(1000);
            Some((actor, control))
        })
        .collect()
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
    use crate::simulation::state::{RulesContext, SimulatedSnake};

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
    fn competitive_control_is_relative_to_current_claimed_space() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 3), (1, 2), (1, 1), (0, 1)]),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
        );
        let territory = TerritoryAnalysis::from_state(&state);
        let ours = state.actor_index("ours").unwrap();
        let enemy = state.actor_index("enemy").unwrap();

        let ours_control = territory.competitive_control_milli(ours);
        let enemy_control = territory.competitive_control_milli(enemy);

        assert!(ours_control > enemy_control);
        assert!(ours_control <= 1000);
        assert!(enemy_control <= 1000);
        assert!(ours_control.saturating_add(enemy_control) >= 990);
    }

    #[test]
    fn competitive_control_excludes_occupied_body_cells() {
        let open = vec![false, false, true, true];
        let ours = ActorIndex::new(0).unwrap();
        let enemy = ActorIndex::new(1).unwrap();
        let claims = vec![
            CompetitiveClaim::Owned(ours),
            CompetitiveClaim::Owned(ours),
            CompetitiveClaim::Owned(ours),
            CompetitiveClaim::Owned(enemy),
        ];

        let control = competitive_control_by_actor(2, &open, &claims);

        assert_eq!(control.get(ours).copied(), Some(500));
        assert_eq!(control.get(enemy).copied(), Some(500));
    }

    #[test]
    fn competitive_transition_ignores_cells_not_open_in_both_states() {
        let ours = ActorIndex::new(0).unwrap();
        let enemy = ActorIndex::new(1).unwrap();

        let parent = TerritoryAnalysis {
            width: 2,
            height: 1,
            open: vec![false, true],
            distances: ActorVec::new(),
            snakes: ActorVec::new(),
            competitive_claims: vec![
                CompetitiveClaim::Owned(enemy),
                CompetitiveClaim::Owned(enemy),
            ],
        };
        let child = TerritoryAnalysis {
            width: 2,
            height: 1,
            open: vec![false, true],
            distances: ActorVec::new(),
            snakes: ActorVec::new(),
            competitive_claims: vec![CompetitiveClaim::Owned(ours), CompetitiveClaim::Owned(ours)],
        };

        let delta = parent.competitive_transition_to(&child);

        assert_eq!(delta.captures.get(ours).copied(), Some(1));
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
    fn actor_relative_territory_preserves_core_ownership() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 3), (1, 2), (1, 1)]),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
        );
        let spatial = SpatialOccupancy::from_state(&state);
        let full = TerritoryAnalysis::from_spatial(&state, &spatial);
        let lean = TerritoryAnalysis::from_spatial_actor_relative(&state, &spatial);

        for snake_id in ["ours", "enemy"] {
            let full_snapshot = full.for_snake(snake_id).unwrap();
            let lean_snapshot = lean.for_snake(snake_id).unwrap();

            assert_eq!(lean_snapshot.reachable_space, full_snapshot.reachable_space);
            assert_eq!(lean_snapshot.exclusive_space, full_snapshot.exclusive_space);
            assert_eq!(lean_snapshot.contested_space, full_snapshot.contested_space);
            assert_eq!(lean_snapshot.escape_frontier, full_snapshot.escape_frontier);
            assert_eq!(lean_snapshot.edge_distance, full_snapshot.edge_distance);
        }

        for y in 0..state.height {
            for x in 0..state.width {
                let coord = Coord {
                    x: i32::try_from(x).unwrap(),
                    y: i32::try_from(y).unwrap(),
                };
                assert_eq!(
                    lean.competitive_owner_at(coord),
                    full.competitive_owner_at(coord)
                );
                assert_eq!(
                    lean.competitive_is_contested_at(coord),
                    full.competitive_is_contested_at(coord)
                );
            }
        }
    }

    #[test]
    fn distance_field_is_available_from_territory_core() {
        let state = state(
            7,
            7,
            vec![
                snake("ours", &[(1, 1), (1, 0)]),
                snake("enemy", &[(5, 5), (5, 4)]),
            ],
        );
        let territory = TerritoryAnalysis::from_state(&state);

        assert_eq!(
            territory.distance_for("ours", Coord { x: 3, y: 1 }),
            Some(2)
        );
        assert_eq!(
            territory.distance_for("enemy", Coord { x: 3, y: 5 }),
            Some(2)
        );
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

        assert!(enemy.useful_choke_count > 0);
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
        let recomposed = TerritoryAnalysis::from_parts(&state, core, structural);

        for snake_id in ["ours", "enemy", "wall"] {
            assert_eq!(public.for_snake(snake_id), recomposed.for_snake(snake_id));
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

    #[test]
    fn shared_spatial_preserves_consumer_specific_head_semantics() {
        use std::sync::Arc;

        use crate::simulation::mobility::{DeterministicMoveBlock, MobilityAnalysis};
        use crate::spatial::SpatialOccupancy;

        let state = state(
            7,
            7,
            vec![snake("ours", &[(2, 2)]), snake("enemy", &[(3, 2), (3, 1)])],
        );
        let spatial = Arc::new(SpatialOccupancy::from_state(&state));
        let mobility = MobilityAnalysis::from_spatial(Arc::clone(&spatial));
        let territory = TerritoryAnalysis::from_spatial(&state, &spatial);

        assert_eq!(
            mobility.classify_move(&state, state.snake("ours").unwrap(), Direction::Right,),
            Some(DeterministicMoveBlock::DeterministicBodyCollision)
        );
        assert!(territory.for_snake("ours").unwrap().reachable_space > 0);
        assert_eq!(
            territory.for_snake("ours"),
            TerritoryAnalysis::from_state(&state).for_snake("ours")
        );
    }
}
