mod enclosure;
mod routes;
mod tactical;
mod territory;
pub(crate) mod transition;

pub(crate) use enclosure::{EnclosureAnalysis, EnclosureRisk, EnclosureSnapshot};
pub(crate) use routes::{FoodClaimInfo, FoodRouteInfo, StateAnalysis};
pub(crate) use tactical::{EnemyTacticalSnapshot, TacticalStateAnalysis};
pub(crate) use territory::{ChokePoint, SnakeTerritorySnapshot, TerritoryAnalysis};
