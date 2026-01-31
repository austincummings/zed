use std::{fmt, time::Instant};

use clock::Lamport;
use collections::HashMap;

#[derive(Clone, Debug)]
pub enum TransactionSource {
    User,
    Agent,
}

#[derive(Clone)]
struct UndoNode {
    parent: Option<Lamport>,
    children: Vec<Lamport>,
    last_visited_child: Option<Lamport>,
    timestamp: Instant,
    source: Option<TransactionSource>,
}

#[derive(Clone, Default)]
pub struct UndoTree {
    nodes: HashMap<Lamport, UndoNode>,
    current: Option<Lamport>,
}

impl UndoTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, id: Lamport) {
        if let Some(current) = self.current {
            if let Some(current_node) = self.nodes.get_mut(&current) {
                current_node.children.push(id);
            }
        }
        self.nodes.insert(
            id,
            UndoNode {
                parent: self.current,
                children: Vec::new(),
                last_visited_child: None,
                timestamp: Instant::now(),
                source: None,
            },
        );
        self.current = Some(id);
    }

    pub fn push_with_source(&mut self, id: Lamport, source: TransactionSource) {
        self.push(id);
        if let Some(node) = self.nodes.get_mut(&id) {
            node.source = Some(source);
        }
    }

    pub fn move_to_parent(&mut self) -> Option<Lamport> {
        let current = self.current?;
        let parent = self.nodes.get(&current)?.parent;
        if let Some(parent_id) = parent {
            if let Some(parent_node) = self.nodes.get_mut(&parent_id) {
                parent_node.last_visited_child = Some(current);
            }
        }
        self.current = parent;
        self.current
    }

    pub fn move_to_child(&mut self, child: Lamport) -> Option<Lamport> {
        let current = self.current?;
        let current_node = self.nodes.get(&current)?;
        if !current_node.children.contains(&child) {
            return None;
        }
        if let Some(current_node) = self.nodes.get_mut(&current) {
            current_node.last_visited_child = Some(child);
        }
        self.current = Some(child);
        Some(child)
    }

    pub fn contains(&self, id: Lamport) -> bool {
        self.nodes.contains_key(&id)
    }

    pub fn current(&self) -> Option<Lamport> {
        self.current
    }

    pub fn timestamp_of(&self, id: Lamport) -> Option<Instant> {
        self.nodes.get(&id).map(|node| node.timestamp)
    }

    pub fn children_of(&self, parent: Lamport) -> Vec<Lamport> {
        self.nodes
            .get(&parent)
            .map(|node| node.children.clone())
            .unwrap_or_default()
    }

    pub fn parent_of(&self, id: Lamport) -> Option<Lamport> {
        self.nodes.get(&id)?.parent
    }

    pub fn last_visited_child_of_current(&self) -> Option<Lamport> {
        let current = self.current?;
        self.nodes.get(&current)?.last_visited_child
    }

    pub fn last_visited_child_of(&self, id: Lamport) -> Option<Lamport> {
        self.nodes.get(&id)?.last_visited_child
    }

    pub fn is_at_branch_point(&self) -> bool {
        self.current
            .and_then(|id| self.nodes.get(&id))
            .map(|node| node.children.len() > 1)
            .unwrap_or(false)
    }

    pub fn is_branch_point(&self, id: Lamport) -> bool {
        self.nodes
            .get(&id)
            .map(|node| node.children.len() > 1)
            .unwrap_or(false)
    }

    pub fn branch_points(&self) -> Vec<Lamport> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.children.len() > 1)
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn path_to_current(&self) -> Vec<Lamport> {
        self.path_to_node(self.current)
    }

    pub fn path_to_node(&self, target: Option<Lamport>) -> Vec<Lamport> {
        let mut path = Vec::new();
        let mut node = target;
        while let Some(id) = node {
            path.push(id);
            node = self.nodes.get(&id).and_then(|n| n.parent);
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
        from: Option<Lamport>,
        to: Option<Lamport>,
    ) -> (Vec<Lamport>, Vec<Lamport>) {
        let path_to_from = self.path_to_node(from);
        let path_to_to = self.path_to_node(to);

        let mut common_len = 0;
        for (a, b) in path_to_from.iter().zip(path_to_to.iter()) {
            if a == b {
                common_len += 1;
            } else {
                break;
            }
        }

        let to_undo: Vec<_> = path_to_from[common_len..].iter().rev().copied().collect();
        let to_redo: Vec<_> = path_to_to[common_len..].to_vec();

        (to_undo, to_redo)
    }

    /// Navigate to a specific transaction, updating current and last_visited_child
    /// along the path.
    pub fn navigate_to(&mut self, target: Option<Lamport>) {
        if let Some(target_id) = target {
            let path = self.path_to_node(Some(target_id));
            for window in path.windows(2) {
                if let Some(node) = self.nodes.get_mut(&window[0]) {
                    node.last_visited_child = Some(window[1]);
                }
            }
        }
        self.current = target;
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn mark_transaction_source(
        &mut self,
        transaction_id: Lamport,
        source: TransactionSource,
    ) {
        if let Some(node) = self.nodes.get_mut(&transaction_id) {
            node.source = Some(source);
        }
    }

    fn fmt_node(
        &self,
        f: &mut fmt::Formatter<'_>,
        id: Lamport,
        prefix: &str,
        is_last: bool,
    ) -> fmt::Result {
        let connector = if is_last { "└── " } else { "├── " };
        let marker = if self.current == Some(id) {
            "●"
        } else {
            "○"
        };

        writeln!(f, "{prefix}{connector}{marker} #{}", id.value)?;

        let children = self.children_of(id);
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

        let roots: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.parent.is_none())
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

    fn tx(value: u32) -> Lamport {
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
        let tx2 = tx(2);
        tree.push(tx2);
        let tx3 = tx(3);
        tree.push(tx3);

        assert_eq!(tree.move_to_parent(), Some(tx2));
        assert_eq!(tree.current(), Some(tx2));

        assert_eq!(tree.move_to_child(tx3), Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
    }

    #[test]
    fn test_last_visited_child() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        let tx2 = tx(2);
        tree.push(tx2);
        let tx3 = tx(3);
        tree.push(tx3);

        assert_eq!(tree.move_to_parent(), Some(tx2));
        // Moving to parent sets last_visited_child on tx2
        assert_eq!(tree.last_visited_child_of(tx2), Some(tx3));

        assert_eq!(tree.move_to_child(tx3), Some(tx3));
        // Moving to child sets last_visited_child on tx2
        assert_eq!(tree.last_visited_child_of(tx2), Some(tx3));
    }

    #[test]
    fn test_forking() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        let tx2 = tx(2);
        tree.push(tx2);

        assert_eq!(tree.move_to_parent(), Some(tx1));

        let tx3 = tx(3);
        tree.push(tx3);
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);

        assert_eq!(tree.children_of(tx1), vec![tx2, tx3]);
    }

    #[test]
    fn test_path_to_current() {
        let mut tree = UndoTree::new();

        assert_eq!(tree.path_to_current(), vec![]);

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        assert_eq!(tree.path_to_current(), vec![tx1, tx2, tx3]);

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

        tree.push(tx1);
        tree.push(tx2);
        assert_eq!(tree.branch_points(), vec![]);

        tree.move_to_parent();
        tree.push(tx3);

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

        tree.move_to_parent();
        assert!(!tree.is_at_branch_point());

        tree.push(tx3);
        tree.move_to_parent();

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

        let (to_undo, to_redo) = tree.compute_path(Some(tx3), Some(tx1));
        assert_eq!(to_undo, vec![tx3, tx2]);
        assert_eq!(to_redo, vec![]);

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

        tree.push(tx1);
        tree.push(tx2);
        tree.move_to_parent();
        tree.push(tx3);

        let (to_undo, to_redo) = tree.compute_path(Some(tx2), Some(tx3));
        assert_eq!(to_undo, vec![tx2]);
        assert_eq!(to_redo, vec![tx3]);
    }

    #[test]
    fn test_compute_path_to_root() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);

        tree.push(tx1);
        tree.push(tx2);

        let (to_undo, to_redo) = tree.compute_path(Some(tx2), None);
        assert_eq!(to_undo, vec![tx2, tx1]);
        assert_eq!(to_redo, vec![]);

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

        tree.push(tx1);
        tree.push(tx2);
        tree.move_to_parent();
        tree.push(tx3);

        assert_eq!(tree.current(), Some(tx3));

        tree.navigate_to(Some(tx2));
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.last_visited_child_of(tx1), Some(tx2));

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

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);
        let tx4 = tx(4);
        let tx5 = tx(5);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        tree.move_to_parent();
        tree.move_to_parent();
        tree.push(tx4);
        tree.push(tx5);

        assert_eq!(tree.children_of(tx1), vec![tx2, tx4]);
        assert_eq!(tree.children_of(tx2), vec![tx3]);
        assert_eq!(tree.children_of(tx4), vec![tx5]);

        let (to_undo, to_redo) = tree.compute_path(Some(tx5), Some(tx3));
        assert_eq!(to_undo, vec![tx5, tx4]);
        assert_eq!(to_redo, vec![tx2, tx3]);
    }
}
