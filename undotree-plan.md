# Undo Tree MVP - Detailed Implementation Plan

## Goal

Preserve edit branches when editing after undo, and provide basic UI to navigate between branches. This is a minimal viable implementation that can be extended later.

**What's in scope:**
- Preserve redo branches instead of clearing them
- Track branch structure as metadata
- Simple panel UI showing branches
- Click to switch branches
- Basic keyboard navigation

**What's deferred:**
- Persistence (history still lost on restart)
- Time-based navigation
- AI edit tagging
- Graph visualization (start with simple list)
- Diff previews
- Bookmarks

---

## Current Architecture Analysis

### Key Data Structures

**Location:** `crates/text/src/text.rs`

```rust
// Line 50 - TransactionId is a Lamport clock timestamp
pub type TransactionId = clock::Lamport;

// Lines 146-153 - History stores all operations permanently
struct History {
    base_text: Rope,
    operations: TreeMap<clock::Lamport, Operation>,  // ALL operations preserved forever
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,  // CLEARED on new edit (line 248)
    transaction_depth: usize,
    group_interval: Duration,  // 300ms default
}

// Lines 120-137 - Each history entry wraps a transaction
pub struct HistoryEntry {
    transaction: Transaction,
    first_edit_at: Instant,
    last_edit_at: Instant,
    suppress_grouping: bool,
}

// Lines 124-133 - Transaction contains edit references
pub struct Transaction {
    pub id: TransactionId,
    pub edit_ids: Vec<clock::Lamport>,  // References to edit operations
    pub start: clock::Global,
}
```

### Critical Code Path - Where Branches Are Lost

**Location:** `crates/text/src/text.rs`, lines 233-256

```rust
fn end_transaction(&mut self, now: Instant) -> Option<&HistoryEntry> {
    assert_ne!(self.transaction_depth, 0);
    self.transaction_depth -= 1;
    if self.transaction_depth == 0 {
        if self.undo_stack.last().unwrap().transaction.edit_ids.is_empty() {
            self.undo_stack.pop();
            None
        } else {
            self.redo_stack.clear();  // <-- LINE 248: THIS IS WHERE BRANCHES ARE LOST
            let entry = self.undo_stack.last_mut().unwrap();
            entry.last_edit_at = now;
            Some(entry)
        }
    } else {
        None
    }
}
```

### Why Branch Preservation Is Possible

The `operations` TreeMap stores ALL operations permanently. The `UndoMap` (in `crates/text/src/undo_map.rs`) tracks undo counts per edit - edits aren't deleted, they're just toggled visible/invisible:

```rust
// undo_map.rs - Visibility is determined by undo count parity
pub fn is_undone(&self, edit_id: clock::Lamport) -> bool {
    self.undo_count(edit_id) % 2 == 1  // Odd count = undone/invisible
}
```

This means we can navigate to any historical state by toggling the right edit visibility.

---

## Phase 1: Create `undo_tree` Crate

### 1.1 Directory Structure

```
crates/undo_tree/
├── Cargo.toml
└── src/
    ├── undo_tree.rs      # Main library file (Zed convention: named after crate)
    └── tests.rs          # Unit tests
```

### 1.2 Cargo.toml

**File:** `crates/undo_tree/Cargo.toml`

```toml
[package]
name = "undo_tree"
version = "0.1.0"
edition.workspace = true
publish.workspace = true
license = "GPL-3.0-or-later"

[lints]
workspace = true

[lib]
path = "src/undo_tree.rs"
doctest = false

[features]
test-support = []

[dependencies]
clock.workspace = true
collections.workspace = true

[dev-dependencies]
clock = { workspace = true, features = ["test-support"] }
```

### 1.3 Main Library File

**File:** `crates/undo_tree/src/undo_tree.rs`

