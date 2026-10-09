#[cfg(test)]
pub(crate) mod replay_fixtures;
mod temporal_corridors;
mod temporal_return;
mod temporal_territory;
mod territorial_control;
pub(crate) use temporal_corridors::{adversarial_order, AdversarialCorridorOrder, CorridorOutlook};
pub(crate) use temporal_return::{
    verify_return as verify_territorial_return, ReturnExpansion, ReturnProof,
};
pub(crate) use temporal_territory::{TemporalTerritory, TerritoryCellWeights};
pub(crate) use territorial_control::{
    analyze as analyze_territorial_control, partition_after as detect_territorial_partition,
    TerritorialControlAnalysis,
};
