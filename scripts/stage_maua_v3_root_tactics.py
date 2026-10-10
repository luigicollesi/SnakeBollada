#!/usr/bin/env python3
"""Stage a shared, bounded, adversarial next-turn tactical guard for Maua V3.

Run from SnakeBollada checkout:
  python3 scripts/stage_maua_v3_root_tactics.py ../battlesnake_luigi

This edits an existing NON-DEPLOYMENT branch only; never pushes or deploys.
It does NOT reconstruct the concrete lost game (replay remains required).
"""
from pathlib import Path
import argparse, subprocess

def replace_once(content, old, new, where):
    if content.count(old) != 1:
        raise RuntimeError(f"{where}: expected exactly one anchor, got {content.count(old)}")
    return content.replace(old, new, 1)

def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root, text=True).strip()

ROOT_TACTICS_RUST = "//! Verified one-ply outcomes using exactly the joint-action generator and turn\n//! resolver used by FutureGraph. No Hobbs heuristic can turn partial\n//! adversarial coverage into a proof of survival.\nuse std::time::Instant;\n\nuse crate::decision::joint_actions::JointActionGenerator;\nuse crate::direction::{Direction, MoveMask};\nuse crate::simulation::mobility::MobilityAnalysis;\nuse crate::simulation::resolver::resolve_turn;\nuse crate::simulation::state::SimulatedGameState;\n\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub(crate) enum TacticalStatus {\n    /// All legal joint replies were simulated; all leave our snake alive.\n    VerifiedSafe,\n    /// A concrete adversarial reply kills our snake. Other replies may survive.\n    Vulnerable,\n    /// All legal joint replies were simulated; every reply kills us.\n    ForcedLoss,\n    /// The reply generator was not exhausted within the tactical budget.\n    Incomplete,\n    /// Deterministic fatal next move, independent of enemy response.\n    Blocked,\n}\n\n#[derive(Debug, Clone, Copy)]\npub(crate) struct TacticalDirection {\n    pub(crate) status: TacticalStatus,\n    pub(crate) checked_replies: u16,\n    pub(crate) fatal_replies: u16,\n}\nimpl Default for TacticalDirection {\n    fn default() -> Self {\n        Self {\n            status: TacticalStatus::Incomplete,\n            checked_replies: 0,\n            fatal_replies: 0,\n        }\n    }\n}\n\n#[derive(Debug, Clone)]\npub(crate) struct RootTactics {\n    directions: [TacticalDirection; 4],\n}\nimpl RootTactics {\n    pub(crate) fn entry(&self, direction: Direction) -> TacticalDirection {\n        self.directions[usize::from(direction.rank())]\n    }\n    pub(crate) fn verified_safe(&self, direction: Direction) -> bool {\n        self.entry(direction).status == TacticalStatus::VerifiedSafe\n    }\n    pub(crate) fn any_verified_safe(&self) -> bool {\n        Direction::ALL.into_iter().any(|direction| self.verified_safe(direction))\n    }\n    pub(crate) fn demonstrated_loss(&self, direction: Direction) -> bool {\n        matches!(\n            self.entry(direction).status,\n            TacticalStatus::Vulnerable | TacticalStatus::ForcedLoss | TacticalStatus::Blocked\n        )\n    }\n    /// Only simulate root replies when a simultaneous head-to-head is geometrically\n    /// possible next turn. Else return None to keep the normal V3 budget intact.\n    pub(crate) fn when_head_contact_possible(\n        state: &SimulatedGameState,\n        deadline: Instant,\n        max_replies_per_direction: usize,\n    ) -> Option<Self> {\n        let head = state.snake(&state.our_snake_id)?.head()?;\n        let contact_possible = state.snakes.iter().any(|snake| {\n            snake.alive\n                && snake.id != state.our_snake_id\n                && snake.head().is_some_and(|enemy| {\n                    head.x.abs_diff(enemy.x) + head.y.abs_diff(enemy.y) <= 2\n                })\n        });\n        contact_possible.then(|| Self::analyze(state, deadline, max_replies_per_direction))\n    }\n    /// A complete direction checks every action generated for all living\n    /// opponents. If work is interrupted, no VerifiedSafe proof is emitted.\n    fn analyze(\n        state: &SimulatedGameState,\n        deadline: Instant,\n        max_replies_per_direction: usize,\n    ) -> Self {\n        let mut result = Self {\n            directions: [TacticalDirection::default(); 4],\n        };\n        let mobility = MobilityAnalysis::from_state(state);\n        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {\n            return result;\n        };\n        for direction in Direction::ALL {\n            let slot = &mut result.directions[usize::from(direction.rank())];\n            if mobility.classify_move(state, ours, direction).is_some() {\n                slot.status = TacticalStatus::Blocked;\n                continue;\n            }\n            if Instant::now() >= deadline {\n                break;\n            }\n            let mut exhausted = true;\n            for joint in JointActionGenerator::adversarial_root(\n                state,\n                MoveMask::single(direction),\n                &mobility,\n            ) {\n                if Instant::now() >= deadline\n                    || usize::from(slot.checked_replies) >= max_replies_per_direction\n                {\n                    exhausted = false;\n                    break;\n                }\n                let Ok(next) = resolve_turn(state, &joint) else {\n                    exhausted = false;\n                    break;\n                };\n                slot.checked_replies += 1;\n                if !next\n                    .state\n                    .snake(&state.our_snake_id)\n                    .is_some_and(|snake| snake.alive)\n                {\n                    slot.fatal_replies += 1;\n                }\n            }\n            slot.status = if exhausted && slot.checked_replies > 0 {\n                if slot.fatal_replies == 0 {\n                    TacticalStatus::VerifiedSafe\n                } else if slot.fatal_replies == slot.checked_replies {\n                    TacticalStatus::ForcedLoss\n                } else {\n                    TacticalStatus::Vulnerable\n                }\n            } else if slot.fatal_replies > 0 {\n                // A single fully simulated losing counterexample is enough\n                // to disprove a claim that this direction is guaranteed safe.\n                TacticalStatus::Vulnerable\n            } else {\n                TacticalStatus::Incomplete\n            };\n        }\n        result\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use std::time::Duration;\n    use crate::simulation::state::{RulesContext, SimulatedSnake};\n    use crate::Coord;\n\n    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {\n        SimulatedSnake {\n            id: id.into(),\n            health: 100,\n            body: body.iter().map(|&(x, y)| Coord { x, y }).collect(),\n            alive: true,\n        }\n    }\n    fn state(our_body: &[(i32, i32)], enemy_body: &[(i32, i32)], food: &[(i32, i32)]) -> SimulatedGameState {\n        SimulatedGameState {\n            turn: 33,\n            width: 11,\n            height: 11,\n            food: food.iter().map(|&(x,y)| Coord{x,y}).collect(),\n            hazards: vec![],\n            snakes: vec![snake(\"ours\",our_body),snake(\"enemy\",enemy_body)],\n            our_snake_id: \"ours\".into(),\n            rules: RulesContext{\n                name:\"standard\".into(),\n                max_health:100,\n                hazard_damage_per_turn:0,\n            },\n        }\n    }\n    fn report(s: &SimulatedGameState, cap: usize) -> RootTactics {\n        RootTactics::analyze(s, Instant::now() + Duration::from_secs(1), cap)\n    }\n    #[test]\n    fn smaller_snake_cannot_claim_contested_food_against_longer_enemy() {\n        let s = state(\n            &[(2,2),(2,1)],\n            &[(4,2),(4,1),(5,1),(5,2)],\n            &[(3,2)],\n        );\n        let r = report(&s, 16);\n        assert!(r.demonstrated_loss(Direction::Right), \"{r:?}\");\n        assert!(r.verified_safe(Direction::Up), \"{r:?}\");\n        assert!(r.entry(Direction::Right).fatal_replies >= 1);\n    }\n    #[test]\n    fn equal_length_head_to_head_is_not_safe_even_with_food() {\n        let s = state(\n            &[(2,2),(2,1),(1,1)],\n            &[(4,2),(4,1),(5,1)],\n            &[(3,2)],\n        );\n        let r = report(&s, 16);\n        assert!(r.demonstrated_loss(Direction::Right), \"{r:?}\");\n        assert!(r.verified_safe(Direction::Up), \"{r:?}\");\n    }\n    #[test]\n    fn reply_budget_exhaustion_cannot_prove_survival() {\n        let s = state(\n            &[(2,2),(2,1)],\n            &[(4,2),(4,1),(5,1),(5,2)],\n            &[(3,2)],\n        );\n        let r = report(&s, 0);\n        assert_eq!(r.entry(Direction::Up).status, TacticalStatus::Incomplete);\n        assert!(!r.any_verified_safe());\n    }\n    #[test]\n    fn distant_heads_skip_expensive_simulation() {\n        let s = state(&[(1,1),(1,0)], &[(9,9),(9,8),(8,8)], &[]);\n        assert!(RootTactics::when_head_contact_possible(\n            &s, Instant::now() + Duration::from_millis(15), 16\n        ).is_none());\n    }\n}\n"

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    target = args.destination.resolve()
    branch = git(target, "branch", "--show-current")
    if not branch.startswith("fix/"):
        raise SystemExit(f"Refusing branch {branch!r}: create a separate fix/ branch before running")
    if "Maua-Dev/battlesnake_luigi" not in git(target, "remote", "get-url", "origin"):
        raise SystemExit("Wrong target remote")
    if git(target, "status", "--porcelain"):
        raise SystemExit("Target has uncommitted modifications; refusing to edit")
    if not (target / "src/search/hobbs_flow.rs").is_file():
        raise SystemExit("Expected migrated V3 motor is absent")
    path = target / "src/analysis/immediate_tactics.rs"
    if path.exists():
        raise SystemExit("Tactics already installed; avoid applying twice.")

    patches = {}
    def edit(file, a, b):
        text = patches.get(file, (target / file).read_text())
        patches[file] = replace_once(text, a, b, file)

    edit("src/analysis/mod.rs",
        "mod adversarial_escape;",
        "mod adversarial_escape;\nmod immediate_tactics;\npub(crate) use immediate_tactics::RootTactics;")

    # Root proof must include even suicidal enemy commands, unlike the
    # search's usual opponent pruning. The shared resolver is unchanged.
    edit("src/decision/joint_actions.rs",
        "    fn advance(&mut self) {",
        '''    pub(crate) fn adversarial_root(
        state: &SimulatedGameState,
        our_moves: MoveMask,
        mobility: &MobilityAnalysis,
    ) -> Self {
        let mut generator = Self::new(state, our_moves, mobility);
        if let Some(ours) = state.actor_index(&state.our_snake_id) {
            for (actor, options) in &mut generator.options {
                if *actor != ours {
                    // The rules accept losing directions as inputs; do not
                    // omit them while certifying guaranteed survival.
                    *options = Direction::ALL.to_vec();
                }
            }
        }
        generator
    }

    fn advance(&mut self) {''')

    # Keep fallback's usual priority unless a fully verified safe option exists.
    edit("src/strategy.rs",
        "pub(crate) fn choose_move_baseline(state: &GameState) -> Decision {\n",
        '''pub(crate) fn choose_move_baseline(state: &GameState) -> Decision {
    let simulated = crate::simulation::state::SimulatedGameState::from(state);
    let tactics = crate::analysis::RootTactics::when_head_contact_possible(
        &simulated,
        std::time::Instant::now() + std::time::Duration::from_millis(12),
        16,
    );
    choose_move_baseline_with_tactics(state, tactics.as_ref())
}

pub(crate) fn choose_move_baseline_with_tactics(
    state: &GameState,
    tactics: Option<&crate::analysis::RootTactics>,
) -> Decision {
''')
    edit("src/strategy.rs",
        '''            let contested_head = map.lethal_head_danger.contains(destination);
            let cramped = reachable < state.you.length;''',
        '''            let safe = tactics.is_some_and(|report| report.verified_safe(direction));
            let tactical_priority =
                tactics.is_some_and(|report| report.any_verified_safe() && !safe);
            let contested_head = !safe && map.lethal_head_danger.contains(destination);
            let cramped = reachable < state.you.length;''')
    edit("src/strategy.rs",
        '''                direction,
                reachable,
                contested_head,
                cramped,
                hazard,
                food_distance,
            )
        })
        .min_by_key(
            |(direction, reachable, contested_head, cramped, hazard, food_distance)| {
                (
                    *contested_head,''',
        '''                direction,
                reachable,
                tactical_priority,
                contested_head,
                cramped,
                hazard,
                food_distance,
            )
        })
        .min_by_key(
            |(direction, reachable, tactical_priority, contested_head, cramped, hazard, food_distance)| {
                (
                    *tactical_priority,
                    *contested_head,''')

    edit("src/logic.rs",
        '''    let state = crate::GameState::from(input);
    let mut decision = runtime().decide(&state, started).await;''',
        '''    let state = crate::GameState::from(input);
    // Cheap adversarial precheck is only needed when heads could meet next
    // turn. It uses the exact FutureGraph action generator and resolver.
    let simulated = crate::simulation::state::SimulatedGameState::from(&state);
    let tactics = crate::analysis::RootTactics::when_head_contact_possible(
        &simulated,
        Instant::now() + std::time::Duration::from_millis(15),
        16,
    );
    let mut decision = runtime().decide(&state, started).await;
    if let Some(report) = tactics.as_ref() {
        if report.any_verified_safe() && report.demonstrated_loss(decision.direction) {
            let previous = decision.direction;
            let replacement = crate::strategy::choose_move_baseline_with_tactics(
                &state,
                Some(report),
            );
            if report.verified_safe(replacement.direction) {
                log::warn!(
                    "lambda_tactical_guard game={} turn={} rejected={:?} reply={:?} selected={:?} selected_reply={:?}",
                    state.game.id,
                    state.turn,
                    previous,
                    report.entry(previous),
                    replacement.direction,
                    report.entry(replacement.direction)
                );
                decision = replacement;
            }
        }
    }''')

    # Tactical test is intentionally synthetic. It does NOT claim to be the
    # replay of the real 34-turn head-to-head loss.
    patches["src/analysis/immediate_tactics.rs"] = ROOT_TACTICS_RUST
    for file, text in patches.items():
        dest = target / file
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)
    print(f"Applied {len(patches)} Rust files on {branch}, without Terraform, push or deploy.")

if __name__ == "__main__":
    main()
