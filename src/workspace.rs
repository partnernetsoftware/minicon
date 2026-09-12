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
        match minicon_core::tree::compute_tree_depths_by(
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

    fn depth_of(workspace: &Workspace, id: TabId) -> u32 {
        let index = workspace
            .nodes()
            .iter()
            .position(|node| node.id == id)
            .expect("node present");
        workspace.depths()[index]
    }

    /// Closing a node with several children promotes all of them to its parent,
    /// and every depth below the removed node shifts up by one — not just the
    /// promoted node's own row.
    #[test]
    fn closing_a_branch_promotes_every_child_and_rewrites_the_subtree_depths() {
        let mut workspace = Workspace::default();
        let root = workspace.add_root("root".into()).unwrap();
        let branch = workspace.add_child(root, "branch".into()).unwrap();
        let leaf_a = workspace.add_child(branch, "a".into()).unwrap();
        let leaf_b = workspace.add_child(branch, "b".into()).unwrap();
        let deep = workspace.add_child(leaf_a, "a1".into()).unwrap();
        assert_eq!(depth_of(&workspace, deep), 3, "before: root>branch>a>a1");

        workspace.close(branch).unwrap();

        // Both leaves move up under the root, and `a`'s own child follows it.
        assert_eq!(workspace.node(leaf_a).unwrap().parent, Some(root));
        assert_eq!(workspace.node(leaf_b).unwrap().parent, Some(root));
        assert_eq!(workspace.node(deep).unwrap().parent, Some(leaf_a));
        assert_eq!(depth_of(&workspace, root), 0);
        assert_eq!(depth_of(&workspace, leaf_a), 1, "promoted one level");
        assert_eq!(depth_of(&workspace, leaf_b), 1, "promoted one level");
        assert_eq!(depth_of(&workspace, deep), 2, "the grandchild moves up too");
    }

    /// Closing the root promotes its direct children to roots (depth 0) and the
    /// rest of each subtree follows them down.
    #[test]
    fn closing_the_root_makes_its_children_roots() {
        let mut workspace = Workspace::default();
        let root = workspace.add_root("root".into()).unwrap();
        let child_one = workspace.add_child(root, "one".into()).unwrap();
        let child_two = workspace.add_child(root, "two".into()).unwrap();
        let grandchild = workspace.add_child(child_one, "one-a".into()).unwrap();

        workspace.close(root).unwrap();

        assert_eq!(workspace.node(child_one).unwrap().parent, None);
        assert_eq!(workspace.node(child_two).unwrap().parent, None);
        assert_eq!(depth_of(&workspace, child_one), 0);
        assert_eq!(depth_of(&workspace, child_two), 0);
        assert_eq!(
            depth_of(&workspace, grandchild),
            1,
            "the subtree follows its root"
        );
    }

    /// Closing a deep leaf leaves its ancestors' depths unchanged.
    #[test]
    fn closing_a_deep_leaf_does_not_move_its_ancestors() {
        let mut workspace = Workspace::default();
        let root = workspace.add_root("root".into()).unwrap();
        let middle = workspace.add_child(root, "middle".into()).unwrap();
        let leaf = workspace.add_child(middle, "leaf".into()).unwrap();

        workspace.close(leaf).unwrap();
        assert_eq!(depth_of(&workspace, root), 0);
        assert_eq!(depth_of(&workspace, middle), 1);
        assert_eq!(workspace.nodes().len(), 2);
    }

    #[test]
    fn closing_active_tab_selects_a_remaining_neighbor() {
        let mut workspace = Workspace::default();
        let first = workspace.add_root("first".into()).unwrap();
        let second = workspace.add_root("second".into()).unwrap();
        workspace.close(second);
        assert_eq!(workspace.active(), Some(first));
    }

    /// Closing the active tab must land on a neighbour by a rule, not an
    /// accident: the node that shifted into the closed slot (the next tab in
    /// tree order) when one exists, otherwise the new last tab. Only the
    /// two-tab case was covered.
    #[test]
    fn closing_the_active_tab_picks_the_next_tab_then_the_previous() {
        let mut workspace = Workspace::default();
        let first = workspace.add_root("first".into()).unwrap();
        let second = workspace.add_root("second".into()).unwrap();
        let third = workspace.add_root("third".into()).unwrap();

        // Close the middle tab: the next tab (`third`) shifts into its slot.
        assert!(workspace.set_active(second));
        workspace.close(second).unwrap();
        assert_eq!(
            workspace.active(),
            Some(third),
            "the next tab in order takes the slot"
        );
        assert_eq!(workspace.nodes().len(), 2);

        // Close the last tab: there is no next one, so the new last is chosen.
        assert!(workspace.set_active(third));
        workspace.close(third).unwrap();
        assert_eq!(
            workspace.active(),
            Some(first),
            "the previous tab is the new last"
        );

        // Closing the only remaining tab leaves no active tab at all.
        assert!(workspace.set_active(first));
        workspace.close(first).unwrap();
        assert_eq!(workspace.active(), None, "an empty tree has no active tab");
        assert!(workspace.nodes().is_empty());
    }

    /// Closing a tab that is not active must not move the selection, however
    /// many neighbours it has.
    #[test]
    fn closing_a_background_tab_leaves_the_active_tab_alone() {
        let mut workspace = Workspace::default();
        let first = workspace.add_root("first".into()).unwrap();
        let second = workspace.add_root("second".into()).unwrap();
        let third = workspace.add_root("third".into()).unwrap();
        assert!(workspace.set_active(third));

        workspace.close(first).unwrap();
        assert_eq!(
            workspace.active(),
            Some(third),
            "a background close is inert"
        );
        workspace.close(second).unwrap();
        assert_eq!(workspace.active(), Some(third), "still inert");
        assert_eq!(workspace.nodes().len(), 1);
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

    /// The last usable id is `u64::MAX - 1`: the guard refuses only when
    /// `next_id` is already `u64::MAX`, so the tab below it is created and the
    /// counter lands exactly on the exhausted value. That boundary is the whole
    /// difference between "one id left" and "none left".
    #[test]
    fn the_last_usable_id_is_consumed_before_exhaustion() {
        let mut workspace = Workspace {
            next_id: u64::MAX - 1,
            ..Default::default()
        };
        let last = workspace
            .add_root("last".into())
            .expect("the id below the ceiling is usable");
        assert_eq!(last.get(), u64::MAX - 1);
        // The counter is now exhausted, so the next add is a typed refusal.
        assert_eq!(workspace.add_root("none left".into()), None);
        assert_eq!(workspace.nodes().len(), 1, "the refusal must not add a tab");
        assert_eq!(
            workspace.active(),
            Some(last),
            "the refusal must not move the selection"
        );
    }

    /// Closing a tab frees its slot in the tree but not its id: ids are drawn
    /// from a monotonic counter and never reused, so a stale id held by a caller
    /// can never be mistaken for a newly opened tab.
    #[test]
    fn a_closed_tabs_id_is_never_reused() {
        let mut workspace = Workspace::default();
        let first = workspace.add_root("first".into()).unwrap();
        let second = workspace.add_root("second".into()).unwrap();
        workspace.close(first).unwrap();
        let third = workspace.add_root("third".into()).unwrap();
        assert_ne!(third, first, "a closed id must not come back");
        assert_ne!(third, second);
        assert!(third.get() > second.get());
        // The stale id stays unknown rather than resolving to the new tab.
        assert!(workspace.node(first).is_none());
        assert_eq!(
            workspace.node(third).map(|node| node.title.as_str()),
            Some("third")
        );
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
        if cfg!(debug_assertions) {
            // Debug builds trip the debug_assert on the malformed parent.
            assert!(
                result.is_err(),
                "an ill-formed tree must trip the debug assertion in a debug build; got {result:?}"
            );
        } else {
            // Release builds (e.g. release-fast, which the runtime payload runs)
            // compile the debug_assert out and must tolerate the ill-formed
            // tree flatly, returning depths rather than panicking.
            assert!(
                result.is_ok(),
                "a release build must tolerate an ill-formed tree without panicking"
            );
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
