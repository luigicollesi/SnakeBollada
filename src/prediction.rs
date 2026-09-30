use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::board_mask::BoardMask;
use crate::strategy::Direction;
use crate::telemetry::ObservedMove;
use crate::{Battlesnake, Coord, GameState};

const HISTORY_LIMIT: usize = 64;
const SOFTMAX_TEMPERATURE: f32 = 1.75;
const MIN_PROBABILITY: f32 = 1.0e-6;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoveDistribution {
    pub(crate) up: f32,
    pub(crate) right: f32,
    pub(crate) down: f32,
    pub(crate) left: f32,
}

impl MoveDistribution {
    fn zero() -> Self {
        Self {
            up: 0.0,
            right: 0.0,
            down: 0.0,
            left: 0.0,
        }
    }

    fn get(self, direction: Direction) -> f32 {
        match direction {
            Direction::Up => self.up,
            Direction::Right => self.right,
            Direction::Down => self.down,
            Direction::Left => self.left,
        }
    }

    fn set(&mut self, direction: Direction, value: f32) {
        match direction {
            Direction::Up => self.up = value,
            Direction::Right => self.right = value,
            Direction::Down => self.down = value,
            Direction::Left => self.left = value,
        }
    }

    fn uniform(candidates: &[CandidateFeatures]) -> Self {
        let legal_count = candidates.iter().filter(|candidate| candidate.legal).count();
        if legal_count == 0 {
            return Self::zero();
        }

        let probability = 1.0 / legal_count as f32;
        let mut result = Self::zero();

        for candidate in candidates.iter().filter(|candidate| candidate.legal) {
            result.set(candidate.direction, probability);
        }

        result
    }

    fn blend(self, other: Self, self_weight: f32) -> Self {
        let weight = self_weight.clamp(0.0, 1.0);
        let mut result = Self::zero();

        for direction in Direction::ALL {
            result.set(
                direction,
                self.get(direction) * weight + other.get(direction) * (1.0 - weight),
            );
        }

        result
    }

    fn top_direction(self) -> Option<Direction> {
        Direction::ALL
            .into_iter()
            .filter(|direction| self.get(*direction) > 0.0)
            .min_by(|left, right| {
                self.get(*right)
                    .total_cmp(&self.get(*left))
                    .then_with(|| left.rank().cmp(&right.rank()))
            })
    }

