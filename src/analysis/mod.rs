mod temporal_corridors;
mod temporal_territory;
mod territorial_control;
pub(crate) use temporal_corridors::{adversarial_order, AdversarialCorridorOrder, CorridorOutlook};
pub(crate) use temporal_territory::{TemporalTerritory, TerritoryCellWeights};
pub(crate) use territorial_control::{
    analyze as analyze_territorial_control, TerritorialControlAnalysis,
};