```rust
//! Undo tree data structure for tracking branching edit history.
//!
//! This crate provides `UndoTree`, a data structure that tracks the tree structure
//! of undo history. It maintains parent/child relationships between transactions,
//! enabling navigation across branches when users edit after undoing.

mod tests;

use clock::Lamport;
use collections::HashMap;

/// A transaction identifier, which is a Lamport timestamp.
pub type TransactionId = Lamport;

/// Tracks the tree structure of undo history.
///
/// This is metadata on top of existing History — not a replacement for the
/// undo/redo stacks. It tracks parent/child relationships between transactions
/// to enable branch navigation.
#[derive(Clone, Debug, Default)]
pub struct UndoTree {
    /// Parent of each transaction (None for root means it's the initial state)
    parents: HashMap<TransactionId, Option<TransactionId>>,

    /// Children of each transaction (multiple children = branch point)
    children: HashMap<TransactionId, Vec<TransactionId>>,

    /// Current position in the tree (None = at initial state before any edits)
    current: Option<TransactionId>,

    /// Track which child was most recently visited for each parent.
    /// Used for default redo behavior at branch points.
    last_visited_child: HashMap<TransactionId, TransactionId>,
}

impl UndoTree {
    /// Create a new empty undo tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new transaction as child of current position.
    ///
    /// This should be called when a new transaction is committed after an edit.
    pub fn push(&mut self, id: TransactionId) {
        self.parents.insert(id, self.current);
        if let Some(current) = self.current {
            self.children.entry(current).or_default().push(id);
        }
        self.current = Some(id);
    }

    /// Move current pointer to parent (called on undo).
    ///
    /// Returns the new current position (the parent), or None if already at root.
    pub fn move_to_parent(&mut self) -> Option<TransactionId> {
        let current = self.current?;
        if let Some(parent) = self.parents.get(&current).copied().flatten() {
            self.last_visited_child.insert(parent, current);
        }
        self.current = self.parents.get(&current).copied().flatten();
        self.current
    }

    /// Move current pointer to a specific child (called on redo).
    ///
    /// Returns true if the child was valid and navigation succeeded.
    pub fn move_to_child(&mut self, child: TransactionId) -> bool {
        let children = self.children_of_current();
        if children.contains(&child) {
            if let Some(current) = self.current {
                self.last_visited_child.insert(current, child);
            }
            self.current = Some(child);
            true
        } else {
            false
        }
    }

    /// Get the current transaction (None if at initial state).
    pub fn current(&self) -> Option<TransactionId> {
        self.current
    }

    /// Get children of current node.
    pub fn children_of_current(&self) -> Vec<TransactionId> {
        self.current
            .and_then(|c| self.children.get(&c))
            .cloned()
            .unwrap_or_default()
    }

    /// Get children of a specific node.
    pub fn children_of(&self, id: TransactionId) -> Vec<TransactionId> {
        self.children.get(&id).cloned().unwrap_or_default()
    }

    /// Get the parent of a specific node.
    pub fn parent_of(&self, id: TransactionId) -> Option<TransactionId> {
        self.parents.get(&id).copied().flatten()
    }

    /// Get the most recently visited child (for default redo at branch point).
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
        self.children_of_current().len() > 1
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

    /// Get the total number of transactions in the tree.
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    /// Check if the tree is empty (no transactions recorded).
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }
}
```

### 1.4 Tests Module

**File:** `crates/undo_tree/src/tests.rs`

