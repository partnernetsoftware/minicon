//! Tree depth derivation for hierarchical tab layouts.
//!
//! The workspace keeps tabs in insertion order and records each tab's parent, so
//! a flat parent list has to be turned back into depths before anything can draw
//! it. That is pure arithmetic over ids, which is why it lives here rather than
//! next to a window: nothing about it is platform-specific, and a consumer that
//! only wants the layout rules should not have to take a rendering layer to get
//! them.
//!
//! The traversal is iterative. A depth chain is bounded only by the tab count,
//! and a recursive version would place that bound on the host's stack — the
//! kind of limit that fails in the field rather than in a test.

/// A node's identity and its optional parent, in input order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeDepthNode<Id> {
    pub id: Id,
    pub parent: Option<Id>,
}

/// Why a parent list is not a well-formed tree.
///
/// Typed rather than a string: callers decide whether the condition is a bug in
/// their own construction (`debug_assert`) or bad input they must report, and a
/// message would force an allocation on a path that is usually the happy one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeDepthError<Id> {
    /// Two nodes share an id, so parentage is ambiguous.
    DuplicateId { id: Id, index: usize },
    /// A node names a parent that is not in the list.
    MissingParent { id: Id, parent: Id, index: usize },
    /// Parentage reaches back on itself, so no depth exists.
    Cycle { id: Id, index: usize },
}

/// Computes every node's depth from its parent, for nodes that expose the
/// parentage as fields.
pub fn compute_tree_depths<Id>(nodes: &[TreeDepthNode<Id>]) -> Result<Vec<u32>, TreeDepthError<Id>>
where
    Id: Copy + Ord,
{
    compute_tree_depths_by(nodes, |node| node.id, |node| node.parent)
}

/// Computes every node's depth from its parent, reading parentage through
/// accessors so a caller can keep its own node type.
///
/// The result is parallel to `nodes`: `depths[i]` is the depth of `nodes[i]`, so
/// a caller that displays nodes in input order does not have to reorder.
pub fn compute_tree_depths_by<Node, Id, IdOf, ParentOf>(
    nodes: &[Node],
    id_of: IdOf,
    parent_of: ParentOf,
) -> Result<Vec<u32>, TreeDepthError<Id>>
where
    Id: Copy + Ord,
    IdOf: Fn(&Node) -> Id,
    ParentOf: Fn(&Node) -> Option<Id>,
{
    let mut ids = Vec::with_capacity(nodes.len());
    let mut parents = Vec::with_capacity(nodes.len());
    for node in nodes {
        ids.push(id_of(node));
        parents.push(parent_of(node));
    }

    // Ids need not be dense or ordered, so index them once and binary-search
    // after, rather than scanning per parent lookup. Ids do not have to be
    // `Ord` for this to be correct, only consistent, and requiring it keeps the
    // lookup honest.
    let mut indexes: Vec<(Id, usize)> = ids.iter().copied().zip(0..nodes.len()).collect();
    sort_index_pairs(&mut indexes);
    for duplicate in indexes.windows(2) {
        if duplicate[0].0 == duplicate[1].0 {
            return Err(TreeDepthError::DuplicateId {
                id: duplicate[1].0,
                index: duplicate[1].1,
            });
        }
    }

    let index_of = |id: Id| {
        indexes
            .binary_search_by_key(&id, |(candidate, _)| *candidate)
            .ok()
            .map(|index| indexes[index].1)
    };

    // Validate and resolve parentage in one pass, keeping the resolved index
    // rather than the id. The walk below then indexes directly, so there is no
    // second lookup and no `expect` on a "this cannot fail" invariant: the only
    // way to get a `None` here is the `MissingParent` return above, and after
    // that every entry is either the root or a valid index into `nodes`.
    //
    // The alternative -- resolving again inside the walk -- is what the original
    // did, and it is the kind of unreachable panic that is invisible until it is
    // not: this crate's output feeds a windowed process with no console.
    let mut parent_indexes = Vec::with_capacity(parents.len());
    for (index, parent) in parents.iter().copied().enumerate() {
        match parent {
            None => parent_indexes.push(None),
            Some(parent) => match index_of(parent) {
                Some(parent_index) => parent_indexes.push(Some(parent_index)),
                None => {
                    return Err(TreeDepthError::MissingParent {
                        id: ids[index],
                        parent,
                        index,
                    });
                }
            },
        }
    }

    let mut depths = vec![0_u32; nodes.len()];
    // 0 unvisited, 1 on the current path, 2 settled. A node still marked 1 when
    // the walk returns to it is a cycle, which is what makes the check free:
    // no separate visited set, and no recursion.
    let mut state = vec![0_u8; nodes.len()];
    let mut path = Vec::with_capacity(nodes.len());

    for start in 0..nodes.len() {
        if state[start] != 0 {
            continue;
        }

        path.clear();
        let mut current = start;
        loop {
            match state[current] {
                0 => {}
                1 => {
                    return Err(TreeDepthError::Cycle {
                        id: ids[current],
                        index: current,
                    });
                }
                _ => break,
            }

            state[current] = 1;
            path.push(current);
            match parent_indexes[current] {
                Some(parent_index) => current = parent_index,
                None => break,
            }
        }

        // Settle the path from the deepest node back towards its root, so each
        // depth is already known when it is needed.
        for &index in path.iter().rev() {
            depths[index] = parent_indexes[index]
                .map(|parent_index| depths[parent_index].saturating_add(1))
                .unwrap_or(0);
            state[index] = 2;
        }
    }

    Ok(depths)
}

