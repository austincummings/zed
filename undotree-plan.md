# Undo Tree MVP - Implementation Plan

## Goal

Preserve edit branches when editing after undo, and provide basic UI to navigate between branches. This is a minimal viable implementation that can be extended later.

**What's in scope:**
- Preserve redo branches instead of clearing them
- Track branch structure as metadata
- Simple panel UI showing branches
- Click to switch branches
- Basic keyboard navigation
- **MultiBuffer support** (coordinate undo trees across multiple buffers)

**What's deferred:**
- Persistence (history still lost on restart)
- Time-based navigation
- Graph visualization (start with simple list)
- Diff previews
- Bookmarks

---

## Current Status

| Phase | Status | Description |
|-------|--------|-------------|
| Phase 1 | ✅ DONE | `undo_tree` crate created with full implementation |
| Phase 2 | ✅ DONE | Text crate integration complete |
| Phase 3 | ✅ DONE | Language buffer integration complete |
| Phase 4 | ⬜ TODO | MultiBuffer integration (CRITICAL GAP) |
| Phase 5 | ⬜ TODO | Panel UI crate |
| Phase 6 | ⬜ TODO | Integration tests |

---

## Architecture Overview

### Key Insight

The `operations` TreeMap stores ALL operations permanently. The `UndoMap` tracks undo counts per edit - edits aren't deleted, they're just toggled visible/invisible:

```rust
// undo_map.rs - Visibility is determined by undo count parity
pub fn is_undone(&self, edit_id: clock::Lamport) -> bool {
    self.undo_count(edit_id) % 2 == 1  // Odd count = undone/invisible
}
```

This means we can navigate to any historical state by toggling the right edit visibility.

### Completed Components

**`crates/undo_tree/src/undo_tree.rs`** - Core data structure:
- `UndoTree` with nodes tracking parent/children/timestamps
- `TransactionSource` enum (User vs Agent) for AI edit tagging
- Navigation: `push()`, `move_to_parent()`, `move_to_child()`, `navigate_to()`
- Path computation: `compute_path()`, `path_to_current()`, `path_to_node()`
- Branch detection: `is_branch_point()`, `branch_points()`
- Debug formatting with tree visualization

**`crates/text/src/text.rs`** - History integration:
- `History.undo_tree: UndoTree` field
- `push()` records transactions to tree
- `pop_undo()` calls `undo_tree.move_to_parent()`
- `pop_redo()` calls `undo_tree.move_to_child()`
- Buffer exposes: `undo_tree_path()`, `undo_tree_branch_points()`, `undo_tree_children()`, `undo_tree_current()`, `is_at_undo_branch_point()`, `default_redo_target()`, `goto_undo_tree_transaction()`

**`crates/language/src/buffer.rs`** - Language-aware wrapper:
- `goto_undo_tree_transaction()` at line 3222

---

## Phase 4: MultiBuffer Integration (CRITICAL)

### Problem

The MultiBuffer's `History` in `crates/multi_buffer/src/transaction.rs` does NOT have undo tree tracking:

```rust
// Current state - NO undo_tree field!
pub(super) struct History {
    next_transaction_id: TransactionId,
    undo_stack: Vec<Transaction>,
    redo_stack: Vec<Transaction>,  // Still cleared on new edit!
    transaction_depth: usize,
    group_interval: Duration,
}
```

Issues:
1. `mark_transaction_source()` at line 123-128 is a **no-op** (empty body)
2. No `undo_tree` field - branches are lost on `redo_stack.clear()` at line 76
3. For singletons, it delegates to underlying buffer (works)
4. For non-singletons (multi-file edits), no undo tree tracking exists

### Implementation Steps

#### 4.1 Add UndoTree to MultiBuffer History

**File:** `crates/multi_buffer/Cargo.toml`

Add dependency:
```toml
undo_tree.workspace = true
```

**File:** `crates/multi_buffer/src/transaction.rs`

Add import:
```rust
use undo_tree::UndoTree;
```

Add field to History:
```rust
pub(super) struct History {
    next_transaction_id: TransactionId,
    undo_stack: Vec<Transaction>,
    redo_stack: Vec<Transaction>,
    transaction_depth: usize,
    group_interval: Duration,
    undo_tree: UndoTree,  // NEW
}
```

Update Default impl:
```rust
impl Default for History {
    fn default() -> Self {
        History {
            next_transaction_id: clock::Lamport::MIN,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            transaction_depth: 0,
            group_interval: Duration::from_millis(300),
            undo_tree: UndoTree::new(),  // NEW
        }
    }
}
```

#### 4.2 Update Transaction Lifecycle