```rust
#![cfg(test)]

use super::*;
use clock::ReplicaId;

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
    assert!(tree.path_to_current().is_empty());
}

#[test]
fn test_linear_history() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));
    tree.push(tx(3));

    assert_eq!(tree.len(), 3);
    assert_eq!(tree.current(), Some(tx(3)));
    assert_eq!(tree.path_to_current(), vec![tx(1), tx(2), tx(3)]);

    tree.move_to_parent();
    assert_eq!(tree.current(), Some(tx(2)));

    tree.move_to_parent();
    assert_eq!(tree.current(), Some(tx(1)));

    tree.move_to_parent();
    assert_eq!(tree.current(), None);

    tree.move_to_child(tx(1));
    assert_eq!(tree.current(), None); // Can't move to child from None this way

    // Navigate properly
    tree.navigate_to(Some(tx(1)));
    assert_eq!(tree.current(), Some(tx(1)));

    tree.move_to_child(tx(2));
    assert_eq!(tree.current(), Some(tx(2)));
}

#[test]
fn test_branching() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));

    // Undo back to tx(1)
    tree.move_to_parent();
    assert_eq!(tree.current(), Some(tx(1)));

    // Create branch by adding tx(3)
    tree.push(tx(3));
    assert_eq!(tree.current(), Some(tx(3)));

    // tx(1) should now have two children
    let children = tree.children_of(tx(1));
    assert!(children.contains(&tx(2)));
    assert!(children.contains(&tx(3)));
    assert_eq!(children.len(), 2);

    // tx(1) is a branch point
    assert!(tree.is_branch_point(tx(1)));
    assert_eq!(tree.branch_points(), vec![tx(1)]);
}

#[test]
fn test_compute_path_same_branch() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));
    tree.push(tx(3));

    // From tx(3) to tx(1) - just undo
    let (to_undo, to_redo) = tree.compute_path(Some(tx(3)), Some(tx(1)));
    assert_eq!(to_undo, vec![tx(3), tx(2)]);
    assert!(to_redo.is_empty());

    // From tx(1) to tx(3) - just redo
    let (to_undo, to_redo) = tree.compute_path(Some(tx(1)), Some(tx(3)));
    assert!(to_undo.is_empty());
    assert_eq!(to_redo, vec![tx(2), tx(3)]);
}

#[test]
fn test_compute_path_across_branches() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));
    tree.move_to_parent(); // back to tx(1)
    tree.push(tx(3));
    tree.push(tx(4));

    // Now at tx(4), want to go to tx(2)
    // Common ancestor is tx(1)
    let (to_undo, to_redo) = tree.compute_path(Some(tx(4)), Some(tx(2)));
    assert_eq!(to_undo, vec![tx(4), tx(3)]); // Undo 4, then 3
    assert_eq!(to_redo, vec![tx(2)]); // Then redo 2
}

#[test]
fn test_compute_path_to_root() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));

    // From tx(2) to root (None)
    let (to_undo, to_redo) = tree.compute_path(Some(tx(2)), None);
    assert_eq!(to_undo, vec![tx(2), tx(1)]);
    assert!(to_redo.is_empty());

    // From root to tx(2)
    let (to_undo, to_redo) = tree.compute_path(None, Some(tx(2)));
    assert!(to_undo.is_empty());
    assert_eq!(to_redo, vec![tx(1), tx(2)]);
}

#[test]
fn test_last_visited_child() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));
    tree.move_to_parent();
    tree.push(tx(3));
    tree.move_to_parent();

    // At tx(1), last visited was tx(3) (we just came from there)
    assert_eq!(tree.last_visited_child_of_current(), Some(tx(3)));
    assert_eq!(tree.last_visited_child_of(tx(1)), Some(tx(3)));

    // Visit tx(2)
    tree.move_to_child(tx(2));
    tree.move_to_parent();

    // Now last visited should be tx(2)
    assert_eq!(tree.last_visited_child_of_current(), Some(tx(2)));
}

#[test]
fn test_navigate_to() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));
    tree.push(tx(3));

    // Navigate directly to tx(1)
    tree.navigate_to(Some(tx(1)));
    assert_eq!(tree.current(), Some(tx(1)));

    // last_visited_child should be updated along the path
    // When we navigate to tx(1) from tx(3), the path is [tx(1), tx(2), tx(3)]
    // But we're going TO tx(1), so we update last_visited for the path TO tx(1)
    // which doesn't set anything since tx(1) is at the end

    // Navigate to tx(3) - should set last_visited along the way
    tree.navigate_to(Some(tx(3)));
    assert_eq!(tree.current(), Some(tx(3)));
    assert_eq!(tree.last_visited_child_of(tx(1)), Some(tx(2)));
    assert_eq!(tree.last_visited_child_of(tx(2)), Some(tx(3)));
}

#[test]
fn test_parent_of() {
    let mut tree = UndoTree::new();
    tree.push(tx(1));
    tree.push(tx(2));
    tree.push(tx(3));

    assert_eq!(tree.parent_of(tx(1)), None);
    assert_eq!(tree.parent_of(tx(2)), Some(tx(1)));
    assert_eq!(tree.parent_of(tx(3)), Some(tx(2)));
}

#[test]
fn test_deep_branching() {
    let mut tree = UndoTree::new();

    // Create: 1 -> 2 -> 3
    tree.push(tx(1));
    tree.push(tx(2));
    tree.push(tx(3));

    // Go back to 2, create branch: 2 -> 4
    tree.move_to_parent();
    tree.push(tx(4));

    // Go back to 1, create branch: 1 -> 5 -> 6
    tree.move_to_parent();
    tree.move_to_parent();
    tree.push(tx(5));
    tree.push(tx(6));

    // Verify structure
    assert_eq!(tree.children_of(tx(1)), vec![tx(2), tx(5)]);
    assert_eq!(tree.children_of(tx(2)), vec![tx(3), tx(4)]);
    assert_eq!(tree.children_of(tx(5)), vec![tx(6)]);

    // Two branch points: tx(1) and tx(2)
    let branch_points = tree.branch_points();
    assert!(branch_points.contains(&tx(1)));
    assert!(branch_points.contains(&tx(2)));
    assert_eq!(branch_points.len(), 2);

    // Navigate from tx(6) to tx(3)
    let (to_undo, to_redo) = tree.compute_path(Some(tx(6)), Some(tx(3)));
    assert_eq!(to_undo, vec![tx(6), tx(5)]); // Undo back to tx(1)
    assert_eq!(to_redo, vec![tx(2), tx(3)]); // Redo to tx(3)
}
```

