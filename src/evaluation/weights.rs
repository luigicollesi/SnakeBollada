use super::context::ActorContext;

const STRATEGIC_BUDGET: u16 = 1000;
const MIN_CATEGORY_WEIGHT: u16 = 100;
const MAX_CATEGORY_WEIGHT: u16 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrategicWeights {
    pub(crate) food: u16,
    pub(crate) hunting: u16,
    pub(crate) survival: u16,
}

impl StrategicWeights {
    pub(crate) fn from_territory_share(context: &ActorContext, territory_share_milli: u16) -> Self {
        let survival = STRATEGIC_BUDGET
            .saturating_sub(territory_share_milli)
            .clamp(MIN_CATEGORY_WEIGHT, MAX_CATEGORY_WEIGHT);
        let offensive_budget = STRATEGIC_BUDGET.saturating_sub(survival);

        let food_share = 500_i32
            .saturating_add(i32::from(context.size_disadvantage_milli) / 2)
            .saturating_sub(i32::from(context.size_advantage_milli) / 2)
            .clamp(
                i32::from(MIN_CATEGORY_WEIGHT),
                i32::from(MAX_CATEGORY_WEIGHT),
            ) as u16;

        let food = u32::from(offensive_budget)
            .saturating_mul(u32::from(food_share))
            .saturating_div(u32::from(STRATEGIC_BUDGET))
            .try_into()
            .unwrap_or(offensive_budget);
        let hunting = offensive_budget.saturating_sub(food);

        Self {
            food,
            hunting,
            survival,
        }
    }

    pub(crate) const fn total(self) -> u16 {
        self.food
            .saturating_add(self.hunting)
            .saturating_add(self.survival)
    }
}

#[cfg(test)]
mod tests {
    use crate::evaluation::context::ActorContext;

    use super::*;

    fn context() -> ActorContext {
        ActorContext {
            size_advantage_milli: 0,
            size_disadvantage_milli: 0,
        }
    }

    #[test]
    fn strategic_weights_always_consume_the_whole_budget() {
        let weights = StrategicWeights::from_territory_share(&context(), 400);
        assert_eq!(weights.total(), STRATEGIC_BUDGET);
    }

    #[test]
    fn less_territory_increases_survival_and_suppresses_offense() {
        let ctx = context();
        let comfortable = StrategicWeights::from_territory_share(&ctx, 700);
        let constrained = StrategicWeights::from_territory_share(&ctx, 150);

        assert!(constrained.survival > comfortable.survival);
        assert!(
            constrained.food.saturating_add(constrained.hunting)
                < comfortable.food.saturating_add(comfortable.hunting)
        );
    }

    #[test]
    fn size_disadvantage_shifts_offense_from_hunting_to_food() {
        let mut smaller = context();
        smaller.size_disadvantage_milli = 800;
        smaller.size_advantage_milli = 0;

        let mut larger = context();
        larger.size_disadvantage_milli = 0;
        larger.size_advantage_milli = 800;

        let smaller = StrategicWeights::from_territory_share(&smaller, 600);
        let larger = StrategicWeights::from_territory_share(&larger, 600);

        assert!(smaller.food > smaller.hunting);
        assert!(larger.hunting > larger.food);
        assert!(smaller.food > larger.food);
        assert!(larger.hunting > smaller.hunting);
    }
}
