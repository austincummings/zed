use std::{fmt, time::Instant};

use clock::Lamport;
use collections::HashMap;

/// A unique identifier for a transaction in the undo tree.
pub type TransactionId = Lamport;

/// Indicates whether a transaction was created by the user or by an AI agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionSource {
    User,
    Agent,
}

/// Information about a node in the undo tree.
///
/// This struct provides access to all node data in a single lookup,
/// which is more efficient than calling individual accessor methods.
#[derive(Clone, Debug)]
pub struct NodeInfo<'a> {
    pub parent: Option<TransactionId>,
    pub children: &'a [TransactionId],
    pub last_visited_child: Option<TransactionId>,
    pub timestamp: Instant,
    pub source: Option<&'a TransactionSource>,
}

impl NodeInfo<'_> {
    /// Check if this node is a branch point (has multiple children).
    pub fn is_branch_point(&self) -> bool {
        self.children.len() > 1
    }
}

#[derive(Clone)]
struct UndoNode {
    parent: Option<TransactionId>,
    children: Vec<TransactionId>,
    last_visited_child: Option<TransactionId>,
    timestamp: Instant,
    source: Option<TransactionSource>,
}

/// A tree structure that tracks the history of undo/redo operations,
/// preserving branches when edits are made after undoing.
#[derive(Clone, Default)]
pub struct UndoTree {
    nodes: HashMap<TransactionId, UndoNode>,
    current: Option<TransactionId>,
}