### 1.5 Update Root Cargo.toml

**File:** `Cargo.toml` (root workspace)

Add to `[workspace]` members (keep alphabetically sorted):
```toml
members = [
    # ... existing members ...
    "crates/undo_tree",
    "crates/undo_tree_panel",
    # ... rest of members ...
]
```

Add to `[workspace.dependencies]` (keep alphabetically sorted):
```toml
undo_tree = { path = "crates/undo_tree" }
undo_tree_panel = { path = "crates/undo_tree_panel" }
```

---

## Phase 2: Integrate with Text Crate

### 2.1 Add Dependency to Text Crate

**File:** `crates/text/Cargo.toml`

Add to `[dependencies]`:
```toml
undo_tree.workspace = true
```

### 2.2 Update text.rs

**File:** `crates/text/src/text.rs`

**Step 1: Add import at top**

```rust
use undo_tree::UndoTree;
```

**Step 2: Re-export TransactionId (if not already)**

The text crate already defines `pub type TransactionId = clock::Lamport;` at line 50. We can keep this or re-export from undo_tree. For consistency, keep the existing definition.

**Step 3: Add field to History struct (around line 146)**

```rust
struct History {
    base_text: Rope,
    operations: TreeMap<clock::Lamport, Operation>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    transaction_depth: usize,
    group_interval: Duration,
    // NEW: Track branch structure
    undo_tree: UndoTree,
}
```

**Step 4: Initialize in History::new() (around line 183)**

```rust
fn new(base_text: Rope) -> Self {
    Self {
        base_text,
        operations: Default::default(),
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
        transaction_depth: 0,
        group_interval: Duration::from_millis(300),
        undo_tree: UndoTree::new(),
    }
}
```

**Step 5: Modify end_transaction (lines 233-256)**

```rust
fn end_transaction(&mut self, now: Instant) -> Option<&HistoryEntry> {
    assert_ne!(self.transaction_depth, 0);
    self.transaction_depth -= 1;
    if self.transaction_depth == 0 {
        if self.undo_stack.last().unwrap().transaction.edit_ids.is_empty() {
            self.undo_stack.pop();
            None
        } else {
            // Still clear redo_stack for now - branches are tracked in undo_tree
            self.redo_stack.clear();

            let entry = self.undo_stack.last_mut().unwrap();
            entry.last_edit_at = now;

            // NEW: Record in undo tree
            self.undo_tree.push(entry.transaction.id);

            Some(entry)
        }
    } else {
        None
    }
}
```

**Step 6: Update pop_undo (lines 367-375)**

```rust
fn pop_undo(&mut self) -> Option<&HistoryEntry> {
    assert_eq!(self.transaction_depth, 0);
    if let Some(entry) = self.undo_stack.pop() {
        self.redo_stack.push(entry);
        self.undo_tree.move_to_parent();
        self.redo_stack.last()
    } else {
        None
    }
}
```

**Step 7: Update pop_redo (lines 457-465)**

```rust
fn pop_redo(&mut self) -> Option<&HistoryEntry> {
    assert_eq!(self.transaction_depth, 0);
    if let Some(entry) = self.redo_stack.pop() {
        let id = entry.transaction.id;
        self.undo_stack.push(entry);
        self.undo_tree.move_to_child(id);
        self.undo_stack.last()
    } else {
        None
    }
}
```

