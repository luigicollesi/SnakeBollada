#![cfg_attr(not(test), allow(dead_code))]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForecastCertainty {
    Deterministic,
    FoodProvisional,
}