/// Heap-sorts id/index pairs.
///
/// Written out rather than calling `sort_by_key` so the crate keeps its
/// dependency-free build: the codec exists because the usual JSON crate is too
/// large for this product's budget, and reaching for a std allocation-light
/// helper here would reintroduce the same trade in a less visible place.
fn sort_index_pairs<Id: Ord>(values: &mut [(Id, usize)]) {
    let len = values.len();
    for root in (0..len / 2).rev() {
        sift_down(values, root, len);
    }
    for end in (1..len).rev() {
        values.swap(0, end);
        sift_down(values, 0, end);
    }
}

fn sift_down<Id: Ord>(values: &mut [(Id, usize)], mut root: usize, end: usize) {
    loop {
        let left = root * 2 + 1;
        if left >= end {
            return;
        }
        let right = left + 1;
        let child = if right < end && values[left] < values[right] {
            right
        } else {
            left
        };
        if values[root] >= values[child] {
            return;
        }
        values.swap(root, child);
        root = child;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, parent: Option<u32>) -> TreeDepthNode<u32> {
        TreeDepthNode { id, parent }
    }

    /// Depths do not depend on the order the walk happens to settle nodes in.
    ///
    /// The walk resolves each parent to an index once, up front, and then
    /// follows those indexes; an earlier version re-resolved the parent id while
    /// walking and settled the path from the deepest node back. Both are
    /// supposed to produce the same depths, and this checks that on shapes
    /// where the two orders differ: a parent declared after its child, a forest
    /// of several roots interleaved in the input, and a chain that shares a
    /// prefix with a sibling branch.
    #[test]
    fn depth_does_not_depend_on_the_order_nodes_are_settled_in() {
        for nodes in [
            vec![node(3, Some(2)), node(1, None), node(2, Some(1))],
            vec![node(5, Some(1)), node(9, None), node(1, Some(9)), node(4, None)],
            vec![
                node(1, None),
                node(2, Some(1)),
                node(3, Some(2)),
                node(4, Some(2)),
                node(5, Some(3)),
            ],
        ] {
            let depths = compute_tree_depths(&nodes).expect("well-formed");
            // Recompute through the accessor form on a by-value copy, which
            // builds its own index and so cannot share the first pass's state.
            let copies: Vec<_> = nodes
                .iter()
                .map(|n| TreeDepthNode { id: n.id, parent: n.parent })
                .collect();
            let again = compute_tree_depths_by(&copies, |n| n.id, |n| n.parent)
                .expect("well-formed");
            assert_eq!(depths, again);
            // Every depth is one more than its parent's, which is the property
            // the ordering must not violate.
            for (index, n) in nodes.iter().enumerate() {
                if let Some(parent) = n.parent {
                    let parent_index = nodes.iter().position(|c| c.id == parent).unwrap();
                    assert_eq!(
                        depths[index],
                        depths[parent_index] + 1,
                        "node {} under {parent}",
                        n.id
                    );
                }
            }
        }
    }

    #[test]
    fn ordinary_tree_keeps_input_order() {
        let nodes = [
            node(1, None),
            node(2, Some(1)),
            node(3, Some(1)),
            node(4, Some(2)),
        ];
        assert_eq!(compute_tree_depths(&nodes), Ok(vec![0, 1, 1, 2]));
    }

    #[test]
    fn parent_may_appear_after_child() {
        let nodes = [node(3, Some(2)), node(1, None), node(2, Some(1))];
        assert_eq!(compute_tree_depths(&nodes), Ok(vec![2, 0, 1]));
    }

    #[test]
    fn deep_chain_is_iterative() {
        let mut nodes = Vec::with_capacity(20_000);
        for id in 0..20_000_u32 {
            let parent = if id == 0 { None } else { Some(id - 1) };
            nodes.push(node(id, parent));
        }

        let depths = compute_tree_depths(&nodes).unwrap();
        assert_eq!(depths[0], 0);
        assert_eq!(depths[19_999], 19_999);
    }

    #[test]
    fn missing_parent_is_typed() {
        let error = compute_tree_depths(&[node(1, Some(9))]).unwrap_err();
        assert_eq!(
            error,
            TreeDepthError::MissingParent {
                id: 1,
                parent: 9,
                index: 0,
            }
        );
    }

    #[test]
    fn duplicate_id_is_typed() {
        let error = compute_tree_depths(&[node(1, None), node(1, None)]).unwrap_err();
        assert_eq!(error, TreeDepthError::DuplicateId { id: 1, index: 1 });
    }

    #[test]
    fn self_cycle_is_typed() {
        let error = compute_tree_depths(&[node(7, Some(7))]).unwrap_err();
        assert_eq!(error, TreeDepthError::Cycle { id: 7, index: 0 });
    }

    #[test]
    fn multi_node_cycle_is_typed_without_recursion() {
        let nodes = [node(1, Some(3)), node(2, Some(1)), node(3, Some(2))];
        assert!(matches!(
            compute_tree_depths(&nodes),
            Err(TreeDepthError::Cycle { .. })
        ));
    }

    /// The accessor form is the one `minicon` itself calls, and it is the form a
    /// caller with its own node type uses. Reading parentage through closures is
    /// what makes the helper reusable rather than tied to `TreeDepthNode`, so it
    /// is checked directly instead of only via the field wrapper above.
    #[test]
    fn accessor_form_reads_parentage_from_the_callers_node_type() {
        struct Tab {
            number: u64,
            up: Option<u64>,
        }
        let tabs = [
            Tab { number: 10, up: None },
            Tab { number: 20, up: Some(10) },
            Tab { number: 30, up: Some(20) },
        ];
        let depths =
            compute_tree_depths_by(&tabs, |tab| tab.number, |tab| tab.up).expect("well-formed");
        assert_eq!(depths, vec![0, 1, 2]);
    }

    /// A wide and unordered id space is where a linear parent lookup degrades,
    /// and it is also where a wrong index mapping shows up as a wrong depth.
    #[test]
    fn sparse_ids_resolve_to_their_own_depths() {
        let nodes = [
            node(1_000_000, None),
            node(7, Some(1_000_000)),
            node(42, Some(7)),
            node(2, None),
        ];
        assert_eq!(compute_tree_depths(&nodes), Ok(vec![0, 1, 2, 0]));
    }
}