### 2.3 Add Public API to Buffer

**File:** `crates/text/src/text.rs`

Add these methods to the `impl Buffer` block (around line 1500):

```rust
/// Get the path from initial state to current position in the undo tree.
pub fn undo_tree_path(&self) -> Vec<TransactionId> {
    self.history.undo_tree.path_to_current()
}

/// Get all branch points in the undo tree.
pub fn undo_tree_branch_points(&self) -> Vec<TransactionId> {
    self.history.undo_tree.branch_points()
}

/// Get children of a specific transaction in the undo tree.
pub fn undo_tree_children(&self, id: TransactionId) -> Vec<TransactionId> {
    self.history.undo_tree.children_of(id)
}

/// Get the current position in the undo tree.
pub fn undo_tree_current(&self) -> Option<TransactionId> {
    self.history.undo_tree.current()
}

/// Check if currently at a branch point.
pub fn is_at_undo_branch_point(&self) -> bool {
    self.history.undo_tree.is_at_branch_point()
}

/// Navigate to a specific transaction in the undo tree.
/// Returns the operations performed to reach that state.
pub fn goto_undo_tree_transaction(&mut self, target: TransactionId) -> Vec<Operation> {
    let current = self.history.undo_tree.current();
    if current == Some(target) {
        return Vec::new();
    }

    let (to_undo, to_redo) = self.history.undo_tree.compute_path(current, Some(target));
    let mut ops = Vec::new();

    // Undo transactions to reach common ancestor
    for tx_id in to_undo {
        if let Some(entry) = self.history.transaction(tx_id) {
            let transaction = entry.clone();
            let op = self.undo_or_redo(transaction);
            ops.push(op);
        }
    }

    // Redo transactions to reach target
    for tx_id in to_redo {
        if let Some(entry) = self.history.transaction(tx_id) {
            let transaction = entry.clone();
            let op = self.undo_or_redo(transaction);
            ops.push(op);
        }
    }

    // Update tree position
    self.history.undo_tree.navigate_to(Some(target));

    ops
}

/// Get the default redo target at current branch point (most recently visited).
pub fn default_redo_target(&self) -> Option<TransactionId> {
    self.history.undo_tree.last_visited_child_of_current()
}
```

---

## Phase 3: Language Buffer Integration

**File:** `crates/language/src/buffer.rs`

Add methods to expose undo tree functionality (after redo_to_transaction around line 3100):

```rust
/// Get the undo tree path for this buffer.
pub fn undo_tree_path(&self) -> Vec<text::TransactionId> {
    self.text.undo_tree_path()
}

/// Get branch points in the undo tree.
pub fn undo_tree_branch_points(&self) -> Vec<text::TransactionId> {
    self.text.undo_tree_branch_points()
}

/// Get children of a transaction.
pub fn undo_tree_children(&self, id: text::TransactionId) -> Vec<text::TransactionId> {
    self.text.undo_tree_children(id)
}

/// Get current undo tree position.
pub fn undo_tree_current(&self) -> Option<text::TransactionId> {
    self.text.undo_tree_current()
}

/// Check if at a branch point.
pub fn is_at_undo_branch_point(&self) -> bool {
    self.text.is_at_undo_branch_point()
}

/// Navigate to a specific undo tree transaction.
pub fn goto_undo_tree_transaction(
    &mut self,
    target: text::TransactionId,
    cx: &mut Context<Self>,
) -> bool {
    let was_dirty = self.is_dirty();
    let old_version = self.version.clone();

    let ops = self.text.goto_undo_tree_transaction(target);
    if ops.is_empty() {
        return false;
    }

    for op in ops {
        self.send_operation(Operation::Buffer(op), true, cx);
    }
    self.did_edit(&old_version, was_dirty, cx);
    true
}
```

---

## Phase 4: Panel UI

### 4.1 Create Panel Crate

**File:** `crates/undo_tree_panel/Cargo.toml`