impl UndoTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new transaction as a child of the current position.
    /// The new transaction becomes the current position.
    pub fn push(&mut self, id: TransactionId) {
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

    /// Set the source (User or Agent) for a transaction.
    pub fn set_source(&mut self, id: TransactionId, source: TransactionSource) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.source = Some(source);
        }
    }

    /// Move current position to parent (used during undo).
    /// Updates last_visited_child on the parent to track the path taken.
    /// Returns the new current position.
    pub fn undo(&mut self) -> Option<TransactionId> {
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

    /// Move current position to a specific child (used during redo).
    /// Updates last_visited_child on current to track the path taken.
    /// Returns the child id if successful, None if child is not valid.
    pub fn redo(&mut self, child: TransactionId) -> Option<TransactionId> {
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

    /// Navigate to a specific transaction, updating current and last_visited_child
    /// along the path. Used for branch switching.
    pub fn navigate_to(&mut self, target: Option<TransactionId>) {
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

    /// Check if a transaction exists in the tree.
    pub fn contains(&self, id: TransactionId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Get the current position in the tree.
    pub fn current(&self) -> Option<TransactionId> {
        self.current
    }

    /// Get the number of transactions in the tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Check if a specific transaction is a branch point (has multiple children).
    pub fn is_branch_point(&self, id: TransactionId) -> bool {
        self.nodes
            .get(&id)
            .map(|node| node.children.len() > 1)
            .unwrap_or(false)
    }

    /// Get all branch points in the tree.
    pub fn branch_points(&self) -> Vec<TransactionId> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.children.len() > 1)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Create a cursor at the current position.
    pub fn cursor(&self) -> Cursor<'_> {
        Cursor {
            tree: self,
            position: self.current,
        }
    }

    /// Create a cursor at a specific position.
    pub fn cursor_at(&self, position: Option<TransactionId>) -> Cursor<'_> {
        Cursor {
            tree: self,
            position,
        }
    }

    fn path_to_node(&self, target: Option<TransactionId>) -> Vec<TransactionId> {
        let mut path = Vec::new();
        let mut node = target;
        while let Some(id) = node {
            path.push(id);
            node = self.nodes.get(&id).and_then(|n| n.parent);
        }
        path.reverse();
        path
    }

    fn children_slice(&self, id: TransactionId) -> &[TransactionId] {
        self.nodes
            .get(&id)
            .map(|node| node.children.as_slice())
            .unwrap_or(&[])
    }

    fn fmt_node(
        &self,
        f: &mut fmt::Formatter<'_>,
        id: TransactionId,
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

        let children = self.children_slice(id);
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

/// A cursor for exploring an UndoTree without modifying its current position.
///
/// Cursors provide read-only access to the tree structure and can be moved
/// around independently of the tree's actual current position.
#[derive(Clone)]
pub struct Cursor<'a> {
    tree: &'a UndoTree,
    position: Option<TransactionId>,
}

impl<'a> Cursor<'a> {
    /// Get the transaction id at the cursor's current position.
    pub fn id(&self) -> Option<TransactionId> {
        self.position
    }

    /// Get all information about the node at the cursor's current position.
    ///
    /// This is more efficient than calling individual accessor methods when
    /// multiple properties are needed, as it performs a single lookup.
    pub fn node(&self) -> Option<NodeInfo<'a>> {
        let id = self.position?;
        let node = self.tree.nodes.get(&id)?;
        Some(NodeInfo {
            parent: node.parent,
            children: &node.children,
            last_visited_child: node.last_visited_child,
            timestamp: node.timestamp,
            source: node.source.as_ref(),
        })
    }

    /// Get the parent of the cursor's current position.
    pub fn parent(&self) -> Option<TransactionId> {
        self.position
            .and_then(|id| self.tree.nodes.get(&id))
            .and_then(|node| node.parent)
    }

    /// Get the children of the cursor's current position.
    pub fn children(&self) -> &'a [TransactionId] {
        self.position
            .map(|id| self.tree.children_slice(id))
            .unwrap_or(&[])
    }

    /// Get the timestamp of the cursor's current position.
    pub fn timestamp(&self) -> Option<Instant> {
        self.position
            .and_then(|id| self.tree.nodes.get(&id))
            .map(|node| node.timestamp)
    }

    /// Get the source (User or Agent) of the cursor's current position.
    pub fn source(&self) -> Option<&'a TransactionSource> {
        self.position
            .and_then(|id| self.tree.nodes.get(&id))
            .and_then(|node| node.source.as_ref())
    }

    /// Get the last visited child of the cursor's current position.
    pub fn last_visited_child(&self) -> Option<TransactionId> {
        self.position
            .and_then(|id| self.tree.nodes.get(&id))
            .and_then(|node| node.last_visited_child)
    }

    /// Check if the cursor's current position is a branch point.
    pub fn is_branch_point(&self) -> bool {
        self.position
            .map(|id| self.tree.is_branch_point(id))
            .unwrap_or(false)
    }

    /// Move the cursor to its parent position.
    /// Returns true if the cursor moved, false if already at root.
    pub fn move_up(&mut self) -> bool {
        if let Some(parent) = self.parent() {
            self.position = Some(parent);
            true
        } else if self.position.is_some() {
            self.position = None;
            true
        } else {
            false
        }
    }

    /// Move the cursor to a specific child.
    /// Returns true if the cursor moved, false if the child is invalid.
    pub fn move_down(&mut self, child: TransactionId) -> bool {
        if self.children().contains(&child) {
            self.position = Some(child);
            true
        } else {
            false
        }
    }

    /// Move the cursor to an arbitrary position.
    pub fn move_to(&mut self, target: Option<TransactionId>) {
        self.position = target;
    }

    /// Compute the navigation path from the cursor's position to a target.
    ///
    /// Returns `(to_undo, to_redo)` - the transactions that need to be
    /// undone and then redone to navigate from current position to target.
    pub fn path_to(
        &self,
        target: Option<TransactionId>,
    ) -> (Vec<TransactionId>, Vec<TransactionId>) {
        let path_to_from = self.tree.path_to_node(self.position);
        let path_to_to = self.tree.path_to_node(target);

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

    /// Get an iterator over ancestors, from the cursor's position up to the root.
    pub fn ancestors(&self) -> Ancestors<'a> {
        Ancestors {
            tree: self.tree,
            current: self.position,
        }
    }

    /// Get the path from the root to the cursor's current position.
    pub fn path_from_root(&self) -> Vec<TransactionId> {
        self.tree.path_to_node(self.position)
    }
}