**end_transaction()** - Record to undo tree after committing:
```rust
fn end_transaction(
    &mut self,
    now: Instant,
    buffer_transactions: HashMap<BufferId, text::TransactionId>,
) -> bool {
    // ... existing code ...
    if self.transaction_depth == 0 {
        if buffer_transactions.is_empty() {
            self.undo_stack.pop();
            false
        } else {
            self.redo_stack.clear();
            let transaction = self.undo_stack.last_mut().unwrap();
            transaction.last_edit_at = now;
            // ... existing buffer_transactions handling ...

            // NEW: Record in undo tree
            self.undo_tree.push(transaction.id);

            true
        }
    } else {
        false
    }
}
```

**group_trailing()** - Update undo tree after grouping:
```rust
fn group_trailing(&mut self, n: usize) -> Option<TransactionId> {
    // ... existing code ...
    self.undo_stack.truncate(new_len);

    // NEW: Update undo tree position if we grouped transactions
    if let Some(id) = self.undo_stack.last().map(|t| t.id) {
        if !self.undo_tree.contains(id) {
            self.undo_tree.push(id);
        }
    }

    self.undo_stack.last().map(|t| t.id)
}
```

**pop_undo()** - Update undo tree:
```rust
fn pop_undo(&mut self) -> Option<&mut Transaction> {
    assert_eq!(self.transaction_depth, 0);
    if let Some(transaction) = self.undo_stack.pop() {
        self.redo_stack.push(transaction);
        self.undo_tree.move_to_parent();  // NEW
        self.redo_stack.last_mut()
    } else {
        None
    }
}
```

**pop_redo()** - Update undo tree:
```rust
fn pop_redo(&mut self) -> Option<&mut Transaction> {
    assert_eq!(self.transaction_depth, 0);
    if let Some(transaction) = self.redo_stack.pop() {
        self.undo_tree.move_to_child(transaction.id);  // NEW
        self.undo_stack.push(transaction);
        self.undo_stack.last_mut()
    } else {
        None
    }
}
```

#### 4.3 Implement mark_transaction_source

**File:** `crates/multi_buffer/src/transaction.rs`

Replace the empty stub:
```rust
fn mark_transaction_source(
    &mut self,
    transaction_id: TransactionId,
    source: TransactionSource,
) {
    self.undo_tree.mark_transaction_source(transaction_id, source);
}
```

#### 4.4 Add Public API to MultiBuffer

**File:** `crates/multi_buffer/src/multi_buffer.rs`

Add these methods:

```rust
/// Get the path from initial state to current position in the undo tree.
pub fn undo_tree_path(&self, cx: &App) -> Vec<TransactionId> {
    if let Some(buffer) = self.as_singleton() {
        buffer.read(cx).undo_tree_path()
    } else {
        self.history.undo_tree.path_to_current()
    }
}

/// Get all branch points in the undo tree.
pub fn undo_tree_branch_points(&self, cx: &App) -> Vec<TransactionId> {
    if let Some(buffer) = self.as_singleton() {
        buffer.read(cx).undo_tree_branch_points()
    } else {
        self.history.undo_tree.branch_points()
    }
}

/// Get children of a specific transaction in the undo tree.
pub fn undo_tree_children(&self, id: TransactionId, cx: &App) -> Vec<TransactionId> {
    if let Some(buffer) = self.as_singleton() {
        buffer.read(cx).undo_tree_children(id)
    } else {
        self.history.undo_tree.children_of(id)
    }
}

/// Get the current position in the undo tree.
pub fn undo_tree_current(&self, cx: &App) -> Option<TransactionId> {
    if let Some(buffer) = self.as_singleton() {
        buffer.read(cx).undo_tree_current()
    } else {
        self.history.undo_tree.current()
    }
}

/// Check if currently at a branch point.
pub fn is_at_undo_branch_point(&self, cx: &App) -> bool {
    if let Some(buffer) = self.as_singleton() {
        buffer.read(cx).is_at_undo_branch_point()
    } else {
        self.history.undo_tree.is_at_branch_point()
    }
}

/// Navigate to a specific transaction in the undo tree.
/// This can navigate across branches, not just along the linear undo/redo stack.
pub fn goto_undo_tree_transaction(
    &mut self,
    target: TransactionId,
    cx: &mut Context<Self>,
) -> bool {
    if let Some(buffer) = self.as_singleton() {
        return buffer.update(cx, |buffer, cx| {
            buffer.goto_undo_tree_transaction(target, cx)
        });
    }

    let current = self.history.undo_tree.current();
    if current == Some(target) {
        return false;
    }

    let (to_undo, to_redo) = self.history.undo_tree.compute_path(current, Some(target));

    // Undo transactions to reach common ancestor
    for tx_id in to_undo {
        if let Some(transaction) = self.history.transaction(tx_id) {
            let buffer_transactions = transaction.buffer_transactions.clone();
            for (buffer_id, buffer_transaction_id) in buffer_transactions {
                if let Some(BufferState { buffer, .. }) = self.buffers.get(&buffer_id) {
                    buffer.update(cx, |buffer, cx| {
                        buffer.undo_to_transaction(buffer_transaction_id, cx);
                    });
                }
            }
        }
    }

    // Redo transactions to reach target
    for tx_id in to_redo {
        if let Some(transaction) = self.history.transaction(tx_id) {
            let buffer_transactions = transaction.buffer_transactions.clone();
            for (buffer_id, buffer_transaction_id) in buffer_transactions {
                if let Some(BufferState { buffer, .. }) = self.buffers.get(&buffer_id) {
                    buffer.update(cx, |buffer, cx| {
                        buffer.redo_to_transaction(buffer_transaction_id, cx);
                    });
                }
            }
        }
    }

    // Update tree position
    self.history.undo_tree.navigate_to(Some(target));

    true
}
```

