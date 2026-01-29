use std::{fmt, time::Instant};

use clock::Lamport;
use collections::HashMap;

pub type TransactionId = Lamport;

#[derive(Clone, Default)]
pub struct UndoTree {
    parents: HashMap<TransactionId, Option<TransactionId>>,
    children: HashMap<TransactionId, Vec<TransactionId>>,
    current: Option<TransactionId>,
    last_visited_child: HashMap<TransactionId, TransactionId>,
    timestamps: HashMap<TransactionId, Instant>,
}

impl UndoTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, id: TransactionId) {
        self.parents.insert(id, self.current);
        self.timestamps.insert(id, Instant::now());
        if let Some(current) = self.current {
            self.children.entry(current).or_default().push(id);
        }
        self.current = Some(id);
    }

    pub fn move_to_parent(&mut self) -> Option<TransactionId> {
        let current = self.current?;
        if let Some(parent) = self.parents.get(&current).copied().flatten() {
            self.last_visited_child.insert(parent, current);
        }
        self.current = self.parents.get(&current).copied().flatten();
        self.current
    }

    pub fn move_to_child(&mut self, child: TransactionId) -> Option<TransactionId> {
        let children = self.current_children()?;
        if children.contains(&child) {
            if let Some(current) = self.current {
                self.last_visited_child.insert(current, child);
            }
            self.current = Some(child);
            Some(child)
        } else {
            None
        }
    }

    pub fn move_to_last_visited_child_of(
        &mut self,
        parent: TransactionId,
    ) -> Option<TransactionId> {
        let last_visited_child = self.last_visited_child.get(&parent).copied();
        if let Some(child) = last_visited_child {
            self.current = Some(child);
            Some(child)
        } else {
            None
        }
    }

    pub fn contains(&self, id: TransactionId) -> bool {
        self.parents.contains_key(&id)
    }

    pub fn current(&self) -> Option<TransactionId> {
        self.current
    }

    pub fn timestamp_of(&self, id: TransactionId) -> Option<Instant> {
        self.timestamps.get(&id).copied()
    }

    pub fn current_children(&self) -> Option<Vec<TransactionId>> {
        if let Some(current) = self.current {
            self.children.get(&current).cloned()
        } else {
            None
        }
    }

    pub fn children_of(&self, parent: TransactionId) -> Vec<TransactionId> {
        self.children.get(&parent).cloned().unwrap_or_default()
    }

    /// Get the parent of a specific node.
    pub fn parent_of(&self, id: TransactionId) -> Option<TransactionId> {
        self.parents.get(&id).copied().flatten()
    }

    /// Get the most recently visited child of the current node (for default redo at branch point).
    pub fn last_visited_child_of_current(&self) -> Option<TransactionId> {
        self.current
            .and_then(|c| self.last_visited_child.get(&c).copied())
    }

    /// Get the most recently visited child of a specific node.
    pub fn last_visited_child_of(&self, id: TransactionId) -> Option<TransactionId> {
        self.last_visited_child.get(&id).copied()
    }

    /// Check if current node has multiple children (is a branch point).
    pub fn is_at_branch_point(&self) -> bool {
        self.current
            .map(|c| self.children_of(c).len() > 1)
            .unwrap_or(false)
    }

    /// Check if a specific node has multiple children (is a branch point).
    pub fn is_branch_point(&self, id: TransactionId) -> bool {
        self.children_of(id).len() > 1
    }

    /// Get all branch points in the tree.
    pub fn branch_points(&self) -> Vec<TransactionId> {
        self.children
            .iter()
            .filter(|(_, kids)| kids.len() > 1)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Get path from root to current (ordered from oldest to newest).
    pub fn path_to_current(&self) -> Vec<TransactionId> {
        self.path_to_node(self.current)
    }

    /// Get path from root to a specific node (ordered from oldest to newest).
    pub fn path_to_node(&self, target: Option<TransactionId>) -> Vec<TransactionId> {
        let mut path = Vec::new();
        let mut node = target;
        while let Some(id) = node {
            path.push(id);
            node = self.parents.get(&id).copied().flatten();
        }
        path.reverse();
        path
    }

    /// Compute navigation path between two nodes.
    ///
    /// Returns `(nodes_to_undo, nodes_to_redo)` - the transactions that need to be
    /// undone and then redone to navigate from `from` to `to`.
    pub fn compute_path(
        &self,
        from: Option<TransactionId>,
        to: Option<TransactionId>,
    ) -> (Vec<TransactionId>, Vec<TransactionId>) {
        let path_to_from = self.path_to_node(from);
        let path_to_to = self.path_to_node(to);

        // Find common ancestor (last shared node in paths)
        let mut common_len = 0;
        for (a, b) in path_to_from.iter().zip(path_to_to.iter()) {
            if a == b {
                common_len += 1;
            } else {
                break;
            }
        }

        // Undo from `from` back to common ancestor (reverse order)
        let to_undo: Vec<_> = path_to_from[common_len..].iter().rev().copied().collect();

        // Redo from common ancestor to `to`
        let to_redo: Vec<_> = path_to_to[common_len..].to_vec();

        (to_undo, to_redo)
    }

    /// Navigate to a specific transaction, updating current and last_visited_child.
    ///
    /// This updates the internal state to reflect that we've navigated to the target,
    /// including updating the last_visited_child along the path.
    pub fn navigate_to(&mut self, target: Option<TransactionId>) {
        if let Some(target_id) = target {
            // Update last_visited_child along the path to target
            let path = self.path_to_node(Some(target_id));
            for i in 0..path.len().saturating_sub(1) {
                self.last_visited_child.insert(path[i], path[i + 1]);
            }
        }
        self.current = target;
    }

    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }

    fn fmt_node(
        &self,
        f: &mut fmt::Formatter<'_>,
        node: TransactionId,
        prefix: &str,
        is_last: bool,
    ) -> fmt::Result {
        let connector = if is_last { "└── " } else { "├── " };
        let marker = if self.current == Some(node) {
            "●"
        } else {
            "○"
        };

        writeln!(f, "{prefix}{connector}{marker} #{}", node.value)?;

        let children = self.children_of(node);
        let child_prefix = if is_last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };

        for (index, child) in children.iter().enumerate() {
            let is_last_child = index == children.len() - 1;
            self.fmt_node(f, *child, &child_prefix, is_last_child)?;
        }

        Ok(())
    }
}

