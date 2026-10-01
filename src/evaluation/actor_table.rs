#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorTable<T> {
    entries: Vec<(String, T)>,
}

impl<T> ActorTable<T> {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::with_capacity(4),
        }
    }

    pub(crate) fn insert(&mut self, actor_id: impl Into<String>, value: T) -> Option<T> {
        let actor_id = actor_id.into();
        if let Some((_, existing)) = self
            .entries
            .iter_mut()
            .find(|(existing_id, _)| existing_id == &actor_id)
        {
            return Some(std::mem::replace(existing, value));
        }

        self.entries.push((actor_id, value));
        None
    }

    pub(crate) fn get(&self, actor_id: &str) -> Option<&T> {
        self.entries
            .iter()
            .find(|(existing_id, _)| existing_id == actor_id)
            .map(|(_, value)| value)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries
            .iter()
            .map(|(actor_id, value)| (actor_id.as_str(), value))
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl ActorTable<i64> {
    pub(crate) fn add(&mut self, actor_id: &str, delta: i64) {
        if let Some((_, value)) = self
            .entries
            .iter_mut()
            .find(|(existing_id, _)| existing_id == actor_id)
        {
            *value = value.saturating_add(delta);
            return;
        }

        self.entries.push((actor_id.to_string(), delta));
    }
}

impl<T> Default for ActorTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FromIterator<(String, T)> for ActorTable<T> {
    fn from_iter<I: IntoIterator<Item = (String, T)>>(iter: I) -> Self {
        let mut table = Self::new();
        for (actor_id, value) in iter {
            table.insert(actor_id, value);
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_replaces_and_reads_actor_values() {
        let mut table = ActorTable::new();
        assert_eq!(table.insert("ours", 10), None);
        assert_eq!(table.insert("enemy", 20), None);
        assert_eq!(table.insert("ours", 30), Some(10));

        assert_eq!(table.len(), 2);
        assert_eq!(table.get("ours"), Some(&30));
        assert_eq!(table.get("enemy"), Some(&20));
    }

    #[test]
    fn integer_table_accumulates_without_hashing() {
        let mut table = ActorTable::new();
        table.add("enemy", 40);
        table.add("enemy", -10);

        assert_eq!(table.get("enemy"), Some(&30));
    }
}