/// An iterator that yields transaction ids from a starting position up to the root.
pub struct Ancestors<'a> {
    tree: &'a UndoTree,
    current: Option<TransactionId>,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = TransactionId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.current?;
        self.current = self.tree.nodes.get(&id).and_then(|n| n.parent);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use clock::ReplicaId;

    use super::*;

    fn tx(value: u32) -> TransactionId {
        TransactionId {
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

        let cursor = tree.cursor();
        assert_eq!(cursor.id(), None);
        assert_eq!(cursor.parent(), None);
        assert!(cursor.children().is_empty());
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
    fn test_undo_redo() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        let tx2 = tx(2);
        tree.push(tx2);
        let tx3 = tx(3);
        tree.push(tx3);

        assert_eq!(tree.undo(), Some(tx2));
        assert_eq!(tree.current(), Some(tx2));

        assert_eq!(tree.redo(tx3), Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
    }

    #[test]
    fn test_cursor_navigation() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        let tx2 = tx(2);
        tree.push(tx2);
        let tx3 = tx(3);
        tree.push(tx3);

        let mut cursor = tree.cursor();
        assert_eq!(cursor.id(), Some(tx3));

        assert!(cursor.move_up());
        assert_eq!(cursor.id(), Some(tx2));

        assert!(cursor.move_up());
        assert_eq!(cursor.id(), Some(tx1));

        assert!(cursor.move_down(tx2));
        assert_eq!(cursor.id(), Some(tx2));

        // Tree's current is unchanged
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

        tree.undo();
        // Moving to parent sets last_visited_child on tx2
        let cursor = tree.cursor_at(Some(tx2));
        assert_eq!(cursor.last_visited_child(), Some(tx3));

        tree.redo(tx3);
        // Moving to child keeps last_visited_child on tx2
        let cursor = tree.cursor_at(Some(tx2));
        assert_eq!(cursor.last_visited_child(), Some(tx3));
    }

    #[test]
    fn test_forking() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        let tx2 = tx(2);
        tree.push(tx2);

        tree.undo();

        let tx3 = tx(3);
        tree.push(tx3);
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.len(), 3);

        let cursor = tree.cursor_at(Some(tx1));
        assert_eq!(cursor.children(), &[tx2, tx3]);
    }

    #[test]
    fn test_path_from_root() {
        let mut tree = UndoTree::new();

        let cursor = tree.cursor();
        assert_eq!(cursor.path_from_root(), vec![]);

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        let cursor = tree.cursor();
        assert_eq!(cursor.path_from_root(), vec![tx1, tx2, tx3]);

        tree.undo();
        let cursor = tree.cursor();
        assert_eq!(cursor.path_from_root(), vec![tx1, tx2]);

        tree.undo();
        let cursor = tree.cursor();
        assert_eq!(cursor.path_from_root(), vec![tx1]);
    }

    #[test]
    fn test_ancestors() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        let cursor = tree.cursor();
        let ancestors: Vec<_> = cursor.ancestors().collect();
        assert_eq!(ancestors, vec![tx3, tx2, tx1]);
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

        tree.undo();
        tree.push(tx3);

        let branch_points = tree.branch_points();
        assert_eq!(branch_points.len(), 1);
        assert!(branch_points.contains(&tx1));
    }

    #[test]
    fn test_is_branch_point() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);

        tree.undo();
        assert!(!tree.cursor().is_branch_point());

        tree.push(tx3);
        tree.undo();

        assert!(tree.cursor().is_branch_point());
        assert!(tree.is_branch_point(tx1));
        assert!(!tree.is_branch_point(tx2));
        assert!(!tree.is_branch_point(tx3));
    }

    #[test]
    fn test_path_to_same_branch() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        let cursor = tree.cursor_at(Some(tx3));
        let (to_undo, to_redo) = cursor.path_to(Some(tx1));
        assert_eq!(to_undo, vec![tx3, tx2]);
        assert_eq!(to_redo, vec![]);

        let cursor = tree.cursor_at(Some(tx1));
        let (to_undo, to_redo) = cursor.path_to(Some(tx3));
        assert_eq!(to_undo, vec![]);
        assert_eq!(to_redo, vec![tx2, tx3]);
    }

    #[test]
    fn test_path_to_across_branches() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.undo();
        tree.push(tx3);

        let cursor = tree.cursor_at(Some(tx2));
        let (to_undo, to_redo) = cursor.path_to(Some(tx3));
        assert_eq!(to_undo, vec![tx2]);
        assert_eq!(to_redo, vec![tx3]);
    }

    #[test]
    fn test_path_to_root() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);

        tree.push(tx1);
        tree.push(tx2);

        let cursor = tree.cursor_at(Some(tx2));
        let (to_undo, to_redo) = cursor.path_to(None);
        assert_eq!(to_undo, vec![tx2, tx1]);
        assert_eq!(to_redo, vec![]);

        let cursor = tree.cursor_at(None);
        let (to_undo, to_redo) = cursor.path_to(Some(tx2));
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
        tree.undo();
        tree.push(tx3);

        assert_eq!(tree.current(), Some(tx3));

        tree.navigate_to(Some(tx2));
        assert_eq!(tree.current(), Some(tx2));
        assert_eq!(tree.cursor_at(Some(tx1)).last_visited_child(), Some(tx2));

        tree.navigate_to(Some(tx3));
        assert_eq!(tree.current(), Some(tx3));
        assert_eq!(tree.cursor_at(Some(tx1)).last_visited_child(), Some(tx3));
    }

    #[test]
    fn test_parent() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.push(tx3);

        assert_eq!(tree.cursor_at(Some(tx1)).parent(), None);
        assert_eq!(tree.cursor_at(Some(tx2)).parent(), Some(tx1));
        assert_eq!(tree.cursor_at(Some(tx3)).parent(), Some(tx2));
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

        tree.undo();
        tree.undo();
        tree.push(tx4);
        tree.push(tx5);

        assert_eq!(tree.cursor_at(Some(tx1)).children(), &[tx2, tx4]);
        assert_eq!(tree.cursor_at(Some(tx2)).children(), &[tx3]);
        assert_eq!(tree.cursor_at(Some(tx4)).children(), &[tx5]);

        let cursor = tree.cursor_at(Some(tx5));
        let (to_undo, to_redo) = cursor.path_to(Some(tx3));
        assert_eq!(to_undo, vec![tx5, tx4]);
        assert_eq!(to_redo, vec![tx2, tx3]);
    }

    #[test]
    fn test_set_source() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        tree.push(tx1);
        tree.set_source(tx1, TransactionSource::Agent);

        let cursor = tree.cursor();
        assert_eq!(cursor.source(), Some(&TransactionSource::Agent));
    }

    #[test]
    fn test_cursor_node() {
        let mut tree = UndoTree::new();

        let tx1 = tx(1);
        let tx2 = tx(2);
        let tx3 = tx(3);

        tree.push(tx1);
        tree.push(tx2);
        tree.set_source(tx2, TransactionSource::User);

        tree.undo();
        tree.push(tx3);
        tree.set_source(tx3, TransactionSource::Agent);

        // Test node() on a branch point
        let cursor = tree.cursor_at(Some(tx1));
        let node = cursor.node().expect("node should exist");
        assert_eq!(node.parent, None);
        assert_eq!(node.children, &[tx2, tx3]);
        assert!(node.is_branch_point());
        assert_eq!(node.source, None);

        // Test node() on a leaf with source
        let cursor = tree.cursor_at(Some(tx3));
        let node = cursor.node().expect("node should exist");
        assert_eq!(node.parent, Some(tx1));
        assert!(node.children.is_empty());
        assert!(!node.is_branch_point());
        assert_eq!(node.source, Some(&TransactionSource::Agent));

        // Test node() at None position
        let cursor = tree.cursor_at(None);
        assert!(cursor.node().is_none());
    }
}