```toml
[package]
name = "undo_tree_panel"
version = "0.1.0"
edition.workspace = true
publish.workspace = true
license = "GPL-3.0-or-later"

[lints]
workspace = true

[lib]
path = "src/undo_tree_panel.rs"
doctest = false

[dependencies]
anyhow.workspace = true
collections.workspace = true
db.workspace = true
editor.workspace = true
gpui.workspace = true
language.workspace = true
project.workspace = true
schemars.workspace = true
serde.workspace = true
serde_json.workspace = true
settings.workspace = true
text.workspace = true
ui.workspace = true
undo_tree.workspace = true
workspace.workspace = true
```

### 4.2 Panel Implementation

**File:** `crates/undo_tree_panel/src/undo_tree_panel.rs`

```rust
use anyhow::Result;
use db::kvp::KEY_VALUE_STORE;
use editor::Editor;
use gpui::*;
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsSources};
use ui::prelude::*;
use undo_tree::TransactionId;
use workspace::{
    dock::DockPosition,
    panel::{Panel, PanelEvent},
    Workspace,
};

const UNDO_TREE_PANEL_KEY: &str = "UndoTreePanel";

actions!(undo_tree_panel, [ToggleFocus]);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<UndoTreePanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UndoTreePanelSettings {
    #[serde(default = "default_dock")]
    pub dock: DockSide,
    #[serde(default = "default_button")]
    pub button: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DockSide {
    Left,
    #[default]
    Right,
}

fn default_dock() -> DockSide {
    DockSide::Right
}

fn default_button() -> bool {
    true
}

impl Default for UndoTreePanelSettings {
    fn default() -> Self {
        Self {
            dock: default_dock(),
            button: default_button(),
        }
    }
}

impl Settings for UndoTreePanelSettings {
    const KEY: Option<&'static str> = Some("undo_tree_panel");

    type FileContent = Self;

    fn load(sources: SettingsSources<Self::FileContent>, _: &mut App) -> Result<Self> {
        sources.json_merge()
    }
}

#[derive(Serialize, Deserialize)]
struct SerializedUndoTreePanel {
    width: Option<Pixels>,
}

pub struct UndoTreePanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    pending_serialization: Task<Option<()>>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct TreeNodeInfo {
    id: TransactionId,
    depth: usize,
    is_current: bool,
    is_branch_point: bool,
    child_count: usize,
}

impl UndoTreePanel {
    pub fn new(workspace: &Workspace, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = workspace.project().clone();
        let workspace_handle = workspace.weak_handle();

        let subscriptions = vec![cx.observe(&project, |_, _, cx| cx.notify())];

        Self {
            workspace: workspace_handle,
            focus_handle: cx.focus_handle(),
            width: None,
            pending_serialization: Task::ready(None),
            _subscriptions: subscriptions,
        }
    }

    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        let serialized_panel = cx
            .background_spawn(async move {
                KEY_VALUE_STORE
                    .read_kvp(UNDO_TREE_PANEL_KEY)
                    .ok()
                    .flatten()
                    .and_then(|value| serde_json::from_str::<SerializedUndoTreePanel>(&value).ok())
            })
            .await;

        workspace.update_in(&mut cx, |workspace, window, cx| {
            let panel = cx.new(|cx| UndoTreePanel::new(workspace, window, cx));
            if let Some(serialized_panel) = serialized_panel {
                panel.update(cx, |panel, cx| {
                    panel.width = serialized_panel.width;
                    cx.notify();
                });
            }
            panel
        })
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        let width = self.width;
        self.pending_serialization = cx.background_spawn(async move {
            KEY_VALUE_STORE
                .write_kvp(
                    UNDO_TREE_PANEL_KEY.to_string(),
                    serde_json::to_string(&SerializedUndoTreePanel { width })?,
                )
                .await?;
            anyhow::Ok(())
        });
    }

    fn get_tree_info(&self, cx: &App) -> Vec<TreeNodeInfo> {
        let mut info = Vec::new();

        let Some(workspace) = self.workspace.upgrade() else {
            return info;
        };

        let Some(active_item) = workspace.read(cx).active_item(cx) else {
            return info;
        };

        let Some(editor) = active_item.act_as::<Editor>(cx) else {
            return info;
        };

        let Some(buffer) = editor.read(cx).buffer().read(cx).as_singleton() else {
            return info;
        };

        let buffer = buffer.read(cx);
        let path = buffer.undo_tree_path();
        let branch_points = buffer.undo_tree_branch_points();
        let current = buffer.undo_tree_current();

        for (depth, &id) in path.iter().enumerate() {
            let is_branch_point = branch_points.contains(&id);
            let child_count = buffer.undo_tree_children(id).len();
            info.push(TreeNodeInfo {
                id,
                depth,
                is_current: current == Some(id),
                is_branch_point,
                child_count,
            });
        }

        info
    }

    fn navigate_to(&mut self, id: TransactionId, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        workspace.update(cx, |workspace, cx| {
            let Some(active_item) = workspace.active_item(cx) else {
                return;
            };

            let Some(editor) = active_item.act_as::<Editor>(cx) else {
                return;
            };

            editor.update(cx, |editor, cx| {
                editor.buffer().update(cx, |buffer, cx| {
                    if let Some(singleton) = buffer.as_singleton() {
                        singleton.update(cx, |buffer, cx| {
                            buffer.goto_undo_tree_transaction(id, cx);
                        });
                    }
                });
            });
        });

        cx.notify();
    }
}

impl Render for UndoTreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree_info = self.get_tree_info(cx);
        let is_empty = tree_info.is_empty();

        v_flex()
            .id("undo-tree-panel")
            .key_context("UndoTreePanel")
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(
                h_flex()
                    .p_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Undo Tree"),
                    ),
            )
            .when(is_empty, |this| {
                this.child(
                    div()
                        .p_4()
                        .text_color(cx.theme().colors().text_muted)
                        .child("No undo history"),
                )
            })
            .when(!is_empty, |this| {
                this.child(
                    div()
                        .p_2()
                        .flex_1()
                        .overflow_y_scroll()
                        .children(tree_info.into_iter().enumerate().map(|(idx, info)| {
                            let id = info.id;

                            div()
                                .id(ElementId::NamedInteger("undo-node".into(), idx))
                                .px_2()
                                .py_1()
                                .ml(px(info.depth as f32 * 8.0))
                                .rounded_md()
                                .cursor_pointer()
                                .when(info.is_current, |this| {
                                    this.font_weight(FontWeight::BOLD)
                                        .bg(cx.theme().colors().element_active)
                                })
                                .hover(|this| this.bg(cx.theme().colors().element_hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.navigate_to(id, window, cx);
                                }))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(if info.is_branch_point {
                                            "◆"
                                        } else if info.is_current {
                                            "●"
                                        } else {
                                            "○"
                                        })
                                        .child(format!("#{}", info.id.value))
                                        .when(info.child_count > 1, |this| {
                                            this.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().colors().text_muted)
                                                    .child(format!(
                                                        "({} branches)",
                                                        info.child_count
                                                    )),
                                            )
                                        }),
                                )
                        })),
                )
            })
    }
}

impl Panel for UndoTreePanel {
    fn persistent_name() -> &'static str {
        "Undo Tree"
    }

    fn panel_key() -> &'static str {
        UNDO_TREE_PANEL_KEY
    }

    fn position(&self, _window: &Window, cx: &App) -> DockPosition {
        match UndoTreePanelSettings::get_global(cx).dock {
            DockSide::Left => DockPosition::Left,
            DockSide::Right => DockPosition::Right,
        }
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
    }

    fn size(&self, _window: &Window, _cx: &App) -> Pixels {
        self.width.unwrap_or(px(240.0))
    }

    fn set_size(&mut self, size: Option<Pixels>, _window: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        self.serialize(cx);
        cx.notify();
    }

    fn icon(&self, _window: &Window, cx: &App) -> Option<ui::IconName> {
        UndoTreePanelSettings::get_global(cx)
            .button
            .then_some(ui::IconName::ListTree)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Undo Tree")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        3
    }
}

impl EventEmitter<PanelEvent> for UndoTreePanel {}

impl Focusable for UndoTreePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
```