#### 4.5 Add contains() to UndoTree

**File:** `crates/undo_tree/src/undo_tree.rs`

The `contains()` method already exists at line 83-85.

---

## Phase 5: Panel UI

### 5.1 Create Panel Crate

**Directory Structure:**
```
crates/undo_tree_panel/
├── Cargo.toml
└── src/
    └── undo_tree_panel.rs
```

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
multi_buffer.workspace = true
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

### 5.2 Update Root Cargo.toml

Add to `[workspace]` members and `[workspace.dependencies]`:
```toml
undo_tree_panel = { path = "crates/undo_tree_panel" }
```

### 5.3 Panel Implementation

See the detailed implementation in the original plan (Phase 4 section). Key changes for MultiBuffer support:

- Use `editor.buffer().read(cx).undo_tree_path(cx)` instead of going through singleton
- The panel should work with both singleton and multi-buffer editors

### 5.4 Register Panel in Zed

**File:** `crates/zed/src/zed.rs`

Add import and registration as described in original plan.

---

## Phase 6: Integration Tests

**File:** `crates/text/src/tests.rs`

Add tests for undo tree functionality (see original plan).

**File:** `crates/multi_buffer/src/multi_buffer.rs` (tests module)

Add MultiBuffer-specific tests:

```rust
#[gpui::test]
fn test_multibuffer_undo_tree_branch(cx: &mut Context<MultiBuffer>) {
    // Create multi-buffer with multiple excerpts
    // Make edits, undo, branch, verify tree structure
}

#[gpui::test]
fn test_multibuffer_navigate_across_branches(cx: &mut Context<MultiBuffer>) {
    // Create branches, navigate between them
    // Verify all underlying buffers are synchronized
}
```

---

## Implementation Order

1. **Phase 4.1-4.4**: MultiBuffer undo tree integration
2. **Phase 4.5**: Verify contains() exists (done)
3. **Phase 5**: Panel UI crate
4. **Phase 6**: Integration tests
5. **Polish**: Test edge cases, fix bugs

---

## Files Summary

| File | Action | Status | Description |
|------|--------|--------|-------------|
| `crates/undo_tree/Cargo.toml` | Created | ✅ | Undo tree crate manifest |
| `crates/undo_tree/src/undo_tree.rs` | Created | ✅ | UndoTree data structure |
| `crates/text/Cargo.toml` | Modified | ✅ | Added undo_tree dependency |
| `crates/text/src/text.rs` | Modified | ✅ | Integrated UndoTree with History |
| `crates/language/src/buffer.rs` | Modified | ✅ | Exposed undo tree methods |
| `crates/multi_buffer/Cargo.toml` | Modify | ⬜ | Add undo_tree dependency |
| `crates/multi_buffer/src/transaction.rs` | Modify | ⬜ | Add undo tree tracking |
| `crates/multi_buffer/src/multi_buffer.rs` | Modify | ⬜ | Add public undo tree API |
| `crates/undo_tree_panel/Cargo.toml` | Create | ⬜ | New panel crate manifest |
| `crates/undo_tree_panel/src/undo_tree_panel.rs` | Create | ⬜ | Panel implementation |
| `crates/zed/src/zed.rs` | Modify | ⬜ | Register panel |
| `crates/zed/Cargo.toml` | Modify | ⬜ | Add undo_tree_panel dependency |
| `Cargo.toml` | Modify | ⬜ | Add undo_tree_panel to workspace |

---

## Future Extensions (Post-MVP)

1. **Persistence** — Save tree structure and operations to disk
2. **Graph visualization** — Replace list with visual tree graph
3. **Time navigation** — Add timestamps, "go back 5 minutes"
4. **AI edit highlighting** — Visual distinction for AI-generated branches (TransactionSource already supports this)
5. **Diff preview** — Show diff on hover
6. **Bookmarks** — Mark important states
