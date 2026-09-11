//! Compact stable-tab-id storage for the lightweight console host.
//!
//! Tree order and parentage belong to `workspace`; this module only maps an
//! id to an owned value. Linear lookup avoids general-purpose map machinery
//! for con's deliberately small interactive tab sets. Storage order is not a
//! public contract, so removal may use `swap_remove`.

use crate::workspace::TabId;

pub(super) struct SessionStore<T> {
    entries: Vec<(TabId, T)>,
}

impl<T> Default for SessionStore<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<T> SessionStore<T> {
    pub(super) fn insert(&mut self, id: TabId, session: T) -> Result<(), T> {
        if self.contains_key(&id) {
            return Err(session);
        }
        self.entries.push((id, session));
        Ok(())
    }

    pub(super) fn get(&self, id: &TabId) -> Option<&T> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == id)
            .map(|(_, session)| session)
    }

    pub(super) fn get_mut(&mut self, id: &TabId) -> Option<&mut T> {
        self.entries
            .iter_mut()
            .find(|(candidate, _)| candidate == id)
            .map(|(_, session)| session)
    }

    pub(super) fn remove(&mut self, id: &TabId) -> Option<T> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == id)?;
        Some(self.entries.swap_remove(index).1)
    }

    pub(super) fn contains_key(&self, id: &TabId) -> bool {
        self.entries.iter().any(|(candidate, _)| candidate == id)
    }

    pub(super) fn entries_mut(&mut self) -> &mut [(TabId, T)] {
        self.entries.as_mut_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_own_values_independently_of_storage_order() {
        let first = TabId::new(1);
        let middle = TabId::new(2);
        let last = TabId::new(3);
        let mut store = SessionStore::default();
        store.insert(first, "first".to_owned()).unwrap();
        store.insert(middle, "middle".to_owned()).unwrap();
        store.insert(last, "last".to_owned()).unwrap();

        store.get_mut(&last).unwrap().push('!');
        assert_eq!(store.remove(&middle).as_deref(), Some("middle"));
        assert_eq!(store.get(&first).map(String::as_str), Some("first"));
        assert_eq!(store.get(&last).map(String::as_str), Some("last!"));
        assert!(!store.contains_key(&middle));
    }

    #[test]
    fn duplicate_id_is_rejected_without_replacing_the_live_session() {
        let id = TabId::new(1);
        let mut store = SessionStore::default();
        store.insert(id, "original").unwrap();

        assert_eq!(store.insert(id, "duplicate"), Err("duplicate"));
        assert_eq!(store.get(&id), Some(&"original"));
    }

    /// A stale id must be a no-op, and removing an id frees it to be inserted
    /// again — closing a tab and opening a new one reuses the slot rather than
    /// leaking it or colliding with the old value.
    #[test]
    fn unknown_ids_and_reinsertion_after_removal() {
        let id = TabId::new(1);
        let unknown = TabId::new(9);
        let mut store = SessionStore::default();
        store.insert(id, "live").unwrap();

        assert!(store.get(&unknown).is_none());
        assert!(store.get_mut(&unknown).is_none());
        assert!(store.remove(&unknown).is_none());
        assert_eq!(
            store.get(&id),
            Some(&"live"),
            "an unknown-id operation must not disturb a live entry"
        );

        // Removal frees the id; the same id is insertable again.
        assert_eq!(store.remove(&id), Some("live"));
        store
            .insert(id, "reopened")
            .expect("a closed id can be reused");
        assert_eq!(store.get(&id), Some(&"reopened"));
    }

    /// `entries_mut` is the drain loop's view of every session; it must expose
    /// exactly the stored pairs, order included.
    #[test]
    fn entries_mut_exposes_every_stored_pair_in_order() {
        let mut store = SessionStore::default();
        store.insert(TabId::new(1), "a").unwrap();
        store.insert(TabId::new(2), "b").unwrap();
        let entries = store.entries_mut();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], (TabId::new(1), "a"));
        assert_eq!(entries[1], (TabId::new(2), "b"));
        entries[0].1 = "a2";
        assert_eq!(store.get(&TabId::new(1)), Some(&"a2"));
    }

    /// `remove` uses `swap_remove`, so dropping a middle entry moves the last
    /// one into its slot. Storage order is explicitly not a contract, but the
    /// removal must still leave every survivor reachable and `entries_mut`
    /// complete — otherwise the drain loop, which reads `entries_mut`, would
    /// silently skip a session.
    #[test]
    fn swap_removal_keeps_every_survivor_reachable() {
        let mut store = SessionStore::default();
        for id in 1..=4 {
            store.insert(TabId::new(id), format!("s{id}")).unwrap();
        }
        assert_eq!(store.remove(&TabId::new(2)).as_deref(), Some("s2"));

        // The survivors are all present and no slot is duplicated or lost.
        let mut seen: Vec<TabId> = store.entries_mut().iter().map(|(id, _)| *id).collect();
        seen.sort_by_key(|id| id.get());
        assert_eq!(
            seen,
            vec![TabId::new(1), TabId::new(3), TabId::new(4)],
            "removal must not lose, duplicate, or keep the removed id"
        );
        for id in [1, 3, 4] {
            let expected = format!("s{id}");
            assert_eq!(
                store.get(&TabId::new(id)).map(String::as_str),
                Some(expected.as_str()),
                "@{id} must survive the swap removal"
            );
        }
        assert!(store.get(&TabId::new(2)).is_none());
    }
}
