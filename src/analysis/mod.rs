mod adversarial_escape;
mod escape_evidence;
#[cfg(test)]
pub(crate) mod replay_fixtures;
mod temporal_corridors;
mod temporal_return;
mod temporal_territory;
mod territorial_control;
mod territorial_forecast;
pub(crate) use adversarial_escape::AdversarialEscapeAnalyzer;
pub(crate) use escape_evidence::{EscapeEvidence, EscapeSnapshot};
pub(crate) use temporal_corridors::{adversarial_order, AdversarialCorridorOrder, CorridorOutlook};
pub(crate) use temporal_return::{
    verify_return_cached as verify_territorial_return_cached, ReturnProof,
};
pub(crate) use temporal_territory::{TemporalTerritory, TerritoryCellWeights};
pub(crate) use territorial_control::{
    analyze as analyze_territorial_control, partition_after as detect_territorial_partition,
    TerritorialControlAnalysis,
};
pub(crate) use territorial_forecast::{
    TerritoryDirectionSample, TerritorySnapshot, TerritoryTrajectory,
};
