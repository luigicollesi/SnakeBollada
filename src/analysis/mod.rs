mod temporal_corridors;
mod temporal_territory;
pub(crate) use temporal_corridors::{
    adversarial_order, AdversarialCorridorOrder, CorridorOutlook, CORRIDOR_HORIZON,
};
pub(crate) use temporal_territory::{TemporalTerritory, TerritoryCellWeights};