### 4.3 Register Panel in Zed

**File:** `crates/zed/src/zed.rs`

Add import:
```rust
use undo_tree_panel::UndoTreePanel;
```

In `initialize_panels` function, add:
```rust
let undo_tree_panel = UndoTreePanel::load(workspace_handle.clone(), cx.clone());
```

And in the `futures::join!` call:
```rust
add_panel_when_ready(undo_tree_panel, workspace_handle.clone(), cx.clone()),
```

**File:** `crates/zed/Cargo.toml`

Add dependency:
```toml
undo_tree_panel.workspace = true
```

---

## Phase 5: Integration Tests

**File:** `crates/text/src/tests.rs`

Add these tests:

```rust
#[test]
fn test_branch_preserved_on_edit_after_undo() {
    let mut buffer = Buffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), "");
    buffer.set_group_interval(Duration::from_secs(0));

    buffer.edit([(0..0, "hello")]);
    assert_eq!(buffer.text(), "hello");

    buffer.edit([(5..5, " world")]);
    assert_eq!(buffer.text(), "hello world");

    buffer.undo();
    assert_eq!(buffer.text(), "hello");

    buffer.edit([(5..5, " rust")]);
    assert_eq!(buffer.text(), "hello rust");

    let path = buffer.undo_tree_path();
    assert_eq!(path.len(), 2);

    let branch_points = buffer.undo_tree_branch_points();
    assert_eq!(branch_points.len(), 1);

    let a_id = path[0];
    let children = buffer.undo_tree_children(a_id);
    assert_eq!(children.len(), 2);
}

#[test]
fn test_navigate_to_transaction() {
    let mut buffer = Buffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), "");
    buffer.set_group_interval(Duration::from_secs(0));

    buffer.edit([(0..0, "A")]);
    let a_id = buffer.undo_tree_current().unwrap();

    buffer.edit([(1..1, "B")]);
    let b_id = buffer.undo_tree_current().unwrap();

    buffer.undo();
    buffer.edit([(1..1, "C")]);
    let c_id = buffer.undo_tree_current().unwrap();

    assert_eq!(buffer.text(), "AC");

    buffer.goto_undo_tree_transaction(b_id);
    assert_eq!(buffer.text(), "AB");

    buffer.goto_undo_tree_transaction(c_id);
    assert_eq!(buffer.text(), "AC");
}

#[test]
fn test_undo_tree_current_position() {
    let mut buffer = Buffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), "");
    buffer.set_group_interval(Duration::from_secs(0));

    assert!(buffer.undo_tree_current().is_none());

    buffer.edit([(0..0, "hello")]);
    let first_id = buffer.undo_tree_current();
    assert!(first_id.is_some());

    buffer.undo();
    assert!(buffer.undo_tree_current().is_none());

    buffer.redo();
    assert_eq!(buffer.undo_tree_current(), first_id);
}
```

