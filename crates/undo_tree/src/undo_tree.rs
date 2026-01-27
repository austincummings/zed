use std::fmt;

use clock::Lamport;
use collections::HashMap;

pub type TransactionId = Lamport;

#[derive(Clone, Default)]
pub struct UndoTree {
    parents: HashMap<TransactionId, Option<TransactionId>>,
    children: HashMap<TransactionId, Vec<TransactionId>>,
    current: Option<TransactionId>,
    last_visited_child: HashMap<TransactionId, TransactionId>,
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

impl UndoTree {
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

impl UndoTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, id: TransactionId) {
        self.parents.insert(id, self.current);
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
        let children = self.children_of_current();
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

    pub fn current(&self) -> Option<TransactionId> {
        self.current
    }

    pub fn children_of_current(&self) -> Vec<TransactionId> {
        self.current
            .and_then(|c| self.children.get(&c))
            .cloned()
            .unwrap_or_default()
    }

    pub fn children_of(&self, parent: TransactionId) -> Vec<TransactionId> {
        self.children.get(&parent).cloned().unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
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
}
