//! Compatibility exports for core-owned tree depth rules.
//!
//! Keep existing UI imports and public type identity while sharing one owner.
pub use minicon_core::tree::{
    TreeDepthError, TreeDepthNode, compute_tree_depths, compute_tree_depths_by,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_tree_types_are_core_types() {
        let nodes: [minicon_core::tree::TreeDepthNode<u32>; 2] = [
            TreeDepthNode {
                id: 1,
                parent: None,
            },
            TreeDepthNode {
                id: 2,
                parent: Some(1),
            },
        ];
        let depths: Result<Vec<u32>, minicon_core::tree::TreeDepthError<u32>> =
            compute_tree_depths(&nodes);
        assert_eq!(depths, Ok(vec![0, 1]));

        let error: minicon_core::tree::TreeDepthError<u32> = TreeDepthError::MissingParent {
            id: 2,
            parent: 1,
            index: 0,
        };
        assert_eq!(compute_tree_depths(&nodes[1..]), Err(error));
    }

    fn node(id: u32, parent: Option<u32>) -> TreeDepthNode<u32> {
        TreeDepthNode { id, parent }
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
}