---

## Files Summary

| File | Action | Description |
|------|--------|-------------|
| `crates/undo_tree/Cargo.toml` | Create | New undo_tree crate manifest |
| `crates/undo_tree/src/undo_tree.rs` | Create | UndoTree data structure |
| `crates/undo_tree/src/tests.rs` | Create | Unit tests for UndoTree |
| `crates/text/Cargo.toml` | Modify | Add undo_tree dependency |
| `crates/text/src/text.rs` | Modify | Integrate UndoTree with History |
| `crates/text/src/tests.rs` | Modify | Add integration tests |
| `crates/language/src/buffer.rs` | Modify | Expose undo tree methods |
| `crates/undo_tree_panel/Cargo.toml` | Create | New panel crate manifest |
| `crates/undo_tree_panel/src/undo_tree_panel.rs` | Create | Panel implementation |
| `crates/zed/src/zed.rs` | Modify | Register panel |
| `crates/zed/Cargo.toml` | Modify | Add undo_tree_panel dependency |
| `Cargo.toml` | Modify | Add workspace members and dependencies |

---

## Implementation Order

1. **Phase 1**: Create `undo_tree` crate with UndoTree data structure and tests
2. **Phase 2**: Integrate with text crate (add dependency, modify History)
3. **Phase 5**: Add integration tests to text crate (verify core works)
4. **Phase 3**: Language buffer integration
5. **Phase 4**: Panel UI crate and registration
6. **Polish**: Test edge cases, fix bugs

---

## Future Extensions (Post-MVP)

1. **Persistence** — Save tree structure and operations to disk
2. **Graph visualization** — Replace list with visual tree graph
3. **Time navigation** — Add timestamps, "go back 5 minutes"
4. **AI tagging** — Mark AI-generated branches differently
5. **Diff preview** — Show diff on hover
6. **Bookmarks** — Mark important states
7. **Multi-buffer** — Coordinate trees across files for refactors
