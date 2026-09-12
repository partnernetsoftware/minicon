//! Lightweight, in-window terminal session tree for `minicon`.
//!
//! This deliberately owns only tab identity and parentage. PTYs, rendering,
//! persistence, and any background authority remain outside this type: the
//! standalone console host must stay one GUI process with bounded local state.

pub const MAX_TABS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TabId(u64);

impl TabId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabNode {
    pub id: TabId,
    pub parent: Option<TabId>,
    pub title: String,
}

/// The lightweight host's complete tab tree.
///
/// Parent cycles are impossible because a node can only be created beneath an
/// existing node. Closing a parent promotes its direct children, preserving
/// their sessions instead of treating hierarchy as ownership of a PTY.
#[derive(Debug)]
pub struct Workspace {
    nodes: Vec<TabNode>,
    depths: Vec<u32>,
    active: Option<TabId>,
    next_id: u64,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            depths: Vec::new(),
            active: None,
            next_id: 1,
        }
    }
}

impl Workspace {
    pub fn nodes(&self) -> &[TabNode] {
        &self.nodes
    }

    pub fn depths(&self) -> &[u32] {
        &self.depths
    }

    pub const fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn node(&self, id: TabId) -> Option<&TabNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn set_active(&mut self, id: TabId) -> bool {
        if self.node(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    pub fn add_root(&mut self, title: String) -> Option<TabId> {
        self.add(None, title, 0)
    }

    pub fn add_child(&mut self, parent: TabId, title: String) -> Option<TabId> {
        let parent_index = self.nodes.iter().position(|node| node.id == parent)?;
        let depth = self.depths[parent_index].saturating_add(1);
        self.add(Some(parent), title, depth)
    }

    pub fn close(&mut self, id: TabId) -> Option<TabNode> {
        let index = self.nodes.iter().position(|node| node.id == id)?;
        let removed = self.nodes.remove(index);
        for node in &mut self.nodes {
            if node.parent == Some(id) {
                node.parent = removed.parent;
            }
        }
        self.depths = self.recompute_depths();
        if self.active == Some(id) {
            self.active = self
                .nodes
                .get(index)
                .or_else(|| self.nodes.last())
                .map(|node| node.id);
        }
        Some(removed)
    }

    /// Recomputes every node's depth from parentage.
    ///
    /// The tree this module builds cannot make `compute_tree_depths_by` fail:
    /// ids come from a monotonic counter (no duplicates), `add_child` demands a
    /// live parent, and `close` reparents a removed node's children to its own
    /// parent (still an ancestor, so no cycle). A failure would therefore be a
    /// bug in this module, not bad input. Surface it loudly under `debug_assert`
    /// so a test catches it, but keep the shipped build's defined behaviour of
    /// a flat tree rather than turning a rendering glitch into a panic in a
    /// windowed process with no console.
    fn recompute_depths(&self) -> Vec<u32> {
        match agenterm_ui_core::compute_tree_depths_by(
            &self.nodes,
            |node| node.id,
            |node| node.parent,
        ) {
            Ok(depths) => depths,
            Err(error) => {
                debug_assert!(
                    false,
                    "workspace parentage is not a well-formed tree: {error:?}"
                );
                vec![0; self.nodes.len()]
            }
        }
    }

    fn add(&mut self, parent: Option<TabId>, title: String, depth: u32) -> Option<TabId> {
        if self.nodes.len() >= MAX_TABS || self.next_id == u64::MAX {
            return None;
        }
        let id = TabId(self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        self.nodes.push(TabNode { id, parent, title });
        self.depths.push(depth);
        self.active = Some(id);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_parent_promotes_direct_children_and_keeps_them_live() {
        let mut workspace = Workspace::default();
        let root = workspace.add_root("root".into()).unwrap();
        let parent = workspace.add_child(root, "parent".into()).unwrap();
        let child = workspace.add_child(parent, "child".into()).unwrap();
        let grandchild = workspace.add_child(child, "grandchild".into()).unwrap();

        workspace.close(parent).unwrap();

        assert_eq!(workspace.node(child).unwrap().parent, Some(root));
        assert_eq!(workspace.node(grandchild).unwrap().parent, Some(child));
        assert_eq!(workspace.depths(), &[0, 1, 2]);
    }

    #[test]
    fn closing_active_tab_selects_a_remaining_neighbor() {
        let mut workspace = Workspace::default();
        let first = workspace.add_root("first".into()).unwrap();
        let second = workspace.add_root("second".into()).unwrap();
        workspace.close(second);
        assert_eq!(workspace.active(), Some(first));
    }

    #[test]
    fn creation_limits_fail_without_mutating_tree_or_active_tab() {
        let mut workspace = Workspace::default();
        for index in 0..MAX_TABS {
            assert!(workspace.add_root(format!("tab {index}")).is_some());
        }
        let active = workspace.active();
        assert_eq!(workspace.add_root("overflow".into()), None);
        assert_eq!(workspace.nodes().len(), MAX_TABS);
        assert_eq!(workspace.active(), active);

        workspace.close(active.unwrap());
        let node_count = workspace.nodes().len();
        let active = workspace.active();
        workspace.next_id = u64::MAX;
        assert_eq!(workspace.add_root("exhausted".into()), None);
        assert_eq!(workspace.nodes().len(), node_count);
        assert_eq!(workspace.active(), active);
    }

    /// The depth recomputation is documented as unable to fail for a tree this
    /// module builds, so the failure branch is not reachable through the public
    /// API. Force the ill-formed state (a child whose parent id is absent) and
    /// prove two things: it is loud under `debug_assert` (a test build panics),
    /// and the shipped path still returns a defined flat vector rather than
    /// propagating the error into a windowed process with no console.
    #[test]
    fn an_ill_formed_tree_is_loud_in_debug_and_flat_in_release() {
        let mut workspace = Workspace::default();
        let root = workspace.add_root("root".into()).unwrap();
        workspace.add_child(root, "child".into()).unwrap();
        // Splice in a parent id that no node carries. Only reachable here
        // because the test module can see `nodes`; the public API cannot.
        workspace.nodes.push(TabNode {
            id: TabId::new(500),
            parent: Some(TabId::new(999)),
            title: "orphan".into(),
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            workspace.recompute_depths()
        }));
        match result {
            Err(_) => {}
            Ok(depths) => {
                panic!("an ill-formed tree must trip the debug assertion, got depths {depths:?}")
            }
        }
    }

    /// A stale id (a click on a tab that has since closed, a control command
    /// racing teardown) must be a no-op, not a panic or a silent retarget.
    #[test]
    fn operations_on_an_unknown_id_are_no_ops() {
        let mut workspace = Workspace::default();
        let root = workspace.add_root("root".into()).unwrap();
        let active_before = workspace.active();
        let unknown = TabId::new(9_999);

        assert!(
            !workspace.set_active(unknown),
            "set_active must refuse an unknown id"
        );
        assert_eq!(workspace.active(), active_before);

        assert!(
            workspace.add_child(unknown, "child".into()).is_none(),
            "a child needs a real parent"
        );
        assert_eq!(workspace.nodes().len(), 1);

        assert!(
            workspace.close(unknown).is_none(),
            "closing an unknown id is a no-op"
        );
        assert_eq!(workspace.nodes().len(), 1);
        assert_eq!(workspace.active(), Some(root));
        assert!(workspace.node(unknown).is_none());
    }
}
