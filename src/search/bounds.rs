#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueBound {
    Exact(i64),
    LowerBound(i64),
    UpperBound(i64),
    Interval { lower: i64, upper: i64 },
}

impl ValueBound {
    pub(crate) const fn exact(value: i64) -> Self {
        Self::Exact(value)
    }

    pub(crate) const fn lower(self) -> i64 {
        match self {
            Self::Exact(value) | Self::LowerBound(value) => value,
            Self::UpperBound(_) => i64::MIN,
            Self::Interval { lower, .. } => lower,
        }
    }

    pub(crate) const fn upper(self) -> i64 {
        match self {
            Self::Exact(value) | Self::UpperBound(value) => value,
            Self::LowerBound(_) => i64::MAX,
            Self::Interval { upper, .. } => upper,
        }
    }

    pub(crate) const fn is_exact(self) -> bool {
        matches!(self, Self::Exact(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_bound_has_identical_limits() {
        let bound = ValueBound::exact(42);

        assert_eq!(bound.lower(), 42);
        assert_eq!(bound.upper(), 42);
        assert!(bound.is_exact());
    }

    #[test]
    fn incomplete_bounds_do_not_claim_unknown_side() {
        assert_eq!(ValueBound::LowerBound(10).upper(), i64::MAX);
        assert_eq!(ValueBound::UpperBound(10).lower(), i64::MIN);
        assert!(!ValueBound::Interval { lower: 1, upper: 9 }.is_exact());
    }
}
