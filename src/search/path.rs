//! Persistent path representation for state-based Hobbs search.
use super::graph::NodeId;
use crate::simulation::joint_action::JointAction;
use std::sync::Arc;

pub(crate) const MAX_SEARCH_DEPTH: u8 = 21;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FutureStep {
    pub(crate) node: NodeId,
    pub(crate) joint_action: JointAction,
    pub(crate) child: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FuturePathNode {
    Step(FutureStep),
    Concat { left: FuturePath, right: FuturePath },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FuturePath {
    root: Option<Arc<FuturePathNode>>,
    len: usize,
}

impl FuturePath {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn single(step: FutureStep) -> Self {
        Self {
            root: Some(Arc::new(FuturePathNode::Step(step))),
            len: 1,
        }
    }

    pub(crate) fn prepend(&self, step: FutureStep) -> Self {
        Self::single(step).concat(self)
    }

    pub(crate) fn concat(&self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }

        Self {
            root: Some(Arc::new(FuturePathNode::Concat {
                left: self.clone(),
                right: other.clone(),
            })),
            len: self.len.saturating_add(other.len),
        }
    }

    #[cfg(test)]
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn first(&self) -> Option<&FutureStep> {
        first_step(self.root.as_deref()?)
    }

    /// Traverse the already-chosen MAX/MIN path without flattening or
    /// cloning its persistent, shared structure.
    pub(crate) fn for_each_step(&self, mut visit: impl FnMut(&FutureStep)) {
        if let Some(root) = self.root.as_deref() {
            visit_steps(root, &mut visit);
        }
    }

    #[cfg(test)]
    pub(crate) fn last(&self) -> Option<&FutureStep> {
        last_step(self.root.as_deref()?)
    }

    #[cfg(test)]
    pub(crate) fn steps(&self) -> Vec<&FutureStep> {
        let mut steps = Vec::with_capacity(self.len);
        if let Some(root) = self.root.as_deref() {
            collect_steps(root, &mut steps);
        }
        steps
    }
}

fn first_step(node: &FuturePathNode) -> Option<&FutureStep> {
    match node {
        FuturePathNode::Step(step) => Some(step),
        FuturePathNode::Concat { left, right } => left.first().or_else(|| right.first()),
    }
}

fn visit_steps(node: &FuturePathNode, visit: &mut impl FnMut(&FutureStep)) {
    match node {
        FuturePathNode::Step(step) => visit(step),
        FuturePathNode::Concat { left, right } => {
            if let Some(left_root) = left.root.as_deref() {
                visit_steps(left_root, visit);
            }
            if let Some(right_root) = right.root.as_deref() {
                visit_steps(right_root, visit);
            }
        }
    }
}

#[cfg(test)]
fn last_step(node: &FuturePathNode) -> Option<&FutureStep> {
    match node {
        FuturePathNode::Step(step) => Some(step),
        FuturePathNode::Concat { left, right } => right.last().or_else(|| left.last()),
    }
}

#[cfg(test)]
fn collect_steps<'a>(node: &'a FuturePathNode, steps: &mut Vec<&'a FutureStep>) {
    match node {
        FuturePathNode::Step(step) => steps.push(step),
        FuturePathNode::Concat { left, right } => {
            if let Some(left_root) = left.root.as_deref() {
                collect_steps(left_root, steps);
            }
            if let Some(right_root) = right.root.as_deref() {
                collect_steps(right_root, steps);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_path_concatenates_without_copying_moves() {
        let first = FuturePath::single(FutureStep {
            node: 1,
            joint_action: JointAction::new(),
            child: 2,
        });
        let second = FuturePath::single(FutureStep {
            node: 2,
            joint_action: JointAction::new(),
            child: 3,
        });
        let combined = first.concat(&second);
        assert_eq!(combined.len(), 2);
        assert_eq!(combined.first().unwrap().node, 1);
        assert_eq!(combined.last().unwrap().child, 3);
        assert_eq!(combined.steps().len(), 2);
        let mut visited = Vec::new();
        combined.for_each_step(|step| visited.push(step.child));
        assert_eq!(visited, vec![2, 3]);
    }
}