impl fmt::Debug for UndoTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "UndoTree (empty)");
        }

        writeln!(f, "UndoTree")?;

        // Find root nodes (transactions whose parent is None)
        let roots: Vec<_> = self
            .parents
            .iter()
            .filter(|(_, parent)| parent.is_none())
            .map(|(id, _)| *id)
            .collect();

        for (index, root) in roots.iter().enumerate() {
            let is_last_root = index == roots.len() - 1;
            self.fmt_node(f, *root, "", is_last_root)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use clock::ReplicaId;

    use super::*;

    fn tx(value: u32) -> TransactionId {
        Lamport {
            value,
            replica_id: ReplicaId::new(1),
        }
    }

    #[test]
    fn test_empty_tree() {
        let tree = UndoTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        assert_eq!(tree.current(), None);
    }

    #[test]
    fn test_push() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        assert_eq!(tree.current(), Some(tx1));
        assert_eq!(tree.len(), 1);

        let tx2 = tx(2);
        tree.push(tx2);
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.len(), 2);

        let tx3 = tx(3);
        tree.push(tx3);
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_move_to_child_and_parent() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        assert_eq!(tree.current(), Some(tx1));
        assert_eq!(tree.len(), 1);

        let tx2 = tx(2);
        tree.push(tx2);
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.len(), 2);

        let tx3 = tx(3);
        tree.push(tx3);
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);

        assert_eq!(tree.move_to_parent(), Some(tx2));
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.len(), 3);

        assert_eq!(tree.move_to_child(tx3), Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_move_to_last_child() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        assert_eq!(tree.current(), Some(tx1));
        assert_eq!(tree.len(), 1);

        let tx2 = tx(2);
        tree.push(tx2);
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.len(), 2);

        let tx3 = tx(3);
        tree.push(tx3);
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);

        assert_eq!(tree.move_to_parent(), Some(tx2));
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.len(), 3);

        assert_eq!(tree.move_to_child(tx3), Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);

        assert_eq!(tree.move_to_last_visited_child_of(tx2), Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_forking() {
        let mut tree = UndoTree::new();

        // Edit 1
        let tx1 = tx(1);
        tree.push(tx1);
        assert_eq!(tree.current(), Some(tx1));
        assert_eq!(tree.len(), 1);

        // Edit 2
        let tx2 = tx(2);
        tree.push(tx2);
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.len(), 2);

        // Undo edit 2
        assert_eq!(tree.move_to_parent(), Some(tx1));
        assert_eq!(tree.current(), Some(tx1));
        assert_eq!(tree.len(), 2);

        // Add a new transaction on top of edit 1
        let tx3 = tx(3);
        tree.push(tx3);
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);

        // Check that edit 1 has 2 children
        assert_eq!(tree.children_of(tx1), vec![tx2, tx3]);
    }

    #[test]
    fn test_path_to_current() {
        let mut tree = UndoTree::new();

        // Empty tree has empty path
        assert_eq!(tree.path_to_current(), vec![]);

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        // Path should be from root to current
        assert_eq!(tree.path_to_current(), vec![tx1, tx2, tx3]);

        // After undo, path should be shorter
        tree.move_to_parent();
        assert_eq!(tree.path_to_current(), vec![tx1, tx2]);

        tree.move_to_parent();
        assert_eq!(tree.path_to_current(), vec![tx1]);
    }

    #[test]
    fn test_branch_points() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        // Linear history has no branch points
        tree.push(tx1);
        tree.push(tx2);
        assert_eq!(tree.branch_points(), vec![]);

        // Create a branch: undo then push new tx
        tree.move_to_parent(); // at tx1
        tree.push(tx3); // tx1 now has two children: tx2 and tx3

        let branch_points = tree.branch_points();
        assert_eq!(branch_points.len(), 1);
        assert!(branch_points.contains(&tx1));
    }

    #[test]
    fn test_is_at_branch_point() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);

        // Not at branch point yet
        tree.move_to_parent();
        assert!(!tree.is_at_branch_point());

        // Create branch
        tree.push(tx3);
        tree.move_to_parent(); // back at tx1

        // Now at branch point
        assert!(tree.is_at_branch_point());
        assert!(tree.is_branch_point(tx1));
        assert!(!tree.is_branch_point(tx2));
        assert!(!tree.is_branch_point(tx3));
    }

    #[test]
    fn test_compute_path_same_branch() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        // Navigate from tx3 to tx1 (same branch, going back)
        let (to_undo, to_redo) = tree.compute_path(Some(tx3), Some(tx1));
        assert_eq!(to_undo, vec![tx3, tx2]);
        assert_eq!(to_redo, vec![]);

        // Navigate from tx1 to tx3 (same branch, going forward)
        let (to_undo, to_redo) = tree.compute_path(Some(tx1), Some(tx3));
        assert_eq!(to_undo, vec![]);
        assert_eq!(to_redo, vec![tx2, tx3]);
    }

    #[test]
    fn test_compute_path_across_branches() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        // Create: tx1 -> tx2 (branch A)
        //              -> tx3 (branch B)
        tree.push(tx1);
        tree.push(tx2);
        tree.move_to_parent(); // back to tx1
        tree.push(tx3);

        // Navigate from tx2 to tx3 (across branches)
        let (to_undo, to_redo) = tree.compute_path(Some(tx2), Some(tx3));
        assert_eq!(to_undo, vec![tx2]); // undo tx2 to get to tx1
        assert_eq!(to_redo, vec![tx3]); // redo tx3 from tx1
    }

    #[test]
    fn test_compute_path_to_root() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);

        tree.push(tx1);
        tree.push(tx2);

        // Navigate from tx2 to root (None)
        let (to_undo, to_redo) = tree.compute_path(Some(tx2), None);
        assert_eq!(to_undo, vec![tx2, tx1]);
        assert_eq!(to_redo, vec![]);

        // Navigate from root to tx2
        let (to_undo, to_redo) = tree.compute_path(None, Some(tx2));
        assert_eq!(to_undo, vec![]);
        assert_eq!(to_redo, vec![tx1, tx2]);
    }

    #[test]
    fn test_navigate_to() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        // Create: tx1 -> tx2 (branch A)
        //              -> tx3 (branch B)
        tree.push(tx1);
        tree.push(tx2);
        tree.move_to_parent();
        tree.push(tx3);

        // Currently at tx3
        assert_eq!(tree.current(), Some(tx3));

        // Navigate to tx2
        tree.navigate_to(Some(tx2));
        assert_eq!(tree.current(), Some(tx2));

        // last_visited_child should be updated along the path
        assert_eq!(tree.last_visited_child_of(tx1), Some(tx2));

        // Navigate to tx3
        tree.navigate_to(Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.last_visited_child_of(tx1), Some(tx3));
    }

    #[test]
    fn test_parent_of() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        assert_eq!(tree.parent_of(tx1), None);
        assert_eq!(tree.parent_of(tx2), Some(tx1));
        assert_eq!(tree.parent_of(tx3), Some(tx2));
    }

    #[test]
    fn test_deep_branching() {
        let mut tree = UndoTree::new();

        // Create a more complex tree:
        //        tx1
        //       /   \
        //     tx2   tx4
        //     /       \
        //   tx3       tx5

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);
        let tx4 = tx(4);
        let tx5 = tx(5);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        // Go back to tx1 and create another branch
        tree.move_to_parent(); // tx2
        tree.move_to_parent(); // tx1
        tree.push(tx4);
        tree.push(tx5);

        // Verify structure
        assert_eq!(tree.children_of(tx1), vec![tx2, tx4]);
        assert_eq!(tree.children_of(tx2), vec![tx3]);
        assert_eq!(tree.children_of(tx4), vec![tx5]);

        // Navigate from tx5 to tx3
        let (to_undo, to_redo) = tree.compute_path(Some(tx5), Some(tx3));
        assert_eq!(to_undo, vec![tx5, tx4]); // undo tx5, tx4 to get to tx1
        assert_eq!(to_redo, vec![tx2, tx3]); // redo tx2, tx3 from tx1
    }
}