    fn brier_score(self, actual: Direction) -> f32 {
        Direction::ALL
            .into_iter()
            .map(|direction| {
                let expected = if direction == actual { 1.0 } else { 0.0 };
                let error = self.get(direction) - expected;
                error * error
            })
            .sum()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct BehavioralProfileSnapshot {
    pub(crate) food_bias: f32,
    pub(crate) space_bias: f32,
    pub(crate) aggression: f32,
    pub(crate) hazard_tolerance: f32,
    pub(crate) observations: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct PredictionMetricsSnapshot {
    pub(crate) resolved_predictions: u32,
    pub(crate) top1_accuracy: f32,
    pub(crate) mean_brier_score: f32,
    pub(crate) mean_log_loss: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct OpponentPredictionSnapshot {
    pub(crate) snake_id: String,
    pub(crate) distribution: MoveDistribution,
    pub(crate) confidence: f32,
    pub(crate) top_move: Option<Direction>,
    pub(crate) legal_moves: u8,
    pub(crate) profile: BehavioralProfileSnapshot,
    pub(crate) metrics: PredictionMetricsSnapshot,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct OpponentPredictionRecord {
    pub(crate) turn: i32,
    pub(crate) opponents: HashMap<String, OpponentPredictionSnapshot>,
}

#[derive(Debug, Clone, Copy)]
struct BetaPreference {
    positive: f32,
    negative: f32,
}

impl Default for BetaPreference {
    fn default() -> Self {
        Self {
            positive: 2.0,
            negative: 2.0,
        }
    }
}

impl BetaPreference {
    fn mean(self) -> f32 {
        self.positive / (self.positive + self.negative)
    }

    fn observe(&mut self, positive: bool) {
        if positive {
            self.positive += 1.0;
        } else {
            self.negative += 1.0;
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BehavioralProfile {
    food: BetaPreference,
    space: BetaPreference,
    aggression: BetaPreference,
    hazard_tolerance: BetaPreference,
    observations: u32,
}

impl BehavioralProfile {
    fn snapshot(&self) -> BehavioralProfileSnapshot {
        BehavioralProfileSnapshot {
            food_bias: self.food.mean(),
            space_bias: self.space.mean(),
            aggression: self.aggression.mean(),
            hazard_tolerance: self.hazard_tolerance.mean(),
            observations: self.observations,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CandidateFeatures {
    direction: Direction,
    legal: bool,
    food_progress: i32,
    reachable_space: u32,
    enemy_progress: i32,
    enters_hazard: bool,
    lethal_head_risk: bool,
}

#[derive(Debug, Clone)]
struct PendingPrediction {
    turn: i32,
    distribution: MoveDistribution,
    candidates: Vec<CandidateFeatures>,
}

#[derive(Debug, Clone, Copy)]
struct OpponentObservation {
    #[allow(dead_code)]
    turn: i32,
    #[allow(dead_code)]
    actual_move: Direction,
    #[allow(dead_code)]
    predicted_probability: f32,
    #[allow(dead_code)]
    top1_correct: bool,
    #[allow(dead_code)]
    brier_score: f32,
    #[allow(dead_code)]
    log_loss: f32,
}

#[derive(Debug, Clone, Default)]
struct PredictionMetrics {
    resolved_predictions: u32,
    top1_correct: u32,
    brier_sum: f32,
    log_loss_sum: f32,
}

impl PredictionMetrics {
    fn observe(&mut self, distribution: MoveDistribution, actual: Direction) -> OpponentObservation {
        let predicted_probability = distribution.get(actual).max(MIN_PROBABILITY);
        let top1_correct = distribution.top_direction() == Some(actual);
        let brier_score = distribution.brier_score(actual);
        let log_loss = -predicted_probability.ln();

        self.resolved_predictions += 1;
        self.top1_correct += u32::from(top1_correct);
        self.brier_sum += brier_score;
        self.log_loss_sum += log_loss;

        OpponentObservation {
            turn: 0,
            actual_move: actual,
            predicted_probability,
            top1_correct,
            brier_score,
            log_loss,
        }
    }

    fn snapshot(&self) -> PredictionMetricsSnapshot {
        if self.resolved_predictions == 0 {
            return PredictionMetricsSnapshot {
                resolved_predictions: 0,
                top1_accuracy: 0.0,
                mean_brier_score: 0.0,
                mean_log_loss: 0.0,
            };
        }

        let count = self.resolved_predictions as f32;
        PredictionMetricsSnapshot {
            resolved_predictions: self.resolved_predictions,
            top1_accuracy: self.top1_correct as f32 / count,
            mean_brier_score: self.brier_sum / count,
            mean_log_loss: self.log_loss_sum / count,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct OpponentModel {
    profile: BehavioralProfile,
    metrics: PredictionMetrics,
    history: VecDeque<OpponentObservation>,
    pending: Option<PendingPrediction>,
}

impl OpponentModel {
    fn resolve(&mut self, observed: ObservedMove) {
        let Some(pending) = self.pending.take() else {
            return;
        };

        let ObservedMove::Known(actual) = observed else {
            return;
        };

        let Some(chosen) = pending
            .candidates
            .iter()
            .copied()
            .find(|candidate| candidate.direction == actual && candidate.legal)
        else {
            return;
        };

        self.update_profile(chosen, &pending.candidates);

        let mut observation = self.metrics.observe(pending.distribution, actual);
        observation.turn = pending.turn;

        if self.history.len() == HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(observation);
    }

    fn update_profile(&mut self, chosen: CandidateFeatures, candidates: &[CandidateFeatures]) {
        let legal = candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.legal)
            .collect::<Vec<_>>();

        if legal.is_empty() {
            return;
        }

        if legal.iter().any(|candidate| candidate.food_progress > 0) {
            self.profile.food.observe(chosen.food_progress > 0);
        }

        let min_space = legal
            .iter()
            .map(|candidate| candidate.reachable_space)
            .min()
            .unwrap_or(0);
        let max_space = legal
            .iter()
            .map(|candidate| candidate.reachable_space)
            .max()
            .unwrap_or(0);

        if max_space > min_space {
            let threshold = (max_space as f32 * 0.90).floor() as u32;
            self.profile
                .space
                .observe(chosen.reachable_space >= threshold);
        }

        if legal.iter().any(|candidate| candidate.enemy_progress > 0) {
            self.profile
                .aggression
                .observe(chosen.enemy_progress > 0);
        }

        let has_hazard = legal.iter().any(|candidate| candidate.enters_hazard);
        let has_safe = legal.iter().any(|candidate| !candidate.enters_hazard);
        if has_hazard && has_safe {
            self.profile
                .hazard_tolerance
                .observe(chosen.enters_hazard);
        }

        self.profile.observations = self.profile.observations.saturating_add(1);
    }

    fn predict(
        &mut self,
        turn: i32,
        state: &GameState,
        enemy: &Battlesnake,
        candidates: Vec<CandidateFeatures>,
    ) -> OpponentPredictionSnapshot {
        let legal_moves = candidates.iter().filter(|candidate| candidate.legal).count() as u8;
        let uniform = MoveDistribution::uniform(&candidates);

        let (distribution, confidence) = if legal_moves <= 1 {
            (uniform, if legal_moves == 1 { 1.0 } else { 0.0 })
        } else {
            let heuristic = heuristic_distribution(state, enemy, &self.profile, &candidates);
            let confidence = model_confidence(&self.profile, heuristic, legal_moves);
            (heuristic.blend(uniform, confidence), confidence)
        };

        self.pending = Some(PendingPrediction {
            turn,
            distribution,
            candidates,
        });

        OpponentPredictionSnapshot {
            snake_id: enemy.id.clone(),
            distribution,
            confidence,
            top_move: distribution.top_direction(),
            legal_moves,
            profile: self.profile.snapshot(),
            metrics: self.metrics.snapshot(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct OpponentPredictor {
    models: HashMap<String, OpponentModel>,
}

impl OpponentPredictor {
    pub(crate) fn observe_and_predict(
        &mut self,
        state: &GameState,
        observed_moves: &HashMap<String, ObservedMove>,
    ) -> OpponentPredictionRecord {
        for (snake_id, observed) in observed_moves {
            if let Some(model) = self.models.get_mut(snake_id) {
                model.resolve(*observed);
            }
        }

        let mut opponents = HashMap::new();

        for enemy in state
            .board
            .snakes
            .iter()
            .filter(|snake| snake.id != state.you.id)
        {
            let candidates = candidate_features(state, enemy);
            let model = self.models.entry(enemy.id.clone()).or_default();
            let prediction = model.predict(state.turn, state, enemy, candidates);
            opponents.insert(enemy.id.clone(), prediction);
        }

        OpponentPredictionRecord {
            turn: state.turn,
            opponents,
        }
    }

    pub(crate) fn resolve_final(&mut self, observed_moves: &HashMap<String, ObservedMove>) {
        for (snake_id, observed) in observed_moves {
            if let Some(model) = self.models.get_mut(snake_id) {
                model.resolve(*observed);
            }
        }
    }
}

fn candidate_features(state: &GameState, enemy: &Battlesnake) -> Vec<CandidateFeatures> {
    let width = state.board.width as u16;
    let height = state.board.height as u16;
    let occupied = BoardMask::from_coords(
        width,
        height,
        state
            .board
            .snakes
            .iter()
            .flat_map(|snake| snake.body.iter().copied()),
    );

    let current_food_distance = nearest_food_distance(state, enemy.head);
    let current_enemy_distance = nearest_other_head_distance(state, enemy.id.as_str(), enemy.head);

    Direction::ALL
        .into_iter()
        .map(|direction| {
            let target = direction.apply(enemy.head);
            let food = state.board.food.contains(&target);
            let hazard = state.board.hazards.contains(&target);
            let own_tail = enemy.body.last().copied();

            let in_bounds = in_bounds(state, target);
            let reverses_into_neck = enemy.body.get(1).copied() == Some(target);
            let occupied_target = occupied.contains(target);
            let tail_releases = own_tail == Some(target) && !food;
            let fatal_hazard = hazard
                && !food
                && enemy.health <= 1 + hazard_damage_per_turn(state);

            let legal = in_bounds
                && !reverses_into_neck
                && (!occupied_target || tail_releases)
                && !fatal_hazard;

            let reachable_space = if legal {
                reachable_space_after_enemy_move(state, enemy, target, &occupied, food)
            } else {
                0
            };

            let next_food_distance = nearest_food_distance(state, target);
            let next_enemy_distance =
                nearest_other_head_distance(state, enemy.id.as_str(), target);

            CandidateFeatures {
                direction,
                legal,
                food_progress: progress(current_food_distance, next_food_distance),
                reachable_space,
                enemy_progress: progress(current_enemy_distance, next_enemy_distance),
                enters_hazard: hazard && !food,
                lethal_head_risk: legal && lethal_head_risk(state, enemy, target),
            }
        })
        .collect()
}

fn heuristic_distribution(
    state: &GameState,
    enemy: &Battlesnake,
    profile: &BehavioralProfile,
    candidates: &[CandidateFeatures],
) -> MoveDistribution {
    let board_cells = (state.board.width * state.board.height).max(1) as f32;
    let health_pressure = ((55 - enemy.health).max(0) as f32 / 55.0).clamp(0.0, 1.0);

    let food_weight = 0.75 + 2.75 * health_pressure + 1.25 * profile.food.mean();
    let space_weight = 0.75 + 1.75 * profile.space.mean();
    let aggression_weight = 0.15 + 1.25 * profile.aggression.mean();
    let hazard_penalty = 1.0 + 3.0 * (1.0 - profile.hazard_tolerance.mean());

    let mut scores = Vec::new();
    for candidate in candidates.iter().filter(|candidate| candidate.legal) {
        let normalized_space = candidate.reachable_space as f32 / board_cells;
        let score = food_weight * candidate.food_progress as f32
            + space_weight * normalized_space
            + aggression_weight * candidate.enemy_progress as f32
            - if candidate.enters_hazard {
                hazard_penalty
            } else {
                0.0
            }
            - if candidate.lethal_head_risk { 4.0 } else { 0.0 };

        scores.push((candidate.direction, score / SOFTMAX_TEMPERATURE));
    }

    softmax(&scores)
}

fn softmax(scores: &[(Direction, f32)]) -> MoveDistribution {
    if scores.is_empty() {
        return MoveDistribution::zero();
    }

    let max_score = scores
        .iter()
        .map(|(_, score)| *score)
        .fold(f32::NEG_INFINITY, f32::max);

    if !max_score.is_finite() {
        return uniform_directions(scores.iter().map(|(direction, _)| *direction));
    }

    let mut weights = Vec::with_capacity(scores.len());
    let mut total = 0.0_f32;

    for (direction, score) in scores {
        let weight = (*score - max_score).exp();
        if !weight.is_finite() {
            return uniform_directions(scores.iter().map(|(direction, _)| *direction));
        }

        total += weight;
        weights.push((*direction, weight));
    }

    if !total.is_finite() || total <= 0.0 {
        return uniform_directions(scores.iter().map(|(direction, _)| *direction));
    }

    let mut result = MoveDistribution::zero();
    for (direction, weight) in weights {
        result.set(direction, weight / total);
    }
    result
}

fn uniform_directions(directions: impl IntoIterator<Item = Direction>) -> MoveDistribution {
    let directions = directions.into_iter().collect::<Vec<_>>();
    if directions.is_empty() {
        return MoveDistribution::zero();
    }

    let probability = 1.0 / directions.len() as f32;
    let mut result = MoveDistribution::zero();
    for direction in directions {
        result.set(direction, probability);
    }
    result
}

fn model_confidence(
    profile: &BehavioralProfile,
    distribution: MoveDistribution,
    legal_moves: u8,
) -> f32 {
    if legal_moves <= 1 {
        return 1.0;
    }

    let observations = profile.observations as f32;
    let observation_confidence = observations / (observations + 8.0);

    let entropy = Direction::ALL
        .into_iter()
        .map(|direction| distribution.get(direction))
        .filter(|probability| *probability > 0.0)
        .map(|probability| -probability * probability.ln())
        .sum::<f32>();

    let max_entropy = (legal_moves as f32).ln();
    let separation = if max_entropy > 0.0 {
        (1.0 - entropy / max_entropy).clamp(0.0, 1.0)
    } else {
        1.0
    };

    (0.25 + 0.55 * observation_confidence + 0.20 * separation).clamp(0.20, 0.95)
}

fn reachable_space_after_enemy_move(
    state: &GameState,
    enemy: &Battlesnake,
    destination: Coord,
    occupied: &BoardMask,
    ate_food: bool,
) -> u32 {
    let width = state.board.width as u16;
    let height = state.board.height as u16;
    let mut blocked = occupied.clone();

    if !ate_food {
        if let Some(tail) = enemy.body.last().copied() {
            blocked.set(tail, false);
        }
    }

    blocked.set(destination, false);

    let mut visited = BoardMask::new(width, height);
    let mut queue = VecDeque::new();
    let mut reachable = 0_u32;

    visited.set(destination, true);
    queue.push_back(destination);

    while let Some(current) = queue.pop_front() {
        reachable += 1;

        for direction in Direction::ALL {
            let next = direction.apply(current);
            if !in_bounds(state, next) || blocked.contains(next) || visited.contains(next) {
                continue;
            }

            visited.set(next, true);
            queue.push_back(next);
        }
    }

    reachable
}

fn lethal_head_risk(state: &GameState, enemy: &Battlesnake, target: Coord) -> bool {
    state.board.snakes.iter().any(|other| {
        other.id != enemy.id
            && other.length >= enemy.length
            && manhattan(other.head, target) == 1
            && other.body.get(1).copied() != Some(target)
    })
}

fn nearest_food_distance(state: &GameState, from: Coord) -> Option<u16> {
    state
        .board
        .food
        .iter()
        .map(|food| manhattan(from, *food))
        .min()
}

fn nearest_other_head_distance(state: &GameState, snake_id: &str, from: Coord) -> Option<u16> {
    state
        .board
        .snakes
        .iter()
        .filter(|snake| snake.id != snake_id)
        .map(|snake| manhattan(from, snake.head))
        .min()
}

fn progress(before: Option<u16>, after: Option<u16>) -> i32 {
    match (before, after) {
        (Some(before), Some(after)) => i32::from(before) - i32::from(after),
        _ => 0,
    }
}

fn manhattan(left: Coord, right: Coord) -> u16 {
    let distance = (left.x - right.x).unsigned_abs() + (left.y - right.y).unsigned_abs();
    distance.min(u32::from(u16::MAX)) as u16
}

fn in_bounds(state: &GameState, coord: Coord) -> bool {
    coord.x >= 0
        && coord.y >= 0
        && coord.x < state.board.width as i32
        && coord.y < state.board.height as i32
}

fn hazard_damage_per_turn(state: &GameState) -> i32 {
    state
        .game
        .ruleset
        .get("settings")
        .and_then(|settings| settings.get("hazardDamagePerTurn"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(14)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Board, Game};

    fn snake(id: &str, health: i32, body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: id.to_string(),
            name: id.to_string(),
            health,
            head: body[0],
            length: body.len() as u32,
            body,
            latency: String::new(),
            shout: None,
        }
    }

    fn state(enemy: Battlesnake, food: Vec<Coord>, hazards: Vec<Coord>) -> GameState {
        let ours = snake(
            "ours",
            100,
            vec![
                Coord { x: 1, y: 1 },
                Coord { x: 1, y: 0 },
                Coord { x: 0, y: 0 },
            ],
        );

        GameState {
            game: Game {
                id: "game".to_string(),
                ruleset: HashMap::from([
                    ("name".to_string(), json!("standard")),
                    (
                        "settings".to_string(),
                        json!({ "hazardDamagePerTurn": 14 }),
                    ),
                ]),
                timeout: 500,
            },
            turn: 1,
            board: Board {
                height: 7,
                width: 7,
                food,
                snakes: vec![ours.clone(), enemy],
                hazards,
            },
            you: ours,
        }
    }

    fn assert_distribution(distribution: MoveDistribution) {
        let total = Direction::ALL
            .into_iter()
            .map(|direction| distribution.get(direction))
            .sum::<f32>();

        assert!((total - 1.0).abs() < 1.0e-5, "total={total}");
        for direction in Direction::ALL {
            assert!(distribution.get(direction).is_finite());
            assert!(distribution.get(direction) >= 0.0);
        }
    }

    #[test]
    fn impossible_moves_receive_zero_probability() {
        let enemy = snake(
            "enemy",
            100,
            vec![
                Coord { x: 0, y: 6 },
                Coord { x: 0, y: 5 },
                Coord { x: 0, y: 4 },
            ],
        );
        let state = state(enemy, vec![], vec![]);
        let mut predictor = OpponentPredictor::default();

        let record = predictor.observe_and_predict(&state, &HashMap::new());
        let prediction = &record.opponents["enemy"];

        assert_eq!(prediction.distribution.up, 0.0);
        assert_eq!(prediction.distribution.down, 0.0);
        assert_distribution(prediction.distribution);
    }

    #[test]
    fn one_legal_move_has_probability_one() {
        let enemy = snake(
            "enemy",
            100,
            vec![
                Coord { x: 0, y: 6 },
                Coord { x: 0, y: 5 },
                Coord { x: 1, y: 5 },
                Coord { x: 1, y: 6 },
            ],
        );
        let state = state(enemy, vec![], vec![]);
        let mut predictor = OpponentPredictor::default();

        let record = predictor.observe_and_predict(&state, &HashMap::new());
        let prediction = &record.opponents["enemy"];

        assert_eq!(prediction.legal_moves, 1);
        assert_eq!(prediction.distribution.right, 1.0);
        assert_eq!(prediction.confidence, 1.0);
    }

    #[test]
    fn observed_food_choice_updates_profile_and_metrics() {
        let enemy = snake(
            "enemy",
            35,
            vec![
                Coord { x: 4, y: 4 },
                Coord { x: 4, y: 3 },
                Coord { x: 4, y: 2 },
            ],
        );
        let mut first = state(enemy.clone(), vec![Coord { x: 5, y: 4 }], vec![]);
        let mut predictor = OpponentPredictor::default();

        predictor.observe_and_predict(&first, &HashMap::new());

        first.turn = 2;
        first.board.snakes[1].head = Coord { x: 5, y: 4 };
        first.board.snakes[1].body = vec![
            Coord { x: 5, y: 4 },
            Coord { x: 4, y: 4 },
            Coord { x: 4, y: 3 },
            Coord { x: 4, y: 2 },
        ];
        first.board.snakes[1].length = 4;

        let observed = HashMap::from([(
            "enemy".to_string(),
            ObservedMove::Known(Direction::Right),
        )]);
        let record = predictor.observe_and_predict(&first, &observed);
        let prediction = &record.opponents["enemy"];

        assert!(prediction.profile.food_bias > 0.5);
        assert_eq!(prediction.metrics.resolved_predictions, 1);
        assert!(prediction.metrics.mean_brier_score.is_finite());
        assert!(prediction.metrics.mean_log_loss.is_finite());
    }

    #[test]
    fn eliminated_unknown_does_not_train_model() {
        let enemy = snake(
            "enemy",
            100,
            vec![
                Coord { x: 4, y: 4 },
                Coord { x: 4, y: 3 },
                Coord { x: 4, y: 2 },
            ],
        );
        let state = state(enemy, vec![], vec![]);
        let mut predictor = OpponentPredictor::default();

        predictor.observe_and_predict(&state, &HashMap::new());
        predictor.resolve_final(&HashMap::from([(
            "enemy".to_string(),
            ObservedMove::EliminatedUnknown,
        )]));

        let model = predictor.models.get("enemy").expect("model exists");
        assert_eq!(model.profile.observations, 0);
        assert_eq!(model.metrics.resolved_predictions, 0);
    }
}
