#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CategoryScore {
    pub(crate) benefit: i64,
    pub(crate) harm: i64,
}

impl CategoryScore {
    pub(crate) const fn new(benefit: i64, harm: i64) -> Self {
        Self { benefit, harm }
    }

    pub(crate) fn net(self) -> i64 {
        self.benefit.saturating_sub(self.harm)
    }

    pub(crate) fn from_net(net: i64) -> Self {
        if net >= 0 {
            Self::new(net, 0)
        } else {
            Self::new(0, net.saturating_neg())
        }
    }

    pub(crate) fn saturating_add(self, other: Self) -> Self {
        Self {
            benefit: self.benefit.saturating_add(other.benefit),
            harm: self.harm.saturating_add(other.harm),
        }
    }

    pub(crate) fn saturating_sub(self, other: Self) -> Self {
        Self {
            benefit: self.benefit.saturating_sub(other.benefit),
            harm: self.harm.saturating_sub(other.harm),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RouteUtilityBreakdown {
    pub(crate) food: CategoryScore,
    pub(crate) hunting: CategoryScore,
    pub(crate) survival: CategoryScore,
    pub(crate) terminal: CategoryScore,
}

impl RouteUtilityBreakdown {
    pub(crate) const fn new(
        food: CategoryScore,
        hunting: CategoryScore,
        survival: CategoryScore,
        terminal: CategoryScore,
    ) -> Self {
        Self {
            food,
            hunting,
            survival,
            terminal,
        }
    }

    pub(crate) fn nonterminal_benefit(self) -> i64 {
        self.food
            .benefit
            .saturating_add(self.hunting.benefit)
            .saturating_add(self.survival.benefit)
    }

    pub(crate) fn nonterminal_harm(self) -> i64 {
        self.food
            .harm
            .saturating_add(self.hunting.harm)
            .saturating_add(self.survival.harm)
    }

    pub(crate) fn nonterminal_net(self) -> i64 {
        self.nonterminal_benefit()
            .saturating_sub(self.nonterminal_harm())
    }

    pub(crate) fn terminal_net(self) -> i64 {
        self.terminal.net()
    }

    pub(crate) fn total_net(self) -> i64 {
        self.nonterminal_net().saturating_add(self.terminal_net())
    }

    pub(crate) fn nonterminal_only(self) -> Self {
        Self {
            terminal: CategoryScore::default(),
            ..self
        }
    }

    pub(crate) fn choice_net(self) -> i64 {
        let terminal = self.terminal_net();
        if terminal != 0 {
            terminal
        } else {
            self.nonterminal_net()
        }
    }

    pub(crate) fn total_benefit(self) -> i64 {
        self.nonterminal_benefit()
            .saturating_add(self.terminal.benefit)
    }

    pub(crate) fn total_harm(self) -> i64 {
        self.nonterminal_harm()
            .saturating_add(self.terminal.harm)
    }

    pub(crate) fn saturating_add(self, other: Self) -> Self {
        Self {
            food: self.food.saturating_add(other.food),
            hunting: self.hunting.saturating_add(other.hunting),
            survival: self.survival.saturating_add(other.survival),
            terminal: self.terminal.saturating_add(other.terminal),
        }
    }

    pub(crate) fn saturating_sub(self, other: Self) -> Self {
        Self {
            food: self.food.saturating_sub(other.food),
            hunting: self.hunting.saturating_sub(other.hunting),
            survival: self.survival.saturating_sub(other.survival),
            terminal: self.terminal.saturating_sub(other.terminal),
        }
    }

    pub(crate) fn competitive_against(self, opponent: Self) -> Self {
        Self {
            food: CategoryScore::new(
                self.food.benefit.saturating_add(opponent.food.harm),
                self.food.harm.saturating_add(opponent.food.benefit),
            ),
            hunting: CategoryScore::new(
                self.hunting
                    .benefit
                    .saturating_add(opponent.hunting.harm),
                self.hunting
                    .harm
                    .saturating_add(opponent.hunting.benefit),
            ),
            survival: CategoryScore::new(
                self.survival
                    .benefit
                    .saturating_add(opponent.survival.harm),
                self.survival
                    .harm
                    .saturating_add(opponent.survival.benefit),
            ),
            terminal: CategoryScore::new(
                self.terminal
                    .benefit
                    .saturating_add(opponent.terminal.harm),
                self.terminal
                    .harm
                    .saturating_add(opponent.terminal.benefit),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_totals_equal_sum_of_categories() {
        let route = RouteUtilityBreakdown::new(
            CategoryScore::new(10, 2),
            CategoryScore::new(20, 3),
            CategoryScore::new(30, 4),
            CategoryScore::new(40, 5),
        );

        assert_eq!(route.nonterminal_benefit(), 60);
        assert_eq!(route.nonterminal_harm(), 9);
        assert_eq!(route.total_benefit(), 100);
        assert_eq!(route.total_harm(), 14);
        assert_eq!(route.nonterminal_net(), 51);
        assert_eq!(route.choice_net(), 35);
    }

    #[test]
    fn add_then_subtract_restores_breakdown() {
        let left = RouteUtilityBreakdown::new(
            CategoryScore::new(50, 10),
            CategoryScore::new(30, 20),
            CategoryScore::new(70, 5),
            CategoryScore::default(),
        );
        let right = RouteUtilityBreakdown::new(
            CategoryScore::new(5, 1),
            CategoryScore::new(6, 2),
            CategoryScore::new(7, 3),
            CategoryScore::default(),
        );

        assert_eq!(left.saturating_add(right).saturating_sub(right), left);
    }

    #[test]
    fn competitive_breakdown_preserves_category_accounting() {
        let ours = RouteUtilityBreakdown::new(
            CategoryScore::new(100, 20),
            CategoryScore::new(80, 30),
            CategoryScore::new(60, 10),
            CategoryScore::default(),
        );
        let opponent = RouteUtilityBreakdown::new(
            CategoryScore::new(40, 50),
            CategoryScore::new(30, 70),
            CategoryScore::new(20, 90),
            CategoryScore::default(),
        );
        let effective = ours.competitive_against(opponent);

        assert_eq!(effective.food, CategoryScore::new(150, 60));
        assert_eq!(effective.hunting, CategoryScore::new(150, 60));
        assert_eq!(effective.survival, CategoryScore::new(150, 30));
    }
}
