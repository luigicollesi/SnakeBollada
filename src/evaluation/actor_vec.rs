use crate::simulation::state::ActorIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorVec<T> {
    entries: Vec<Option<T>>,
}

impl<T> ActorVec<T> {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub(crate) fn with_capacity(actor_count: usize) -> Self {
        Self {
            entries: Vec::with_capacity(actor_count),
        }
    }

    pub(crate) fn insert(&mut self, actor: ActorIndex, value: T) -> Option<T> {
        let index = actor.as_usize();
        if self.entries.len() <= index {
            self.entries.resize_with(index + 1, || None);
        }
        self.entries[index].replace(value)
    }

    pub(crate) fn get(&self, actor: ActorIndex) -> Option<&T> {
        self.entries.get(actor.as_usize())?.as_ref()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (ActorIndex, &T)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, value)| Some((ActorIndex::new(index)?, value.as_ref()?)))
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }
}

impl ActorVec<i64> {
    pub(crate) fn add(&mut self, actor: ActorIndex, delta: i64) {
        let index = actor.as_usize();
        if self.entries.len() <= index {
            self.entries.resize_with(index + 1, || None);
        }

        match &mut self.entries[index] {
            Some(value) => *value = value.saturating_add(delta),
            None => self.entries[index] = Some(delta),
        }
    }
}

impl<T> Default for ActorVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FromIterator<(ActorIndex, T)> for ActorVec<T> {
    fn from_iter<I: IntoIterator<Item = (ActorIndex, T)>>(iter: I) -> Self {
        let mut values = Self::new();
        for (actor, value) in iter {
            values.insert(actor, value);
        }
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_values_by_actor_index_without_ids() {
        let ours = ActorIndex::new(0).unwrap();
        let enemy = ActorIndex::new(2).unwrap();
        let mut values = ActorVec::new();

        values.insert(ours, 10);
        values.insert(enemy, 30);

        assert_eq!(values.get(ours), Some(&10));
        assert_eq!(values.get(enemy), Some(&30));
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn integer_values_accumulate_in_place() {
        let enemy = ActorIndex::new(1).unwrap();
        let mut values = ActorVec::new();

        values.add(enemy, 40);
        values.add(enemy, -10);

        assert_eq!(values.get(enemy), Some(&30));
    }
}
