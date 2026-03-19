## User

I am continuing a conversation with another agent, here is the context:

## User

In my testing I copy and pasted a function somewhere else in the code, but my bookmark stayed on the line it was on. What techniques could be used to move that bookmark around with edits like copy and paste?

## Assistant



This is a great question. The core issue is that bookmarks are anchored to a position in the buffer, and when you copy-paste a function, the buffer edit system sees it as an insertion at the destination — the original anchor stays put because the text at the original location wasn't moved, it was duplicated.

Let me explore the relevant systems to understand what's possible.

**Tool Call: Explore buffer edit tracking**
Status: Completed

task_id: ses_2f7fcbf47ffezBSFyOyYaDzE78 (for resuming to continue this task if needed)

<task_result>
I now have a comprehensive picture. Let me compile all findings.

---

Here are the detailed findings for each question:

## 1. Clipboard / Copy-Paste System

### Data Structures

**`ClipboardSelection`** - `/home/austin/src/zed/crates/editor/src/editor.rs` lines 1735-1746
```rust
pub struct ClipboardSelection {
    pub len: usize,                         // byte length of this selection
    pub is_entire_line: bool,               // was it a full-line copy (empty cursor)
    pub first_line_indent: u32,             // indent of first line (for auto-reindent on paste)
    pub file_path: Option<PathBuf>,         // absolute path of source file
    pub line_range: Option<RangeInclusive<u32>>,  // source line range in the buffer
}
```

**`ClipboardItem` / `ClipboardEntry`** - `/home/austin/src/zed/crates/gpui/src/platform.rs` lines 1646-1660
```rust
pub struct ClipboardItem {
    pub entries: Vec<ClipboardEntry>,
}
pub enum ClipboardEntry {
    String(ClipboardString),
    Image(Image),
    ExternalPaths(ExternalPaths),
}
```

The `ClipboardString` carries text plus optional serialized JSON metadata.

### Copy Flow

**`Editor::copy()`** - `/home/austin/src/zed/crates/editor/src/editor.rs` line 14091
Delegates to **`do_copy()`** at line 14095. This method:
1. Iterates all selections (line 14103).
2. For empty selections, expands to the entire line (lines 14108-14118).
3. Gathers text and builds a `ClipboardSelection` for each selection using `ClipboardSelection::for_buffer()` (lines 14179-14186), which records the `len`, `is_entire_line`, `first_line_indent`, `file_path`, and `line_range`.
4. Writes to the system clipboard as a `ClipboardItem` with the `Vec<ClipboardSelection>` as JSON metadata (lines 14189-14192).

**Key takeaway**: The clipboard stores text + metadata about each selection (byte length, line mode, indent, source file path, source line range). There is **no** content identity, unique ID, or hash stored. The metadata is purely for formatting-aware paste.

### Cut Flow

**`Editor::cut()`** - `/home/austin/src/zed/crates/editor/src/editor.rs` line 14046
Calls **`cut_common()`** at line 13988, which:
1. Gathers text and `ClipboardSelection` metadata for each selection (same as copy).
2. Wraps the deletion in `self.transact()` (line 14037) which opens a transaction, selects the cut ranges, and calls `this.insert("", window, cx)` to delete the text (line 14041).
3. Returns a `ClipboardItem` that is written to the system clipboard.

The cut and paste are **completely independent operations** with no shared state linking them. The `cut()` produces a transaction for the deletion, and `paste()` later produces a separate transaction for the insertion.

### Paste Flow

**`Editor::paste()`** - `/home/austin/src/zed/crates/editor/src/editor.rs` line 14417
Reads from the system clipboard and calls **`do_paste()`** at line 14195, which:
1. Wraps everything in `self.transact()` (line 14209).
2. If `ClipboardSelection` metadata exists and the selection count matches, distributes pasted text per-selection, respecting `is_entire_line` for line-above insertion (lines 14218-14299).
3. Calls `buffer.edit(edits, autoindent_mode, cx)` (line 14288) to perform the insertion.
4. If no metadata, pastes the full text at each cursor (lines 14305-14367).

## 2. Text Anchors and Edit Behavior

### Anchor Structure

**`Anchor`** - `/home/austin/src/zed/crates/text/src/anchor.rs` lines 11-28
```rust
pub struct Anchor {
    pub(crate) timestamp_replica_id: clock::ReplicaId,
    pub(crate) timestamp_value: clock::Seq,
    pub offset: u32,
    pub bias: Bias,
    pub buffer_id: Option<BufferId>,
}
```

An anchor is a timestamped position within a CRDT fragment. It stores:
- **timestamp**: The Lamport timestamp of the insert operation that created the text the anchor points into.
- **offset**: Byte offset within that specific insertion's text.
- **bias**: `Left` or `Right` — determines which side the anchor sticks to when text is inserted at that exact position.

### How Anchors Track Through Edits

Anchors are **not updated** when the buffer is edited. Instead, they are resolved lazily via `offset_for_anchor()` at `/home/austin/src/zed/crates/text/src/text.rs` lines 2450-2483:

1. The anchor's `timestamp` identifies the original insertion operation.
2. The system looks up the `InsertionFragment` (line 2463) via `try_find_fragment()` (lines 2525-2549), which searches the insertions B-tree using the anchor's timestamp and offset.
3. The `InsertionFragment` maps to a `fragment_id` (a `Locator`) in the fragments tree.
4. The fragment tree is searched for that `Locator` to find the fragment's current position (lines 2470-2476).
5. If the fragment is **visible** (not deleted), the anchor resolves to the fragment's start offset plus the local offset within the fragment (lines 2479-2481).
6. If the fragment is **invisible** (deleted), the offset is just the fragment's start — meaning **the anchor collapses to the position where the deleted text was**.

### Critical behavior for BookmarkAnchor during deletion:

When text is deleted (`cut`), the CRDT marks fragments as invisible by adding the edit's timestamp to `fragment.deletions` (line 997-998 in `apply_local_edit`). The `Fragment.visible` flag is set to `false`.

When the anchor is later resolved (line 2479): `if fragment.visible { fragment_offset += ... }` — if the fragment is invisible, the anchor resolves to the **start of the invisible fragment's position**, essentially collapsing to where neighboring visible text exists.

This means: **When bookmarked text is cut, the `BookmarkAnchor` collapses to the deletion point. It does NOT follow the text to its new paste location.** The paste creates entirely new fragments with new timestamps.

### Anchor creation

**`anchor_at_offset()`** - `/home/austin/src/zed/crates/text/src/text.rs` lines 2575-2609
Creates an anchor by finding the fragment at the given offset and recording that fragment's timestamp and insertion offset. This means **paste creates new anchors that point to new fragment timestamps, completely unrelated to the original cut anchors**.

## 3. Editor "Move" Operations (Cut + Paste)

There is **no existing mechanism** to detect that an edit is a "move" rather than a plain insertion or deletion. The analysis shows:

- **`cut()`** (line 14046) calls `cut_common()` which performs the deletion inside a `self.transact()` call, producing a transaction.
- **`paste()`** (line 14417) reads from the clipboard and calls `do_paste()`, which performs the insertion inside a separate `self.transact()` call, producing a different transaction.

These two transactions are:
1. **Temporally separate** — they happen at different `Instant` times.
2. **Structurally independent** — different `TransactionId`s, different `edit_ids`.
3. **Not linked** — there is no field on `Transaction`, `EditOperation`, or `ClipboardSelection` that connects a cut to a subsequent paste.

The `Transaction` struct (`/home/austin/src/zed/crates/text/src/text.rs` line 135) contains only:
```rust
pub struct Transaction {
    pub id: TransactionId,
    pub edit_ids: Vec<clock::Lamport>,
    pub start: clock::Global,
}
```

There is no "source transaction", "paired transaction", or "operation type" field.

The transaction grouping mechanism (lines 287-316) groups transactions on the undo stack based on **time proximity** (`group_interval` = 300ms by default). Cut and paste would typically be grouped together if done quickly, but this grouping is for undo purposes only — it merges `edit_ids` into a single transaction on the undo stack. There is no semantic annotation that the group represents a "move".

## 4. Existing Content-Movement Tracking Systems

### Diagnostics
**`DiagnosticSet`** - `/home/austin/src/zed/crates/language/src/diagnostic_set.rs` lines 21-23
Diagnostics store ranges as `Anchor` pairs in a `SumTree`. They use the same anchor resolution mechanism. When text is edited, diagnostic anchors shift naturally through the CRDT. But diagnostics are **replaced wholesale** by the language server on each update (via `update_diagnostics` at `/home/austin/src/zed/crates/language/src/buffer.rs` line 1938). There is no content-following; the language server recomputes all diagnostics.

### Git Diff Hunks
**`BufferDiff`** - `/home/austin/src/zed/crates/buffer_diff/src/buffer_diff.rs` lines 24-60
The diff system stores hunks with `Anchor` ranges. Hunks are computed by diffing the buffer text against a base text (git HEAD or index). There is no concept of "this hunk moved from here to there" — the diff is recomputed from scratch. The `DiffHunkStatusKind` (line 75) is just `Added`, `Modified`, or `Deleted`.

### Fold Fingerprints (closest analogy)
**Fold persistence** - `/home/austin/src/zed/crates/editor/src/editor.rs` lines 3846-3868
The fold system uses **content fingerprints** — 32-byte text snippets from fold boundaries — to relocate folds across sessions. On restore (lines 25637-25714), it searches the buffer for matching fingerprint text. This is the closest existing pattern to "following content that moved", but it:
- Only works across sessions (not live edits).
- Uses brute-force text search, not any CRDT-level tracking.
- Is designed for fold boundaries, not arbitrary content locations.

### Bookmark Syntactic Context (similar pattern)
**`SerializedBookmark`** - `/home/austin/src/zed/crates/project/src/bookmark_store.rs` lines 28-39
The bookmark serialization system stores `symbol_path` (outline hierarchy like `["impl DataProcessor", "fn process"]`), `offset_in_symbol`, and `context_snippet` (first 30 chars of the bookmarked line). On restore, it uses outline-based resolution (line 213) with snippet disambiguation (lines 241-254). This is for **cross-session** restoration only.

**No existing system tracks content identity across live intra-session edits.**

## 5. Transaction-Level Information for Detecting Cut+Paste

### What `buffer.edit()` provides

**`Buffer::edit()`** - `/home/austin/src/zed/crates/text/src/text.rs` lines 869-889
Each call to `edit()`:
1. Calls `start_transaction()` (line 880).
2. Creates a `clock::Lamport` timestamp (line 881).
3. Calls `apply_local_edit()` which produces an `EditOperation` (lines 897-902) containing:
   - `timestamp`: unique Lamport timestamp
   - `version`: the buffer version before the edit
   - `ranges`: the affected ranges (as `FullOffset`)
   - `new_text`: the replacement text
4. Pushes to history and end transaction (lines 884-887).

### Transaction structure

**`Transaction`** - `/home/austin/src/zed/crates/text/src/text.rs` line 135
Contains `id`, `edit_ids`, and `start` (version at transaction start). The `HistoryEntry` wrapper (line 127) adds `first_edit_at` and `last_edit_at` (wall-clock `Instant`s) and `suppress_grouping`.

### Could transactions help detect cut+paste?

**Transaction grouping** (lines 287-303): Transactions are auto-grouped on the undo stack when `first_edit_at - prev_entry.last_edit_at < group_interval` (300ms). A cut followed quickly by a paste would be grouped. However:
- Grouping is **lossy** — once merged, individual transactions lose their identity.
- Grouping provides no semantic annotation.
- The group_interval is wall-clock-based, so the grouping is timing-dependent and unreliable.

**`EditorEvent::Edited`** - `/home/austin/src/zed/crates/editor/src/editor.rs` line 28336
The editor emits `Edited { transaction_id }` after each transaction. The bookmark store could observe these events to see the sequence of transactions.

**`edited_ranges_for_transaction_id()`** - `/home/austin/src/zed/crates/text/src/text.rs` line 1658
Given a `TransactionId`, you can get back the ranges that were edited. Combined with `edits_since()` (line 2635), you can observe what text was deleted and what was inserted.

### Potential approach using transactions

To detect a cut+paste "move" using existing infrastructure, one could:
1. Observe `EditorEvent::Edited` events in sequence.
2. For the cut transaction, record the deleted text and its original position using `edited_ranges_for_transaction_id()`.
3. For the immediately following paste transaction, compare the inserted text to the recently deleted text.
4. If the texts match, treat the pair as a "move" operation.

However, this approach has significant limitations:
- It requires correlating two separate events with text comparison.
- The paste may modify the text (e.g., auto-indent changes).
- Cuts could go to the system clipboard and paste into a different editor/buffer.
- There is no infrastructure to atomically mark two transactions as a paired "move".

**No existing mechanism in the codebase provides this capability.** Any solution would need to be built from scratch, either by:
- Adding metadata to `ClipboardSelection` (e.g., a move ID or source anchor) to link cut→paste.
- Adding a new field to `Transaction` to annotate move semantics.
- Post-hoc text matching in the `BookmarkStore` when it observes edit events (similar to the fold fingerprint approach but for live edits).
</task_result>

## Assistant

This is a rich problem. Here are the main techniques that could be used, from simplest to most involved:

---

### Technique 1: Clipboard-Aware Bookmark Tracking (cut+paste only)

**How it works:** When the user cuts text, annotate the `ClipboardSelection` metadata with any bookmark anchors that fall within the cut range. When the user pastes, check the clipboard metadata for bookmarks and re-create them at the corresponding offsets within the pasted text.

**Changes needed:**
- Add a `bookmarks: Vec<(usize, ...)>` field to `ClipboardSelection` (byte offsets relative to the selection start) at `crates/editor/src/editor.rs:1735`
- In `cut_common()` (~line 13988), query the `BookmarkStore` for bookmarks within each selection's range before the deletion
- In `do_paste()` (~line 14195), after inserting text, re-create bookmarks at the adjusted positions

**Pros:** Relatively contained change; works naturally with cut+paste workflow; leverages existing clipboard metadata infrastructure.

**Cons:** Only works for cut+paste, not copy+paste (which is an ambiguous case — should the bookmark be on the original or the copy?). Doesn't work across buffers without additional handling. Doesn't handle drag-and-drop moves.

---

### Technique 2: Post-Hoc Text Matching via Edit Observation

**How it works:** The `BookmarkStore` subscribes to `BufferEvent` edit notifications. When an edit deletes text containing a bookmark, it saves the bookmark's context snippet. On subsequent insertions (within a short time window or within the same undo transaction group), it scans inserted text for the snippet and relocates the bookmark.

**Changes needed:**
- Subscribe to `BufferEvent::Edited` in `BufferBookmarks` (alongside the existing `Reparsed` and `FileHandleChanged` handlers)
- Track "displaced bookmarks" — bookmarks whose anchored text was deleted, carrying their context snippet
- On each edit, check if recently inserted text contains any displaced bookmark's snippet, and if so, re-anchor it

**Pros:** Works for cut+paste, drag-and-drop, and potentially even refactoring operations. Doesn't require changes to the clipboard system.

**Cons:** Text matching is fuzzy and could produce false positives (e.g., duplicate lines). Needs a time window or transaction correlation to avoid matching unrelated insertions. More complex state management.

---

### Technique 3: Transaction Correlation (Cut+Paste Move Detection)

**How it works:** Augment the clipboard or transaction system to explicitly link a cut and its corresponding paste as a "move" operation. When a cut deletes text containing bookmarks, store the bookmark offsets (relative to the cut text) on the clipboard. The paste operation retrieves these and re-anchors.

**Changes needed:**
- Add a `moved_bookmarks` field to `ClipboardSelection` carrying relative byte offsets
- In `cut_common()`, compute which bookmarks are within the cut ranges, record their relative offsets
- In `do_paste()`, after inserting, re-anchor bookmarks at `paste_offset + relative_offset`
- Possibly add a new `BookmarkStore` method like `move_bookmarks(old_range, new_range)`

**Pros:** Precise — no fuzzy matching. The bookmarks are deterministically relocated.

**Cons:** Essentially the same as Technique 1 but with more formality. Still only handles cut+paste, not arbitrary refactors.

---

### Technique 4: Content-Identity Fingerprinting (Most Robust)

**How it works:** Inspired by the existing fold fingerprint system (`editor.rs:3846-3868`), store a fingerprint (hash or short text sample) of the lines surrounding each bookmark. Periodically (or on buffer edit), check whether the bookmark's surrounding text still matches. If not, search the buffer for the fingerprint and relocate the bookmark.

**Changes needed:**
- Add a `fingerprint` field to `BookmarkAnchor` or an associated structure
- Compute the fingerprint from surrounding lines (e.g., the bookmarked line + 1-2 lines of context)
- On `BufferEvent::Edited`, if a bookmark's fingerprint no longer matches its current position, scan the buffer for the fingerprint and relocate

**Pros:** Works for any operation that moves text — cut/paste, drag-and-drop, refactoring, even external edits. Closest to "the bookmark follows its content."

**Cons:** Most complex to implement. Fingerprint search over the whole buffer on every edit could be expensive. Ambiguity when identical text appears in multiple locations (though the existing `context_snippet` already helps here). Needs a strategy for when fingerprints can't be found (text was modified during the move).

---

### My Recommendation

**Technique 1 (Clipboard-Aware Bookmark Tracking)** is the best starting point. It's the most natural fit for the user's reported scenario — they copy/pasted a function and expected the bookmark to follow. It:

- Is scoped to a clear, well-defined code path (`cut_common` → clipboard → `do_paste`)
- Leverages the existing `ClipboardSelection` metadata infrastructure
- Doesn't introduce fuzzy matching or scanning
- Can be extended later to handle copy+paste (e.g., by duplicating the bookmark) if desired

The key design question is: **should copy+paste also duplicate the bookmark, or only cut+paste?** Cut+paste is clearly a "move" operation, so relocating the bookmark is unambiguous. Copy+paste is a duplication — it could either leave the bookmark at the original location, duplicate it to both locations, or do nothing. I'd suggest starting with cut+paste only and treating copy+paste as a separate follow-up.

Want me to proceed with Technique 1, or do you have a preference for a different approach? Also, should copy+paste duplicate bookmarks or leave them in place?

## User

I think Technique 4 is what I want. Can you create a guided plan for implementing this?

## Assistant



Let me explore the relevant systems more deeply to build a precise plan.

**Tool Call: Explore fold fingerprint system**
Status: Completed

task_id: ses_2f7f49955ffeIWO9Eq21R6BJP2 (for resuming to continue this task if needed)

<task_result>
Now I have a complete picture. Here is the full analysis:

---

## Fold Fingerprint System: Complete Analysis

### 1. Fingerprint Computation

**File:** `/home/austin/src/zed/crates/editor/src/editor.rs`, lines 3846-3869

```rust
const FINGERPRINT_LEN: usize = 32;
let db_folds = display_snapshot
    .folds_in_range(MultiBufferOffset(0)..display_snapshot.buffer_snapshot().len())
    .map(|fold| {
        let start = fold.range.start.text_anchor.to_offset(&snapshot);
        let end = fold.range.end.text_anchor.to_offset(&snapshot);

        // Extract fingerprints - content at fold boundaries for validation on restore
        // Both fingerprints must be INSIDE the fold to avoid capturing surrounding
        // content that might change independently.
        // start_fp: first min(32, fold_len) bytes of fold content
        // end_fp: last min(32, fold_len) bytes of fold content
        // Clip to character boundaries to handle multibyte UTF-8 characters.
        let fold_len = end - start;
        let start_fp_end = snapshot
            .clip_offset(start + std::cmp::min(FINGERPRINT_LEN, fold_len), Bias::Left);
        let start_fp: String = snapshot.text_for_range(start..start_fp_end).collect();
        let end_fp_start = snapshot
            .clip_offset(end.saturating_sub(FINGERPRINT_LEN).max(start), Bias::Right);
        let end_fp: String = snapshot.text_for_range(end_fp_start..end).collect();

        (start, end, start_fp, end_fp)
    })
    .collect::<Vec<_>>();
```

**How it works:**
- **Two fingerprints per fold**: a `start_fp` and an `end_fp`, both sampled from **inside** the fold range.
- `start_fp` = the first `min(32, fold_len)` bytes of the fold content (from fold start).
- `end_fp` = the last `min(32, fold_len)` bytes of the fold content (ending at fold end).
- For short folds (< 64 bytes), the two fingerprints overlap. For folds <= 32 bytes, they are **identical**.
- `clip_offset` is used to ensure sampling boundaries fall on valid UTF-8 character boundaries.
- The fingerprints are raw text strings, not hashes.

---

### 2. Fingerprint Storage

**File:** `/home/austin/src/zed/crates/editor/src/persistence.rs`, lines 212-225 (schema) and 374-397 (save function)

**Database schema** (`file_folds` table):
```sql
CREATE TABLE file_folds (
    workspace_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    start_fingerprint TEXT,
    end_fingerprint TEXT,
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
        ON DELETE CASCADE
        ON UPDATE CASCADE,
    PRIMARY KEY(workspace_id, path, start)
);
```

**Save function** (`save_file_folds`):
```rust
pub async fn save_file_folds(
    &self,
    workspace_id: WorkspaceId,
    path: Arc<Path>,
    folds: Vec<(usize, usize, String, String)>,  // (start_offset, end_offset, start_fp, end_fp)
) -> Result<()> {
    self.write(move |conn| {
        // Clear existing folds for this file
        conn.exec_bound(sql!(
            DELETE FROM file_folds WHERE workspace_id = ?1 AND path = ?2;
        ))?((workspace_id, path.as_ref()))?;
        // Insert each fold
        for (start, end, start_fp, end_fp) in folds {
            conn.exec_bound(sql!(
                INSERT INTO file_folds (workspace_id, path, start, end, start_fingerprint, end_fingerprint)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6);
            ))?((workspace_id, path.as_ref(), start, end, start_fp, end_fp))?;
        }
        Ok(())
    })
    .await
}
```

**In-memory representation:**
- Stored as `serialize_folds: Task<()>` field on the `Editor` struct (line 1324). This is a background task that gets replaced each time folds change.

**What is stored per fold:**
- `start` (byte offset)
- `end` (byte offset)
- `start_fingerprint` (raw text string, up to 32 bytes)
- `end_fingerprint` (raw text string, up to 32 bytes)

The primary key is `(workspace_id, path, start)` -- keyed by file path, not editor ID, so folds survive tab close.

**Retrieval function** (`get_file_folds`, line 311):
```rust
pub fn get_file_folds(
    workspace_id: WorkspaceId,
    path: &Path
) -> Result<Vec<(usize, usize, Option<String>, Option<String>)>> {
    SELECT start, end, start_fingerprint, end_fingerprint
    FROM file_folds
    WHERE workspace_id = ?1 AND path = ?2
    ORDER BY start
}
```

Note: fingerprints are `Option<String>` on retrieval because the old `editor_folds` table may lack them (migration path).

---

### 3. Fingerprint Matching / Restore Logic

**File:** `/home/austin/src/zed/crates/editor/src/editor.rs`, lines 25637-25726 (in `read_metadata_from_db`) and duplicated at lines 25764-25853 (in `load_folds_from_db`)

The matching uses a **three-tier strategy**:

```rust
// Helper: linear scan for fingerprint text in buffer starting at search_start
let find_fingerprint = |fingerprint: &str, search_start: usize| -> Option<usize> {
    let search_start = snapshot
        .clip_offset(MultiBufferOffset(search_start), Bias::Left)
        .0;
    let search_end = snapshot_len.saturating_sub(fingerprint.len());

    let mut byte_offset = search_start;
    for ch in snapshot.chars_at(MultiBufferOffset(search_start)) {
        if byte_offset > search_end {
            break;
        }
        if snapshot.contains_str_at(MultiBufferOffset(byte_offset), fingerprint) {
            return Some(byte_offset);
        }
        byte_offset += ch.len_utf8();
    }
    None
};

// Track search position to handle duplicate fingerprints correctly.
// Folds are stored in document order, so we advance after each match.
let mut search_start = 0usize;

let valid_folds: Vec<_> = folds
    .into_iter()
    .filter_map(|(stored_start, stored_end, start_fp, end_fp)| {
        // Skip folds without fingerprints (old data before migration)
        let sfp = start_fp?;
        let efp = end_fp?;
        let efp_len = efp.len();

        // === TIER 1: Fast path - check stored offsets directly ===
        let start_matches = stored_start < snapshot_len
            && snapshot.contains_str_at(MultiBufferOffset(stored_start), &sfp);
        let efp_check_pos = stored_end.saturating_sub(efp_len);
        let end_matches = efp_check_pos >= stored_start
            && stored_end <= snapshot_len
            && snapshot.contains_str_at(MultiBufferOffset(efp_check_pos), &efp);

        let (new_start, new_end) = if start_matches && end_matches {
            // Offsets unchanged, use stored values
            (stored_start, stored_end)
        } else if sfp == efp {
            // === TIER 2: Short fold (identical fingerprints) ===
            // Use stored fold length to compute new_end
            let new_start = find_fingerprint(&sfp, search_start)?;
            let fold_len = stored_end - stored_start;
            let new_end = new_start + fold_len;
            (new_start, new_end)
        } else {
            // === TIER 3: Full search for both fingerprints ===
            let new_start = find_fingerprint(&sfp, search_start)?;
            // Search for end_fp after start, then add efp_len to get actual fold end
            let efp_pos = find_fingerprint(&efp, new_start + sfp.len())?;
            let new_end = efp_pos + efp_len;
            (new_start, new_end)
        };

        // Advance search position for next fold
        search_start = new_end;

        // Validate fold makes sense (end must be after start)
        if new_end <= new_start {
            return None;
        }

        Some(
            snapshot.clip_offset(MultiBufferOffset(new_start), Bias::Left)
                ..snapshot.clip_offset(MultiBufferOffset(new_end), Bias::Right),
        )
    })
    .collect();

if !valid_folds.is_empty() {
    self.fold_ranges(valid_folds, false, window, cx);
}
```

**`contains_str_at` implementation** (`/home/austin/src/zed/crates/text/src/text.rs`, line 2100):
```rust
pub fn contains_str_at<T>(&self, position: T, needle: &str) -> bool
where
    T: ToOffset,
{
    let position = position.to_offset(self);
    position == self.clip_offset(position, Bias::Left)
        && self
            .bytes_in_range(position..self.len())
            .flatten()
            .copied()
            .take(needle.len())
            .eq(needle.bytes())
}
```

---

### 4. Ambiguity Handling (Multiple Matches)

The system handles ambiguity through these mechanisms:

1. **Forward-only monotonic search** (`search_start` advances after each match, line 25662-25706): Folds are stored in document order (`ORDER BY start` in SQL). After each fold is matched, `search_start` is advanced to `new_end`, so subsequent folds can only match at positions **after** the previous fold. This means if two folds have the same fingerprint, they get assigned to the first and second occurrence respectively in document order.

2. **Short-fold special case** (line 25689-25695): When `sfp == efp` (fold <= 32 bytes, identical fingerprints), the system searches for just one fingerprint and uses the **stored fold length** to compute the end offset, rather than searching for end_fp separately. This prevents a short fold from matching its own end at an incorrect location.

3. **Fold discarded if not found**: If either fingerprint cannot be found (the `?` on `find_fingerprint` calls), or if `new_end <= new_start`, the fold is silently dropped (`filter_map` returns `None`).

4. **No disambiguation beyond ordering**: There is no scoring, ranking, or distance-based heuristic. The system takes the **first match** at or after `search_start`. If content has been duplicated or reordered, folds may silently mismatch or be lost.

---

### 5. Events That Trigger Fingerprint Checking

**Serialization triggers** (live, during the session):

The `folds_did_change` method (line 3801) is called whenever folds are modified:
- `fold_creases` (line 20703) -- folding
- `unfold_ranges` (line 20717) -- unfolding
- `remove_folds_with_type` (line 20814) -- removing folds by type
- `clone` (line 1967) -- cloning an editor

`folds_did_change` serializes folds to the database via a **background task** with a 200ms throttle (`SERIALIZATION_THROTTLE_TIME`, defined at `/home/austin/src/zed/crates/workspace/src/workspace.rs`, line 159). The previous serialization task is replaced (dropped/cancelled) each time, providing debouncing.

**Restore triggers** (cross-session, on open):

1. **`read_metadata_from_db`** (line 25596): Called when an editor is opened with a persisted item ID. Tries `file_folds` table first (path-based), falls back to `editor_folds` (legacy ID-based), and performs migration if needed.

2. **`load_folds_from_db`** (line 25764): Called when manually opening a file that was previously closed. Uses only the `file_folds` table (path-based lookup).

Both restore paths use identical fingerprint matching logic. The fingerprint matching is **only** performed at restore time (cross-session). During a live session, folds are tracked by anchors in the buffer, which automatically adjust as text changes -- fingerprints are only needed for database persistence where anchors cannot survive.

---

### Summary of Key Files and Line Numbers

| Component | File | Lines |
|---|---|---|
| Fingerprint computation | `crates/editor/src/editor.rs` | 3846-3869 |
| Serialization to DB | `crates/editor/src/editor.rs` | 3870-3886 |
| `serialize_folds` field | `crates/editor/src/editor.rs` | 1324 |
| Trigger: `folds_did_change` | `crates/editor/src/editor.rs` | 3801-3886 |
| Restore (read_metadata_from_db) | `crates/editor/src/editor.rs` | 25596-25741 |
| Restore (load_folds_from_db) | `crates/editor/src/editor.rs` | 25764-25854 |
| `file_folds` schema | `crates/editor/src/persistence.rs` | 212-225 |
| `save_file_folds` | `crates/editor/src/persistence.rs` | 374-397 |
| `get_file_folds` | `crates/editor/src/persistence.rs` | 311-319 |
| `get_editor_folds` (legacy) | `crates/editor/src/persistence.rs` | 300-308 |
| `delete_file_folds` | `crates/editor/src/persistence.rs` | 399-410 |
| `contains_str_at` (core match) | `crates/text/src/text.rs` | 2100-2112 |
| `SERIALIZATION_THROTTLE_TIME` | `crates/workspace/src/workspace.rs` | 159 |
| Tests | `crates/editor/src/persistence.rs` | 502-599 |
</task_result>

## Assistant

<thinking>
Now I need to deeply understand the bookmark system to create a precise implementation plan. Let me explore the bookmark store, bookmark anchors, and how they're currently managed.
</thinking>

Let me explore the bookmark system in more detail to understand the current structures and event handling.

**Tool Call: Explore BookmarkStore and BookmarkAnchor structures**
Status: Completed

Now I have the complete picture. Here's the thorough breakdown:

---

## Full Bookmark System in Zed

### 1. `BookmarkAnchor` struct

```zed/crates/project/src/bookmark_store.rs#L15-22
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct BookmarkAnchor(text::Anchor);

impl BookmarkAnchor {
    pub fn anchor(&self) -> text::Anchor {
        self.0
    }
}
```

A simple **newtype wrapper** around `text::Anchor`. It holds a single field—a CRDT-based `Anchor` from the `text` crate. The anchor tracks a position in a buffer that adjusts automatically as characters are inserted/deleted around it (this is inherent to `text::Anchor` behavior, not bookmark-specific logic). The only method exposes the inner anchor.

---

### 2. `BufferBookmarks` struct

```zed/crates/project/src/bookmark_store.rs#L41-80
#[derive(Debug)]
pub struct BufferBookmarks {
    buffer: Entity<Buffer>,
    bookmarks: Vec<BookmarkAnchor>,
    /// Serialized bookmarks with syntactic context waiting for tree-sitter
    /// to finish parsing so that outline-based resolution can be attempted.
    pending_syntactic: Vec<SerializedBookmark>,
    _subscription: Subscription,
}

impl BufferBookmarks {
    pub fn new(buffer: Entity<Buffer>, cx: &mut Context<BookmarkStore>) -> Self {
        let subscription = cx.subscribe(
            &buffer,
            |bookmark_store, buffer, event: &BufferEvent, cx| match event {
                BufferEvent::FileHandleChanged => {
                    bookmark_store.handle_file_changed(buffer, cx);
                }
                BufferEvent::Reparsed => {
                    bookmark_store.resolve_pending_syntactic_bookmarks(buffer, cx);
                }
                _ => {}
            },
        );

        Self {
            buffer,
            bookmarks: Vec::new(),
            pending_syntactic: Vec::new(),
            _subscription: subscription,
        }
    }

    pub fn buffer(&self) -> &Entity<Buffer> {
        &self.buffer
    }

    pub fn bookmarks(&self) -> &[BookmarkAnchor] {
        &self.bookmarks
    }
}
```

**Fields:**
- `buffer` — the `Entity<Buffer>` this bookmark set belongs to
- `bookmarks` — the live, resolved `BookmarkAnchor` positions
- `pending_syntactic` — serialized bookmarks that had syntactic context (symbol paths) but couldn't be resolved because tree-sitter hadn't finished parsing yet
- `_subscription` — a `Subscription` to buffer events (kept alive by ownership)

**How it manages bookmarks:** It subscribes to two `BufferEvent` variants on the buffer:
- `BufferEvent::FileHandleChanged` → delegates to `BookmarkStore::handle_file_changed` (handles renames/deletes)
- `BufferEvent::Reparsed` → delegates to `BookmarkStore::resolve_pending_syntactic_bookmarks` (re-resolves deferred bookmarks once the outline is available)

---

### 3. How bookmarks respond to buffer edits

**There is NO subscription to `BufferEvent::Edited`.** The subscription (shown above) only handles `FileHandleChanged` and `Reparsed`. The `_ => {}` wildcard discards everything else, including `Edited`.

Bookmarks survive edits because `BookmarkAnchor` wraps `text::Anchor`, which is a CRDT anchor—the `text` crate's rope data structure automatically adjusts anchor positions as edits occur. There is no explicit "relocate on edit" logic in the bookmark system; that stability comes from the anchor primitive itself.

There is **no "relocate" or "reanchor" logic** in the bookmark code for handling edits. The only reanchoring that exists is:
- **On deserialization/restore:** `resolve_anchors_if_needed` converts row numbers back to anchors
- **On tree-sitter reparse:** `resolve_pending_syntactic_bookmarks` moves bookmarks from their fallback row positions to syntactically-resolved positions

---

### 4. `BookmarkStore` struct and all its methods

```zed/crates/project/src/bookmark_store.rs#L98-102
pub struct BookmarkStore {
    buffer_store: Entity<BufferStore>,
    worktree_store: Entity<WorktreeStore>,
    bookmarks: BTreeMap<Arc<Path>, BookmarkEntry>,
}
```

The `BookmarkEntry` enum:

```zed/crates/project/src/bookmark_store.rs#L83-96
#[derive(Debug)]
pub enum BookmarkEntry {
    Loaded(BufferBookmarks),
    Unloaded(Vec<SerializedBookmark>),
}

impl BookmarkEntry {
    pub fn is_empty(&self) -> bool {
        match self {
            BookmarkEntry::Loaded(buffer_bookmarks) => buffer_bookmarks.bookmarks.is_empty(),
            BookmarkEntry::Unloaded(rows) => rows.is_empty(),
        }
    }
}
```

All `BookmarkStore` methods:

#### `new`
```zed/crates/project/src/bookmark_store.rs#L105-111
    pub fn new(worktree_store: Entity<WorktreeStore>, buffer_store: Entity<BufferStore>) -> Self {
        Self {
            buffer_store,
            worktree_store,
            bookmarks: BTreeMap::new(),
        }
    }
```

#### `with_serialized_bookmarks` — loads serialized bookmarks from persistence
```zed/crates/project/src/bookmark_store.rs#L113-135
    pub fn with_serialized_bookmarks(
        &mut self,
        bookmarks: BTreeMap<Arc<Path>, Vec<SerializedBookmark>>,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.bookmarks.clear();

        for (path, serialized) in bookmarks {
            if serialized.is_empty() {
                continue;
            }

            let count = serialized.len();
            let word = if count == 1 { "bookmark" } else { "bookmarks" };
            log::debug!("Stored {count} unloaded {word} at {}", path.display());

            self.bookmarks
                .insert(path, BookmarkEntry::Unloaded(serialized));
        }

        cx.notify();
        Task::ready(Ok(()))
    }
```

#### `resolve_anchors_if_needed` — converts `Unloaded` → `Loaded` on demand
```zed/crates/project/src/bookmark_store.rs#L137-209
    fn resolve_anchors_if_needed(
        &mut self,
        abs_path: &Arc<Path>,
        buffer: &Entity<Buffer>,
        cx: &mut Context<Self>,
    ) {
        let Some(BookmarkEntry::Unloaded(serialized)) = self.bookmarks.get(abs_path) else {
            return;
        };

        let snapshot = buffer.read(cx).snapshot();
        let max_point = snapshot.max_point();

        let outline = snapshot.outline(None);
        let outline_items: &[OutlineItem<Anchor>] = &outline.items;

        let has_syntactic_bookmarks = serialized
            .iter()
            .any(|b| b.symbol_path.as_ref().is_some_and(|p| !p.is_empty()));
        let outline_is_empty = outline_items.is_empty();

        // Collect bookmarks that have syntactic context but can't be resolved
        // yet because the outline is empty (tree-sitter hasn't parsed yet).
        let mut pending_syntactic = Vec::new();

        let anchors: Vec<BookmarkAnchor> = serialized
            .iter()
            .filter_map(|bookmark| {
                if !outline_is_empty {
                    if let Some(resolved_row) =
                        Self::resolve_syntactic_bookmark(bookmark, outline_items, &snapshot)
                    {
                        let point = Point::new(resolved_row, 0);
                        if point > max_point {
                            return None;
                        }
                        let anchor = snapshot.anchor_after(point);
                        return Some(BookmarkAnchor(anchor));
                    }
                } else if bookmark.symbol_path.as_ref().is_some_and(|p| !p.is_empty()) {
                    // Outline not available yet; save for deferred resolution
                    // after tree-sitter parses.
                    pending_syntactic.push(bookmark.clone());
                }

                let point = Point::new(bookmark.row, 0);
                if point > max_point {
                    log::warn!(
                        "Skipping out-of-range bookmark: {} row {} (file has {} rows)",
                        abs_path.display(),
                        bookmark.row,
                        max_point.row
                    );
                    return None;
                }

                let anchor = snapshot.anchor_after(point);
                Some(BookmarkAnchor(anchor))
            })
            .collect();

        if anchors.is_empty() && pending_syntactic.is_empty() {
            self.bookmarks.remove(abs_path);
        } else {
            let mut buffer_bookmarks = BufferBookmarks::new(buffer.clone(), cx);
            buffer_bookmarks.bookmarks = anchors;
            if has_syntactic_bookmarks && outline_is_empty {
                buffer_bookmarks.pending_syntactic = pending_syntactic;
            }
            self.bookmarks
                .insert(abs_path.clone(), BookmarkEntry::Loaded(buffer_bookmarks));
        }
    }
```

#### `resolve_syntactic_bookmark` — matches a serialized bookmark to an outline symbol
```zed/crates/project/src/bookmark_store.rs#L213-281
    fn resolve_syntactic_bookmark(
        bookmark: &SerializedBookmark,
        outline_items: &[OutlineItem<Anchor>],
        snapshot: &language::BufferSnapshot,
    ) -> Option<u32> {
        let symbol_path = bookmark.symbol_path.as_ref()?;
        if symbol_path.is_empty() {
            return None;
        }

        let innermost_name = symbol_path.last()?;

        // Find candidate items whose text matches the innermost symbol name,
        // collecting all full path matches to allow snippet-based disambiguation.
        let mut full_matches: Vec<(usize, &OutlineItem<Anchor>)> = Vec::new();

        for (index, item) in outline_items.iter().enumerate() {
            if &item.text != innermost_name {
                continue;
            }

            if Self::matches_symbol_path(symbol_path, index, outline_items) {
                full_matches.push((index, item));
            }
        }

        // When multiple outline sections share the same symbol path, use the
        // context snippet to pick the right one.
        let best_match = if full_matches.len() > 1 {
            if let Some(snippet) = bookmark.context_snippet.as_deref() {
                full_matches
                    .iter()
                    .find(|(_, item)| {
                        let item_start_row = item.range.start.to_point(snapshot).row;
                        let candidate_row = if let Some(offset) = bookmark.offset_in_symbol {
                            let item_end_row = item.range.end.to_point(snapshot).row;
                            (item_start_row + offset).min(item_end_row)
                        } else {
                            item_start_row
                        };
                        Self::compute_context_snippet(snapshot, candidate_row).as_deref()
                            == Some(snippet)
                    })
                    .or(full_matches.first())
                    .copied()
            } else {
                full_matches.first().copied()
            }
        } else {
            full_matches.first().copied()
        };

        // If exact match failed, try matching just the innermost name.
        let matched_item = best_match.map(|(_, item)| item).or_else(|| {
            outline_items
                .iter()
                .find(|item| &item.text == innermost_name)
        })?;

        let item_start_row = matched_item.range.start.to_point(snapshot).row;
        let resolved_row = if let Some(offset) = bookmark.offset_in_symbol {
            let item_end_row = matched_item.range.end.to_point(snapshot).row;
            (item_start_row + offset).min(item_end_row)
        } else {
            item_start_row
        };

        Some(resolved_row)
    }
```

#### `matches_symbol_path` — walks ancestor chain to verify full symbol path match
```zed/crates/project/src/bookmark_store.rs#L284-312
    fn matches_symbol_path(
        symbol_path: &[String],
        item_index: usize,
        outline_items: &[OutlineItem<Anchor>],
    ) -> bool {
        if symbol_path.len() == 1 {
            return true;
        }

        let target_depth = outline_items[item_index].depth;

        // Walk backwards through the symbol path, matching ancestors.
        let mut remaining_path: &[String] = &symbol_path[..symbol_path.len() - 1];
        let mut current_depth = target_depth;

        for item in outline_items[..item_index].iter().rev() {
            if remaining_path.is_empty() {
                break;
            }
            if item.depth < current_depth {
                if &item.text == remaining_path.last().expect("checked non-empty above") {
                    remaining_path = &remaining_path[..remaining_path.len() - 1];
                }
                current_depth = item.depth;
            }
        }

        remaining_path.is_empty()
    }
```

#### `resolve_all` — opens buffers for all unloaded bookmarks and resolves them
```zed/crates/project/src/bookmark_store.rs#L315-387
    pub fn resolve_all(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let unloaded_paths: Vec<Arc<Path>> = self
            .bookmarks
            .iter()
            .filter_map(|(path, entry)| match entry {
                BookmarkEntry::Unloaded(_) => Some(path.clone()),
                BookmarkEntry::Loaded(_) => None,
            })
            .collect();

        if unloaded_paths.is_empty() {
            return Task::ready(Ok(()));
        }

        let worktree_store = self.worktree_store.downgrade();
        let buffer_store = self.buffer_store.downgrade();

        cx.spawn(async move |this, cx| {
            let open_tasks: Vec<_> = unloaded_paths
                .into_iter()
                .map(|path| {
                    let worktree_store = worktree_store.clone();
                    let buffer_store = buffer_store.clone();
                    let mut cx = cx.clone();
                    async move {
                        let result: Result<Entity<Buffer>> = async {
                            let (worktree, relative_path) = worktree_store
                                .update(&mut cx, |worktree_store, cx| {
                                    worktree_store.find_or_create_worktree(&path, false, cx)
                                })?
                                .await?;

                            let buffer = buffer_store
                                .update(&mut cx, |buffer_store, cx| {
                                    let project_path = ProjectPath {
                                        worktree_id: worktree.read(cx).id(),
                                        path: relative_path,
                                    };
                                    buffer_store.open_buffer(project_path, cx)
                                })?
                                .await?;

                            Ok(buffer)
                        }
                        .await;

                        (path, result)
                    }
                })
                .collect();

            let results = futures::future::join_all(open_tasks).await;

            this.update(cx, |this, cx| {
                for (path, result) in results {
                    match result {
                        Ok(buffer) => {
                            this.resolve_anchors_if_needed(&path, &buffer, cx);
                        }
                        Err(error) => {
                            log::warn!(
                                "Could not open buffer for bookmarked path {}: {error}",
                                path.display()
                            );
                        }
                    }
                }
                cx.notify();
            })?;

            Ok(())
        })
    }
```

#### `abs_path_from_buffer` — utility to extract absolute path from a buffer
```zed/crates/project/src/bookmark_store.rs#L389-393
    pub fn abs_path_from_buffer(buffer: &Entity<Buffer>, cx: &App) -> Option<Arc<Path>> {
        worktree::File::from_dyn(buffer.read(cx).file())
            .map(|file| file.worktree.read(cx).absolutize(&file.path))
            .map(Arc::<Path>::from)
    }
```

#### `compute_syntactic_context` — builds the symbol path + offset for serialization
```zed/crates/project/src/bookmark_store.rs#L397-413
    pub fn compute_syntactic_context(
        snapshot: &language::BufferSnapshot,
        row: u32,
    ) -> (Option<Vec<String>>, Option<u32>) {
        let symbols = snapshot.symbols_containing(Point::new(row, 0), None);
        if symbols.is_empty() {
            return (None, None);
        }

        let symbol_path: Vec<String> = symbols.iter().map(|item| item.text.clone()).collect();

        let innermost = &symbols[symbols.len() - 1];
        let item_start_row = innermost.range.start.to_point(snapshot).row;
        let offset = row.saturating_sub(item_start_row);

        (Some(symbol_path), Some(offset))
    }
```

#### `compute_context_snippet` — extracts first 30 chars of a line for disambiguation
```zed/crates/project/src/bookmark_store.rs#L415-440
    const CONTEXT_SNIPPET_MAX_LEN: usize = 30;

    pub fn compute_context_snippet(
        snapshot: &language::BufferSnapshot,
        row: u32,
    ) -> Option<String> {
        let max_row = snapshot.max_point().row;
        if row > max_row {
            return None;
        }
        let line_start = Point::new(row, 0);
        let line_end = Point::new(row, snapshot.line_len(row));
        let line_text: String = snapshot.text_for_range(line_start..line_end).collect();
        let trimmed = line_text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let clipped: String = trimmed
            .chars()
            .take(Self::CONTEXT_SNIPPET_MAX_LEN)
            .collect();
        Some(clipped)
    }
```

#### `toggle_bookmark` — adds or removes a bookmark at a row
```zed/crates/project/src/bookmark_store.rs#L445-482
    pub fn toggle_bookmark(
        &mut self,
        buffer: Entity<Buffer>,
        anchor: text::Anchor,
        cx: &mut Context<Self>,
    ) {
        let Some(abs_path) = Self::abs_path_from_buffer(&buffer, cx) else {
            return;
        };

        self.resolve_anchors_if_needed(&abs_path, &buffer, cx);

        let entry = self
            .bookmarks
            .entry(abs_path.clone())
            .or_insert_with(|| BookmarkEntry::Loaded(BufferBookmarks::new(buffer.clone(), cx)));

        let BookmarkEntry::Loaded(buffer_bookmarks) = entry else {
            unreachable!("resolve_if_needed should have converted to Loaded");
        };

        let snapshot = buffer.read(cx).snapshot();

        let existing_index = buffer_bookmarks.bookmarks.iter().position(|existing| {
            existing.0.summary::<Point>(&snapshot).row == anchor.summary::<Point>(&snapshot).row
        });

        if let Some(index) = existing_index {
            buffer_bookmarks.bookmarks.remove(index);
            if buffer_bookmarks.bookmarks.is_empty() {
                self.bookmarks.remove(&abs_path);
            }
        } else {
            buffer_bookmarks.bookmarks.push(BookmarkAnchor(anchor));
        }

        cx.notify();
    }
```

#### `bookmarks` — read-only access to the full map
```zed/crates/project/src/bookmark_store.rs#L484-486
    pub fn bookmarks(&self) -> &BTreeMap<Arc<Path>, BookmarkEntry> {
        &self.bookmarks
    }
```

#### `bookmarks_for_buffer` — returns resolved bookmarks for a buffer, optionally within a range
```zed/crates/project/src/bookmark_store.rs#L491-528
    pub fn bookmarks_for_buffer(
        &mut self,
        buffer: Entity<Buffer>,
        range: Option<Range<text::Anchor>>,
        buffer_snapshot: &BufferSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<BookmarkAnchor> {
        let Some(abs_path) = Self::abs_path_from_buffer(&buffer, cx) else {
            return Vec::new();
        };

        self.resolve_anchors_if_needed(&abs_path, &buffer, cx);

        let Some(BookmarkEntry::Loaded(file_bookmarks)) = self.bookmarks.get(&abs_path) else {
            return Vec::new();
        };

        file_bookmarks
            .bookmarks
            .iter()
            .filter_map({
                move |bookmark| {
                    if !buffer_snapshot.can_resolve(&bookmark.anchor()) {
                        return None;
                    }

                    if let Some(range) = &range
                        && (bookmark.anchor().cmp(&range.start, buffer_snapshot).is_lt()
                            || bookmark.anchor().cmp(&range.end, buffer_snapshot).is_gt())
                    {
                        return None;
                    }

                    Some(*bookmark)
                }
            })
            .collect()
    }
```

#### `handle_file_changed` — handles renames and deletes
```zed/crates/project/src/bookmark_store.rs#L530-575
    fn handle_file_changed(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        let entity_id = buffer.entity_id();

        if buffer
            .read(cx)
            .file()
            .is_none_or(|f| f.disk_state().is_deleted())
        {
            self.bookmarks.retain(|_, entry| match entry {
                BookmarkEntry::Loaded(buffer_bookmarks) => {
                    buffer_bookmarks.buffer.entity_id() != entity_id
                }
                BookmarkEntry::Unloaded(_) => true,
            });
            cx.notify();
            return;
        }

        if let Some(new_abs_path) = Self::abs_path_from_buffer(&buffer, cx) {
            if self.bookmarks.contains_key(&new_abs_path) {
                return;
            }

            if let Some(old_path) = self
                .bookmarks
                .iter()
                .find(|(_, entry)| match entry {
                    BookmarkEntry::Loaded(buffer_bookmarks) => {
                        buffer_bookmarks.buffer.entity_id() == entity_id
                    }
                    BookmarkEntry::Unloaded(_) => false,
                })
                .map(|(path, _)| path)
                .cloned()
            {
                let Some(entry) = self.bookmarks.remove(&old_path) else {
                    log::error!(
                        "Couldn't get bookmarks from old path during buffer rename handling"
                    );
                    return;
                };
                self.bookmarks.insert(new_abs_path, entry);
                cx.notify();
            }
        }
    }
```

#### `resolve_pending_syntactic_bookmarks` — re-resolves deferred bookmarks after tree-sitter parses
```zed/crates/project/src/bookmark_store.rs#L580-644
    fn resolve_pending_syntactic_bookmarks(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) {
        let Some(abs_path) = Self::abs_path_from_buffer(&buffer, cx) else {
            return;
        };

        let Some(BookmarkEntry::Loaded(buffer_bookmarks)) = self.bookmarks.get_mut(&abs_path)
        else {
            return;
        };

        if buffer_bookmarks.pending_syntactic.is_empty() {
            return;
        }

        let pending = std::mem::take(&mut buffer_bookmarks.pending_syntactic);

        let snapshot = buffer.read(cx).snapshot();
        let max_point = snapshot.max_point();
        let outline = snapshot.outline(None);
        let outline_items: &[OutlineItem<Anchor>] = &outline.items;

        if outline_items.is_empty() {
            // Still no outline — put them back for the next reparse.
            if let Some(BookmarkEntry::Loaded(bm)) = self.bookmarks.get_mut(&abs_path) {
                bm.pending_syntactic = pending;
            }
            return;
        }

        let text_snapshot = buffer.read(cx).text_snapshot();

        for serialized in &pending {
            let Some(resolved_row) =
                Self::resolve_syntactic_bookmark(serialized, outline_items, &snapshot)
            else {
                continue;
            };

            let point = Point::new(resolved_row, 0);
            if point > max_point {
                continue;
            }

            // Find the existing bookmark that was placed at the fallback row
            // and move it to the syntactically-resolved position.
            let fallback_row = serialized.row;
            let new_anchor = snapshot.anchor_after(point);

            if let Some(BookmarkEntry::Loaded(bm)) = self.bookmarks.get_mut(&abs_path) {
                if let Some(existing) = bm
                    .bookmarks
                    .iter_mut()
                    .find(|b| b.0.summary::<Point>(&text_snapshot).row == fallback_row)
                {
                    *existing = BookmarkAnchor(new_anchor);
                }
            }
        }

        cx.notify();
    }
```

#### `all_serialized_bookmarks` — converts everything back to `SerializedBookmark` for persistence
```zed/crates/project/src/bookmark_store.rs#L646-690
    pub fn all_serialized_bookmarks(
        &self,
        cx: &App,
    ) -> BTreeMap<Arc<Path>, Vec<SerializedBookmark>> {
        self.bookmarks
            .iter()
            .filter_map(|(path, entry)| {
                let mut serialized = match entry {
                    BookmarkEntry::Unloaded(bookmarks) => bookmarks.clone(),
                    BookmarkEntry::Loaded(buffer_bookmarks) => {
                        let snapshot = buffer_bookmarks.buffer.read(cx).snapshot();
                        buffer_bookmarks
                            .bookmarks
                            .iter()
                            .filter_map(|bookmark| {
                                if !snapshot.can_resolve(&bookmark.anchor()) {
                                    return None;
                                }
                                let row =
                                    snapshot.summary_for_anchor::<Point>(&bookmark.anchor()).row;
                                let (symbol_path, offset_in_symbol) =
                                    Self::compute_syntactic_context(&snapshot, row);
                                let context_snippet = Self::compute_context_snippet(&snapshot, row);
                                Some(SerializedBookmark {
                                    row,
                                    symbol_path,
                                    offset_in_symbol,
                                    context_snippet,
                                })
                            })
                            .collect()
                    }
                };

                serialized.sort_by_key(|b| b.row);
                serialized.dedup_by_key(|b| b.row);

                if serialized.is_empty() {
                    None
                } else {
                    Some((path.clone(), serialized))
                }
            })
            .collect()
    }
```

#### `all_bookmark_locations` — returns point ranges for rendering
```zed/crates/project/src/bookmark_store.rs#L692-713
    pub fn all_bookmark_locations(&self, cx: &App) -> HashMap<Entity<Buffer>, Vec<Range<Point>>> {
        let mut locations: HashMap<Entity<Buffer>, Vec<Range<Point>>> = HashMap::default();

        for (_, entry) in &self.bookmarks {
            let BookmarkEntry::Loaded(buffer_bookmarks) = entry else {
                continue;
            };
            let buffer = buffer_bookmarks.buffer().clone();
            let snapshot = buffer.read(cx).snapshot();
            let ranges: Vec<Range<Point>> = buffer_bookmarks
                .bookmarks()
                .iter()
                .map(|anchor| {
                    let row = snapshot.summary_for_anchor::<Point>(&anchor.anchor()).row;
                    Point::row_range(row..row)
                })
                .collect();
            locations.entry(buffer).or_default().extend(ranges);
        }

        locations
    }
```

#### `clear_bookmarks`
```zed/crates/project/src/bookmark_store.rs#L715-718
    pub fn clear_bookmarks(&mut self, cx: &mut Context<Self>) {
        self.bookmarks.clear();
        cx.notify();
    }
```

---

### 5. Serialization / Persistence

#### `SerializedBookmark` — the domain model for persistence
```zed/crates/project/src/bookmark_store.rs#L25-39
/// A bookmark serialized with optional syntactic context for cross-session stability.
/// When a symbol_path is present, restoration will attempt to locate the matching
/// syntactic construct first, falling back to the raw row number if not found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerializedBookmark {
    /// The row number at time of serialization (fallback anchor).
    pub row: u32,
    /// Hierarchical path of enclosing outline symbol names, from outermost to innermost.
    /// e.g. `["impl DataProcessor", "fn process"]`
    pub symbol_path: Option<Vec<String>>,
    /// Row offset of the bookmark relative to the start of the innermost symbol.
    pub offset_in_symbol: Option<u32>,
    /// First 30 characters of the bookmarked line, used to disambiguate when
    /// multiple outline sections share the same symbol path.
    pub context_snippet: Option<String>,
}
```

#### `Bookmark` — the SQLite row model
```zed/crates/workspace/src/persistence.rs#L372-456
pub struct Bookmark {
    pub row: u32,
    pub symbol_path: Option<String>,
    pub offset_in_symbol: Option<u32>,
    pub context_snippet: Option<String>,
}
```

With full `Bind` and `Column` impls for SQLite serialization (lines 380–456 shown above).

#### Database schema — initial table + migrations
```zed/crates/workspace/src/persistence.rs#L1063-1079
        sql!(
            CREATE TABLE bookmarks (
                workspace_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                row INTEGER NOT NULL,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
                ON UPDATE CASCADE
            );
        ),
        sql!(
            ALTER TABLE bookmarks ADD COLUMN symbol_path TEXT;
            ALTER TABLE bookmarks ADD COLUMN offset_in_symbol INTEGER;
        ),
        sql!(
            ALTER TABLE bookmarks ADD COLUMN context_snippet TEXT;
        ),
```

#### Save path — inside `save_workspace`
```zed/crates/workspace/src/persistence.rs#L1496-1513
                conn.exec_bound(
                    sql!(
                        DELETE FROM bookmarks WHERE workspace_id = ?1;
                    )
                )?(workspace.id).context("Clearing old bookmarks")?;

                for (path, bookmarks) in workspace.bookmarks {
                    for bookmark in bookmarks {
                        let symbol_path_json = bookmark
                            .symbol_path
                            .as_ref()
                            .and_then(|sp| serde_json::to_string(sp).ok());
                        let offset_in_symbol = bookmark.offset_in_symbol.map(|v| v as i32);
                        let context_snippet = bookmark.context_snippet.as_deref();
                        conn.exec_bound(sql!(
                            INSERT INTO bookmarks (workspace_id, path, row, symbol_path, offset_in_symbol, context_snippet)
                            VALUES (?1, ?2, ?3, ?4, ?5, ?6);
                        ))?((workspace.id, path.as_ref(), bookmark.row, symbol_path_json, offset_in_symbol, context_snippet)).context("Inserting bookmark")?;
                    }
                }
```

#### Load path — the `bookmarks()` query method (lines 1311–1355 shown earlier)

#### `SerializedWorkspace` includes bookmarks:
```zed/crates/workspace/src/persistence/model.rs#L81-94
pub(crate) struct SerializedWorkspace {
    pub(crate) id: WorkspaceId,
    pub(crate) location: SerializedWorkspaceLocation,
    pub(crate) paths: PathList,
    pub(crate) center_group: SerializedPaneGroup,
    pub(crate) window_bounds: Option<SerializedWindowBounds>,
    pub(crate) centered_layout: bool,
    pub(crate) display: Option<Uuid>,
    pub(crate) docks: DockStructure,
    pub(crate) session_id: Option<String>,
    pub(crate) bookmarks: BTreeMap<Arc<Path>, Vec<SerializedBookmark>>,
    pub(crate) breakpoints: BTreeMap<Arc<Path>, Vec<SourceBreakpoint>>,
    pub(crate) user_toolchains: BTreeMap<ToolchainScope, IndexSet<Toolchain>>,
    pub(crate) window_id: Option<u64>,
}
```

---

### 6. Existing "relocate" / "reanchor" logic

**There is no explicit relocate/reanchor logic for handling in-session edits.** A `grep` for `relocate` and `reanchor` across all bookmark files returns zero results.

The two mechanisms that adjust bookmark positions are:

1. **CRDT anchors handle edits automatically.** `BookmarkAnchor` wraps `text::Anchor`, which is position-tracked by the rope data structure. As edits happen, the anchor's resolved row shifts accordingly. No bookmark-level code is needed.

2. **Syntactic re-resolution on reparse** (`resolve_pending_syntactic_bookmarks` at line 580). This is the closest thing to "reanchoring" — when bookmarks are restored from disk and tree-sitter hasn't parsed yet, they're placed at fallback row positions. Once `BufferEvent::Reparsed` fires, `resolve_pending_syntactic_bookmarks` re-resolves them using the now-available outline, finding the matching symbol and replacing the fallback anchor with a syntactically-correct one. This only happens once per bookmark during restoration, not on every edit.

3. **File rename handling** (`handle_file_changed` at line 530). When a buffer's file path changes, the bookmark entry is moved from the old path key to the new one in the `BTreeMap`. When a file is deleted, its bookmarks are removed entirely.

## Assistant

Now let me look at how bookmark positions are accessed during rendering and navigation to understand the full picture.

**Tool Call: Explore bookmark rendering and navigation**
Status: Completed

Now I have all the pieces. Here's the comprehensive answer:

---

## 1. How Bookmarks Are Rendered in the Gutter

### Step 1: During `EditorElement::prepaint`, visible bookmark rows are collected

```zed/crates/editor/src/element.rs#L10167-10169
                    let mut bookmark_rows = self.editor.update(cx, |editor, cx| {
                        editor.active_bookmarks(start_row..end_row, window, cx)
                    });
```

### Step 2: Rows with breakpoints are filtered out so icons don't overlap

```zed/crates/editor/src/element.rs#L10219-10220
                    // Don't render bookmarks on lines that already have breakpoints or phantom breakpoints
                    bookmark_rows.retain(|row| !breakpoint_rows.contains_key(row));
```

### Step 3: `layout_bookmarks` is called to produce the gutter elements

```zed/crates/editor/src/element.rs#L10828-10839
                        self.layout_bookmarks(
                            line_height,
                            start_row..end_row,
                            scroll_position,
                            &gutter_dimensions,
                            &gutter_hitbox,
                            &snapshot,
                            bookmark_rows,
                            &row_infos,
                            window,
                            cx,
                        )
```

### Step 4: `Editor::active_bookmarks` — the bridge from store → display rows

This method calls `bookmarks_for_buffer` for each visible excerpt, then converts each `BookmarkAnchor` into a `DisplayRow`:

```zed/crates/editor/src/editor.rs#L8906-8960
    fn active_bookmarks(
        &self,
        range: Range<DisplayRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashSet<DisplayRow> {
        let mut bookmark_display_points = HashSet::default();

        let Some(bookmark_store) = self.bookmark_store.clone() else {
            return bookmark_display_points;
        };

        let snapshot = self.snapshot(window, cx);

        let multi_buffer_snapshot = snapshot.buffer_snapshot();
        let Some(project) = self.project() else {
            return bookmark_display_points;
        };

        let range = snapshot.display_point_to_point(DisplayPoint::new(range.start, 0), Bias::Left)
            ..snapshot.display_point_to_point(DisplayPoint::new(range.end, 0), Bias::Right);

        for (buffer_snapshot, range, excerpt_id) in
            multi_buffer_snapshot.range_to_buffer_ranges(range.start..=range.end)
        {
            let Some(buffer) = project
                .read(cx)
                .buffer_for_id(buffer_snapshot.remote_id(), cx)
            else {
                continue;
            };
            let bookmarks = bookmark_store.update(cx, |store, cx| {
                store.bookmarks_for_buffer(
                    buffer,
                    Some(
                        buffer_snapshot.anchor_before(range.start)
                            ..buffer_snapshot.anchor_after(range.end),
                    ),
                    buffer_snapshot,
                    cx,
                )
            });
            for bookmark in bookmarks {
                let multi_buffer_anchor = Anchor::in_buffer(excerpt_id, bookmark.anchor());
                let position = multi_buffer_anchor
                    .to_point(&multi_buffer_snapshot)
                    .to_display_point(&snapshot);

                bookmark_display_points.insert(position.row());
            }
        }

        bookmark_display_points
    }
```

The key conversion chain for each `BookmarkAnchor` is:
1. `bookmark.anchor()` — unwraps the inner `text::Anchor`
2. `Anchor::in_buffer(excerpt_id, anchor)` — wraps it as a multi-buffer `Anchor`
3. `.to_point(&multi_buffer_snapshot)` — resolves the anchor to a `Point` (row, col)
4. `.to_display_point(&snapshot)` — converts to `DisplayPoint` (accounts for folds, wraps)
5. `.row()` → `DisplayRow` — inserted into the `HashSet`

### Step 5: `layout_bookmarks` positions and renders each icon

```zed/crates/editor/src/element.rs#L3131-3195
    fn layout_bookmarks(
        &self,
        line_height: Pixels,
        range: Range<DisplayRow>,
        scroll_position: gpui::Point<ScrollOffset>,
        gutter_dimensions: &GutterDimensions,
        gutter_hitbox: &Hitbox,
        snapshot: &EditorSnapshot,
        bookmarks: HashSet<DisplayRow>,
        row_infos: &[RowInfo],
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        if self.split_side == Some(SplitSide::Left) {
            return Vec::new();
        }

        self.editor.update(cx, |editor, cx| {
            bookmarks
                .into_iter()
                .filter_map(|display_row| {
                    if row_infos
                        .get((display_row.0.saturating_sub(range.start.0)) as usize)
                        .is_some_and(|row_info| {
                            row_info.expand_info.is_some()
                                || row_info
                                    .diff_status
                                    .is_some_and(|status| status.is_deleted())
                        })
                    {
                        return None;
                    }

                    if range.start > display_row || range.end < display_row {
                        return None;
                    }

                    let row =
                        MultiBufferRow(DisplayPoint::new(display_row, 0).to_point(snapshot).row);
                    if snapshot.is_line_folded(row) {
                        return None;
                    }

                    let icon = editor.render_bookmark();

                    let icon = prepaint_gutter_button(
                        icon.into_any_element(),
                        display_row,
                        line_height,
                        gutter_dimensions,
                        scroll_position,
                        gutter_hitbox,
                        window,
                        cx,
                    );
                    Some(icon)
                })
                .collect_vec()
        })
    }
```

Each bookmark is rendered as a blue `IconName::Bookmark` icon via `render_bookmark`:

```zed/crates/editor/src/editor.rs#L8962-8966
    fn render_bookmark(&self) -> Icon {
        Icon::new(IconName::Bookmark)
            .size(IconSize::XSmall)
            .color(Color::Info)
    }
```

---

## 2. How "Go To Next/Previous Bookmark" Works

### The action types (defined in `actions.rs`)

```zed/crates/editor/src/actions.rs#L565-573
        /// Goes to the next bookmark in the file.
        GoToNextBookmark,
        /// Goes to the next change in the file.
        GoToNextChange,
        GoToParentModule,
        /// Goes to the previous bookmark in the file.
        GoToPreviousBookmark,
```

### The public handlers delegate to `go_to_bookmark_impl`

```zed/crates/editor/src/editor.rs#L12243-12260
    pub fn go_to_next_bookmark(
        &mut self,
        _: &crate::actions::GoToNextBookmark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_bookmark_impl(Direction::Next, window, cx);
    }

    pub fn go_to_previous_bookmark(
        &mut self,
        _: &crate::actions::GoToPreviousBookmark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_bookmark_impl(Direction::Prev, window, cx);
    }
```

### The core navigation logic — `go_to_bookmark_impl`

This splits the multi-buffer into two ranges around the cursor, collects and sorts bookmarks in each half, then picks the first one in the desired direction (wrapping around):

```zed/crates/editor/src/editor.rs#L12262-12342
    fn go_to_bookmark_impl(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = &self.project else {
            return;
        };
        let Some(bookmark_store) = &self.bookmark_store else {
            return;
        };

        let selection = self
            .selections
            .newest::<MultiBufferOffset>(&self.display_snapshot(cx));
        let multi_buffer_snapshot = self.buffer.read(cx).snapshot(cx);

        let bookmarks_in_range =
            |range: Range<MultiBufferOffset>, cx: &mut Context<Self>| -> Vec<Anchor> {
                multi_buffer_snapshot
                    .range_to_buffer_ranges(range)
                    .into_iter()
                    .flat_map(|(buffer_snapshot, buffer_range, excerpt_id)| {
                        let Some(buffer) = project
                            .read(cx)
                            .buffer_for_id(buffer_snapshot.remote_id(), cx)
                        else {
                            return Vec::new();
                        };
                        bookmark_store
                            .update(cx, |store, cx| {
                                store.bookmarks_for_buffer(
                                    buffer,
                                    Some(
                                        buffer_snapshot.anchor_before(buffer_range.start)
                                            ..buffer_snapshot.anchor_after(buffer_range.end),
                                    ),
                                    buffer_snapshot,
                                    cx,
                                )
                            })
                            .into_iter()
                            .map(|bookmark| Anchor::in_buffer(excerpt_id, bookmark.anchor()))
                            .collect::<Vec<_>>()
                    })
                    .collect()
            };

        let mut before = bookmarks_in_range(MultiBufferOffset(0)..selection.head(), cx);
        let mut after = bookmarks_in_range(selection.head()..multi_buffer_snapshot.len(), cx);
        before.sort_by_key(|a| a.to_offset(&multi_buffer_snapshot));
        after.sort_by_key(|a| a.to_offset(&multi_buffer_snapshot));

        let anchor = if direction == Direction::Next {
            after
                .into_iter()
                .chain(before)
                .find(|anchor| anchor.to_offset(&multi_buffer_snapshot) != selection.head())
        } else {
            [before, after]
                .into_iter()
                .flat_map(|bookmarks| bookmarks.into_iter().rev())
                .find(|anchor| anchor.to_offset(&multi_buffer_snapshot) != selection.head())
        };

        if let Some(anchor) = anchor {
            self.unfold_ranges(&[anchor..anchor], true, false, cx);
            self.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |s| {
                    s.select_anchor_ranges([anchor..anchor]);
                },
            );
        }
    }
```

The algorithm:
- **Next**: tries `after` first (bookmarks ahead of cursor), then wraps into `before`; skips any bookmark at the exact cursor offset.
- **Prev**: iterates `before` reversed, then `after` reversed; skips the cursor position.
- When found, it **unfolds** the target range and moves the selection there with `Autoscroll::center()`.

---

## 3. How the Bookmark Store Emits Changes / Notifications

### `BookmarkStore` calls `cx.notify()` in 7 places

Every mutation path on `BookmarkStore` calls `cx.notify()`:

```zed/crates/project/src/bookmark_store.rs#L133
        cx.notify();
```
(after `with_serialized_bookmarks`)

```zed/crates/project/src/bookmark_store.rs#L382
                cx.notify();
```
(after `resolve_all`)

```zed/crates/project/src/bookmark_store.rs#L481
        cx.notify();
```
(after `toggle_bookmark`)

```zed/crates/project/src/bookmark_store.rs#L543
            cx.notify();
```
(in `handle_file_changed` — bookmark deleted because file deleted)

```zed/crates/project/src/bookmark_store.rs#L572
                cx.notify();
```
(in `handle_file_changed` — bookmarks moved after file rename)

```zed/crates/project/src/bookmark_store.rs#L643
        cx.notify();
```
(after `resolve_pending_syntactic_bookmarks`)

```zed/crates/project/src/bookmark_store.rs#L717
        cx.notify();
```
(after `clear_bookmarks`)

### Who observes these notifications?

There is **no explicit `cx.observe(&bookmark_store, ...)`** anywhere in the codebase. Instead, the mechanism is **GPUI's automatic invalidation**: when `bookmark_store.update(cx, |store, cx| { ... })` is called during `Editor::active_bookmarks` (which runs inside `EditorElement::prepaint`), GPUI records that the `EditorElement` **read** the `BookmarkStore` entity via `.update()`. When the `BookmarkStore` later calls `cx.notify()`, GPUI knows the editor view needs to be repainted.

Additionally, `toggle_bookmark` itself lives on the `Editor` and ends with `cx.notify()` on the editor:

```zed/crates/editor/src/editor.rs#L12240-12241
        cx.notify();
    }
```

So the editor is explicitly invalidated after every toggle, and implicitly tracked for any other bookmark-store mutation via GPUI's dependency tracking during rendering.

### The `BookmarkStore` also subscribes to `BufferEvent`s per-buffer

Each `BufferBookmarks` subscribes to its buffer for two events:

```zed/crates/project/src/bookmark_store.rs#L55-67
    pub fn new(buffer: Entity<Buffer>, cx: &mut Context<BookmarkStore>) -> Self {
        let subscription = cx.subscribe(
            &buffer,
            |bookmark_store, buffer, event: &BufferEvent, cx| match event {
                BufferEvent::FileHandleChanged => {
                    bookmark_store.handle_file_changed(buffer, cx);
                }
                BufferEvent::Reparsed => {
                    bookmark_store.resolve_pending_syntactic_bookmarks(buffer, cx);
                }
                _ => {}
            },
        );

        Self {
            buffer,
            bookmarks: Vec::new(),
            pending_syntactic: Vec::new(),
            _subscription: subscription,
        }
    }
```

- **`BufferEvent::FileHandleChanged`** → updates bookmarks when a file is renamed/deleted.
- **`BufferEvent::Reparsed`** → resolves pending syntactic bookmarks once the tree-sitter parse is available.

Note: `BufferEvent::Edited` is **not** handled by the bookmark store. Bookmark anchors survive edits automatically because they are `text::Anchor`s — the text CRDT adjusts their positions as the buffer content changes.

---

## 4. How `BufferEvent::Edited` Is Defined

### The full enum

```zed/crates/language/src/buffer.rs#L354-385
pub enum BufferEvent {
    /// The buffer was changed in a way that must be
    /// propagated to its other replicas.
    Operation {
        operation: Operation,
        is_local: bool,
    },
    /// The buffer was edited.
    Edited { is_local: bool },
    /// The buffer's `dirty` bit changed.
    DirtyChanged,
    /// The buffer was saved.
    Saved,
    /// The buffer's file was changed on disk.
    FileHandleChanged,
    /// The buffer was reloaded.
    Reloaded,
    /// The buffer is in need of a reload
    ReloadNeeded,
    /// The buffer's language was changed.
    /// The boolean indicates whether this buffer did not have a language before, but does now.
    LanguageChanged(bool),
    /// The buffer's syntax trees were updated.
    Reparsed,
    /// The buffer's diagnostics were updated.
    DiagnosticsUpdated,
    /// The buffer gained or lost editing capabilities.
    CapabilityChanged,
}
```

`BufferEvent::Edited` carries **a single field**: `is_local: bool`, which indicates whether the edit originated locally or from a remote collaborator.

### Where it's emitted

It's emitted in `Buffer::did_edit`, which is called after every edit transaction:

```zed/crates/language/src/buffer.rs#L2857-2886
    fn did_edit(
        &mut self,
        old_version: &clock::Global,
        was_dirty: bool,
        is_local: bool,
        cx: &mut Context<Self>,
    ) {
        self.was_changed();

        if self.edits_since::<usize>(old_version).next().is_none() {
            return;
        }

        self.reparse(cx, true);
        cx.emit(BufferEvent::Edited { is_local });
        let is_dirty = self.is_dirty();
        if was_dirty != is_dirty {
            cx.emit(BufferEvent::DirtyChanged);
        }
        if was_dirty && !is_dirty {
            if let Some(file) = self.file.as_ref() {
                if matches!(file.disk_state(), DiskState::Present { .. })
                    && file.disk_state().mtime() != self.saved_mtime
                {
                    cx.emit(BufferEvent::ReloadNeeded);
                }
            }
        }
        cx.notify();
    }
```

It first checks whether any actual edits occurred (via `edits_since`), triggers a reparse, then emits `Edited`. No edit details (ranges, text, etc.) are carried in the event itself — consumers are expected to diff the buffer's version clock or use `edits_since` to discover what changed.

---

## Summary

| Concern | Mechanism |
|---|---|
| **Gutter rendering** | `EditorElement::prepaint` → `Editor::active_bookmarks` → `BookmarkStore::bookmarks_for_buffer` → `BookmarkAnchor::anchor()` → `Anchor::in_buffer` → `.to_point()` → `.to_display_point()` → `DisplayRow` → `layout_bookmarks` renders `IconName::Bookmark` |
| **Navigation** | `GoToNextBookmark` / `GoToPreviousBookmark` → `go_to_bookmark_impl` splits buffer into before/after cursor, collects sorted bookmarks, picks the first in the desired direction (wrapping), then `unfold_ranges` + `change_selections` with `Autoscroll::center()` |
| **Change notifications** | `BookmarkStore` calls `cx.notify()` on every mutation (7 call sites). No explicit observer exists; GPUI's automatic dependency tracking during `prepaint` (the `.update()` call on the store) causes the editor to repaint when the store is invalidated |
| **`BufferEvent::Edited`** | Carries only `{ is_local: bool }`. Emitted from `Buffer::did_edit` after verifying actual edits occurred. The bookmark store does **not** listen to `Edited` — anchors track edits automatically via the CRDT |

**Tool Call: Explore buffer edit events and text snapshot diff capabilities**
Status: Completed

Now I have everything. Let me compile a comprehensive answer.

---

Here's a complete walkthrough of the buffer edit event system in Zed:

---

## 1. `BufferEvent` enum — all variants

```zed/crates/language/src/buffer.rs#L354-L383
pub enum BufferEvent {
    /// The buffer was changed in a way that must be
    /// propagated to its other replicas.
    Operation {
        operation: Operation,
        is_local: bool,
    },
    /// The buffer was edited.
    Edited { is_local: bool },
    /// The buffer's `dirty` bit changed.
    DirtyChanged,
    /// The buffer was saved.
    Saved,
    /// The buffer's file was changed on disk.
    FileHandleChanged,
    /// The buffer was reloaded.
    Reloaded,
    /// The buffer is in need of a reload
    ReloadNeeded,
    /// The buffer's language was changed.
    /// The boolean indicates whether this buffer did not have a language before, but does now.
    LanguageChanged(bool),
    /// The buffer's syntax trees were updated.
    Reparsed,
    /// The buffer's diagnostics were updated.
    DiagnosticsUpdated,
    /// The buffer gained or lost editing capabilities.
    CapabilityChanged,
}
```

The key variant for observing text changes is **`Edited { is_local: bool }`**. It's emitted from `did_edit` after the buffer confirms actual edits exist:

```zed/crates/language/src/buffer.rs#L2857-L2886
    fn did_edit(
        &mut self,
        old_version: &clock::Global,
        was_dirty: bool,
        is_local: bool,
        cx: &mut Context<Self>,
    ) {
        self.was_changed();

        if self.edits_since::<usize>(old_version).next().is_none() {
            return;
        }

        self.reparse(cx, true);
        cx.emit(BufferEvent::Edited { is_local });
        let is_dirty = self.is_dirty();
        if was_dirty != is_dirty {
            cx.emit(BufferEvent::DirtyChanged);
        }
        if was_dirty && !is_dirty {
            if let Some(file) = self.file.as_ref() {
                if matches!(file.disk_state(), DiskState::Present { .. })
                    && file.disk_state().mtime() != self.saved_mtime
                {
                    cx.emit(BufferEvent::ReloadNeeded);
                }
            }
        }
        cx.notify();
    }
```

---

## 2. `edits_since` / `edits_since_in_range` — computing diffs between versions

These live on `BufferSnapshot` in the text crate:

```zed/crates/text/src/text.rs#L2635-L2666
    pub fn edits_since<'a, D>(
        &'a self,
        since: &'a clock::Global,
    ) -> impl 'a + Iterator<Item = Edit<D>>
    where
        D: TextDimension + Ord,
    {
        self.edits_since_in_range(since, Anchor::MIN..Anchor::MAX)
    }

    pub fn anchored_edits_since<'a, D>(
        &'a self,
        since: &'a clock::Global,
    ) -> impl 'a + Iterator<Item = (Edit<D>, Range<Anchor>)>
    where
        D: TextDimension + Ord,
    {
        self.anchored_edits_since_in_range(since, Anchor::MIN..Anchor::MAX)
    }

    pub fn edits_since_in_range<'a, D>(
        &'a self,
        since: &'a clock::Global,
        range: Range<Anchor>,
    ) -> impl 'a + Iterator<Item = Edit<D>>
    where
        D: TextDimension + Ord,
    {
        self.anchored_edits_since_in_range(since, range)
            .map(|item| item.0)
    }
```

The type parameter `D` can be `usize` (byte offsets), `Point` (row/column), `PointUtf16`, etc. — anything implementing `TextDimension + Ord`. The `since` parameter is a `clock::Global` (version vector) captured at a previous point in time.

There's also `anchored_edits_since_in_range` which returns `(Edit<D>, Range<Anchor>)` tuples — giving you both the dimension-based ranges and stable anchors.

---

## 3. The `Edit<D>` type — what fields does an edit have?

```zed/crates/text/src/text.rs#L526-L529
pub struct Edit<D> {
    pub old: Range<D>,
    pub new: Range<D>,
}
```

It has two fields:
- **`old: Range<D>`** — the range in the *old* buffer snapshot that was replaced.
- **`new: Range<D>`** — the corresponding range in the *new* buffer snapshot that replaced it.

And these convenience methods:

```zed/crates/text/src/text.rs#L530-L549
impl<D> Edit<D>
where
    D: PartialEq,
{
    pub fn is_empty(&self) -> bool {
        self.old.start == self.old.end && self.new.start == self.new.end
    }
}

impl<D, DDelta> Edit<D>
where
    D: Sub<D, Output = DDelta> + Copy,
{
    pub fn old_len(&self) -> DDelta {
        self.old.end - self.old.start
    }

    pub fn new_len(&self) -> DDelta {
        self.new.end - self.new.start
    }
}
```

So for example, with `Edit<usize>`:
- An insertion at offset 10 of 5 bytes: `old: 10..10`, `new: 10..15`
- A deletion of 3 bytes at offset 10: `old: 10..13`, `new: 10..10`
- A replacement of 3 bytes with 5 bytes: `old: 10..13`, `new: 10..15`

Note that the `Edit` doesn't carry the *text* — you need the old/new snapshots to read the actual content.

---

## 4. `text_for_range` — reading text content from a snapshot

On `text::BufferSnapshot`:

```zed/crates/text/src/text.rs#L2241-L2246
    pub fn text_for_range<T: ToOffset>(&self, range: Range<T>) -> Chunks<'_> {
        let start = range.start.to_offset(self);
        let end = range.end.to_offset(self);
        self.visible_text.chunks_in_range(start..end)
    }
```

It returns a `Chunks<'_>` iterator (from the `rope` crate):

```zed/crates/rope/src/rope.rs#L786-L791
pub struct Chunks<'a> {
    chunks: sum_tree::Cursor<'a, 'static, Chunk, usize>,
    range: Range<usize>,
    offset: usize,
    reversed: bool,
}
```

`Chunks` implements `Iterator<Item = &str>`, yielding string slices from the rope. You can collect them or iterate. The range parameter `T` accepts anything implementing `ToOffset` — `usize`, `Point`, `Anchor`, etc.

There's also a `MultiBufferSnapshot` version:

```zed/crates/multi_buffer/src/multi_buffer.rs#L3998-L4000
    pub fn text_for_range<T: ToOffset>(&self, range: Range<T>) -> impl Iterator<Item = &str> + '_ {
        self.chunks(range, false).map(|chunk| chunk.text)
    }
```

**Typical pattern** to get the new text of an edit:

```/dev/null/example.rs#L1-L5
// Given a new snapshot and an Edit<usize>:
let new_text: String = new_snapshot
    .text_for_range(edit.new.clone())
    .collect();
```

---

## 5. `version()` — capturing buffer version before/after edits

On `Buffer` (owned clone):

```zed/crates/text/src/text.rs#L828-L830
    pub fn version(&self) -> clock::Global {
        self.version.clone()
    }
```

On `BufferSnapshot` (borrowed reference):

```zed/crates/text/src/text.rs#L2209-L2211
    pub fn version(&self) -> &clock::Global {
        &self.version
    }
```

The `clock::Global` is a [version vector](https://en.wikipedia.org/wiki/Version_vector):

```zed/crates/clock/src/clock.rs#L70-L74
pub struct Global {
    // 4 is chosen as it is the biggest count that does not increase the size of the field itself.
    // Coincidentally, it also covers all the important non-collab replica ids.
    values: SmallVec<[u32; 4]>,
}
```

**Typical pattern** — save a version, then later diff against it:

```/dev/null/example.rs#L1-L6
// Before the edit:
let old_version = buffer.read(cx).version();

// ... edits happen ...

// After the edit, compute what changed:
let edits: Vec<Edit<usize>> = buffer.read(cx).edits_since(&old_version).collect();
```

---

## 6. `anchor_before` / `anchor_after` — creating stable position references

```zed/crates/text/src/text.rs#L2555-L2570
    /// Returns an anchor range for the given input position range that is anchored to the text before and after.
    pub fn anchor_range_between<T: ToOffset>(&self, position: Range<T>) -> Range<Anchor> {
        self.anchor_before(position.start)..self.anchor_after(position.end)
    }

    /// Returns an anchor for the given input position that is anchored to the text before the position.
    pub fn anchor_before<T: ToOffset>(&self, position: T) -> Anchor {
        self.anchor_at(position, Bias::Left)
    }

    /// Returns an anchor for the given input position that is anchored to the text after the position.
    pub fn anchor_after<T: ToOffset>(&self, position: T) -> Anchor {
        self.anchor_at(position, Bias::Right)
    }
```

These delegate to `anchor_at`, which resolves to `anchor_at_offset`:

```zed/crates/text/src/text.rs#L2575-L2610
    fn anchor_at_offset(&self, mut offset: usize, bias: Bias) -> Anchor {
        if bias == Bias::Left && offset == 0 {
            Anchor::min_for_buffer(self.remote_id)
        } else if bias == Bias::Right
            && ((!cfg!(debug_assertions) && offset >= self.len()) || offset == self.len())
        {
            Anchor::max_for_buffer(self.remote_id)
        } else {
            if !self
                .visible_text
                .assert_char_boundary::<{ cfg!(debug_assertions) }>(offset)
            {
                offset = match bias {
                    Bias::Left => self.visible_text.floor_char_boundary(offset),
                    Bias::Right => self.visible_text.ceil_char_boundary(offset),
                };
            }
            let (start, _, item) = self.fragments.find::<usize, _>(&None, &offset, bias);
            let Some(fragment) = item else {
                debug_panic!(
                    "Failed to find fragment at offset {} (len: {})",
                    offset,
                    self.len()
                );
                return Anchor::max_for_buffer(self.remote_id);
            };
            let overshoot = offset - start;
            Anchor::new(
                fragment.timestamp,
                fragment.insertion_offset + overshoot as u32,
                bias,
                Some(self.remote_id),
            )
        }
    }
```

The `Anchor` struct itself:

```zed/crates/text/src/anchor.rs#L8-L37
/// A timestamped position in a buffer.
#[doc(alias = "TextAnchor")]
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Anchor {
    // we store the replica id and sequence number of the timestamp inline
    // to avoid the alignment of our fields from increasing the size of this struct
    pub(crate) timestamp_replica_id: clock::ReplicaId,
    pub(crate) timestamp_value: clock::Seq,

    /// The byte offset into the text inserted in the operation
    /// at `timestamp`.
    pub offset: u32,
    /// Whether this anchor stays attached to the character *before* or *after*
    /// the offset.
    pub bias: Bias,
    pub buffer_id: Option<BufferId>,
}
```

Key differences:
- **`anchor_before`** → `Bias::Left` — if text is inserted exactly at this position, the anchor stays *before* the new text.
- **`anchor_after`** → `Bias::Right` — if text is inserted exactly at this position, the anchor moves *after* the new text.

The `anchor_range_between` convenience method creates a range anchored before the start and after the end — ideal for tracking a region that should "expand" when text is inserted at its boundaries.

---

## Summary: Putting it all together

A typical observer pattern for tracking buffer edits looks like:

```/dev/null/example_usage.rs#L1-L20
// 1. Save the version before changes
let old_version = buffer_snapshot.version().clone();

// 2. Listen for BufferEvent::Edited { .. }

// 3. When notified, get the new snapshot
let new_snapshot = buffer.read(cx).snapshot();

// 4. Compute edits since the saved version
for edit in new_snapshot.edits_since::<usize>(&old_version) {
    // edit.old = Range<usize> in the OLD snapshot
    // edit.new = Range<usize> in the NEW snapshot

    // 5. Read the new text that was inserted/replaced
    let new_text: String = new_snapshot.text_for_range(edit.new.clone()).collect();

    // 6. Create stable anchors if you need to track positions across future edits
    let anchor = new_snapshot.anchor_before(edit.new.start);
}
```

## Assistant

Now I have a comprehensive understanding of the entire system. Let me create the implementation plan.

---

# Implementation Plan: Content-Identity Fingerprinting for Bookmarks

## Overview

When bookmarked text is moved (via cut+paste, drag-and-drop, or refactoring), the bookmark currently stays at its original position because the CRDT anchor tracks the position in the buffer, not the content. This plan adds a fingerprint-based system that detects when a bookmark's surrounding content has moved and relocates the bookmark to follow it.

The approach is inspired by the existing fold fingerprint system in `crates/editor/src/editor.rs:3846-3868`, but adapted for live, intra-session use rather than just cross-session persistence.

---

## Phase 1: Add Fingerprint Storage to BookmarkAnchor

**Goal:** Each bookmark carries a content fingerprint that describes the text around it.

### Step 1.1: Define the `BookmarkFingerprint` struct

**File:** `crates/project/src/bookmark_store.rs`

Add a new struct near the top of the file (after `BookmarkAnchor`):

```/dev/null/bookmark_store.rs#L1-L15
/// Content fingerprint for a bookmark, used to detect and follow text movement.
/// Stores short text samples from the bookmarked line and surrounding context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkFingerprint {
    /// The first N characters of the bookmarked line (trimmed), for primary identification.
    /// This is the same content as `context_snippet` in SerializedBookmark.
    pub line_snippet: String,

    /// A few lines of surrounding context (e.g. 2 lines before + 2 lines after),
    /// concatenated and trimmed. Used to disambiguate when `line_snippet` matches
    /// multiple locations.
    pub context_lines: String,

    /// The row at which this fingerprint was last computed, for staleness detection.
    pub last_known_row: u32,
}
```

**Design notes:**
- `line_snippet` is the trimmed first 60 characters of the bookmarked line (longer than the 30 chars used by `context_snippet` in serialization, since we need better discrimination for live matching).
- `context_lines` captures ~2 lines before and ~2 lines after the bookmarked line, concatenated and trimmed. This provides disambiguation when multiple lines have the same content (e.g., closing braces, blank lines).
- `last_known_row` lets us do a fast "is the fingerprint still valid at the current position?" check before doing an expensive scan.

### Step 1.2: Add fingerprint to `BufferBookmarks`

**File:** `crates/project/src/bookmark_store.rs`

Change the `bookmarks` field in `BufferBookmarks` from `Vec<BookmarkAnchor>` to `Vec<(BookmarkAnchor, BookmarkFingerprint)>`, or better, introduce a wrapper:

```/dev/null/bookmark_store.rs#L1-L5
#[derive(Clone, Debug)]
pub struct TrackedBookmark {
    pub anchor: BookmarkAnchor,
    pub fingerprint: BookmarkFingerprint,
}
```

Update `BufferBookmarks`:

```/dev/null/bookmark_store.rs#L1-L6
pub struct BufferBookmarks {
    buffer: Entity<Buffer>,
    bookmarks: Vec<TrackedBookmark>,         // changed from Vec<BookmarkAnchor>
    pending_syntactic: Vec<SerializedBookmark>,
    _subscription: Subscription,
}
```

### Step 1.3: Compute fingerprints on bookmark creation

**File:** `crates/project/src/bookmark_store.rs`

Add a method to `BookmarkStore`:

```/dev/null/bookmark_store.rs#L1-L32
const FINGERPRINT_LINE_LEN: usize = 60;
const FINGERPRINT_CONTEXT_LINES: u32 = 2;

pub fn compute_fingerprint(
    snapshot: &BufferSnapshot,
    row: u32,
) -> BookmarkFingerprint {
    let max_row = snapshot.max_point().row;

    // Line snippet: first 60 trimmed chars of the bookmarked line
    let line_snippet = Self::compute_line_snippet(snapshot, row, FINGERPRINT_LINE_LEN)
        .unwrap_or_default();

    // Context: gather surrounding lines
    let context_start = row.saturating_sub(FINGERPRINT_CONTEXT_LINES);
    let context_end = (row + FINGERPRINT_CONTEXT_LINES).min(max_row);
    let mut context_parts = Vec::new();
    for r in context_start..=context_end {
        if r == row { continue; } // exclude the bookmarked line itself
        let line_start = Point::new(r, 0);
        let line_end = Point::new(r, snapshot.line_len(r));
        let text: String = snapshot.text_for_range(line_start..line_end).collect();
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            context_parts.push(trimmed);
        }
    }

    BookmarkFingerprint {
        line_snippet,
        context_lines: context_parts.join("\n"),
        last_known_row: row,
    }
}
```

### Step 1.4: Update `toggle_bookmark` to create fingerprints

**File:** `crates/project/src/bookmark_store.rs`, around line 445

When a bookmark is added in `toggle_bookmark`, compute and store the fingerprint alongside it. This means constructing a `TrackedBookmark` instead of a bare `BookmarkAnchor`.

### Step 1.5: Update all consumers of `BufferBookmarks::bookmarks`

Every call site that reads `buffer_bookmarks.bookmarks` (rendering in `active_bookmarks`, navigation in `go_to_bookmark_impl`, serialization in `all_serialized_bookmarks`, etc.) will need to adapt to the `TrackedBookmark` type. Most just need `.anchor` instead of the direct `BookmarkAnchor`. You can also keep the `bookmarks()` accessor returning just the anchors via a mapping iterator if you want to minimize churn.

---

## Phase 2: Subscribe to Buffer Edits

**Goal:** Detect when edits may have moved bookmarked content away from its anchor position.

### Step 2.1: Add version tracking per buffer

**File:** `crates/project/src/bookmark_store.rs`

Add a `last_version: clock::Global` field to `BufferBookmarks`:

```/dev/null/bookmark_store.rs#L1-L7
pub struct BufferBookmarks {
    buffer: Entity<Buffer>,
    bookmarks: Vec<TrackedBookmark>,
    pending_syntactic: Vec<SerializedBookmark>,
    last_version: clock::Global,             // NEW
    _subscription: Subscription,
}
```

Initialize it in `BufferBookmarks::new`:

```/dev/null/bookmark_store.rs#L1-L3
let snapshot = buffer.read(cx).text_snapshot();
let last_version = snapshot.version().clone();
```

### Step 2.2: Subscribe to `BufferEvent::Edited`

**File:** `crates/project/src/bookmark_store.rs`, in `BufferBookmarks::new` (around line 55)

Extend the existing subscription to also handle `Edited`:

```/dev/null/bookmark_store.rs#L1-L12
let subscription = cx.subscribe(
    &buffer,
    |bookmark_store, buffer, event: &BufferEvent, cx| match event {
        BufferEvent::FileHandleChanged => {
            bookmark_store.handle_file_changed(buffer, cx);
        }
        BufferEvent::Reparsed => {
            bookmark_store.resolve_pending_syntactic_bookmarks(buffer, cx);
        }
        BufferEvent::Edited { .. } => {
            bookmark_store.check_displaced_bookmarks(buffer, cx);
        }
        _ => {}
    },
);
```

### Step 2.3: Add displacement detection (debounced)

**File:** `crates/project/src/bookmark_store.rs`

Edits can be frequent (every keystroke), so the displacement check should be lightweight. The strategy:

1. On each `Edited` event, do a **quick validation** — resolve each bookmark's anchor to its current row, then check if the `line_snippet` still matches the text at that row.
2. If it matches → update `last_known_row`, done (fast path).
3. If it doesn't match → mark the bookmark as "displaced" and schedule a relocation scan.

Add a `displaced` flag to `TrackedBookmark`:

```/dev/null/bookmark_store.rs#L1-L7
#[derive(Clone, Debug)]
pub struct TrackedBookmark {
    pub anchor: BookmarkAnchor,
    pub fingerprint: BookmarkFingerprint,
    pub displaced: bool,                      // NEW
}
```

The `check_displaced_bookmarks` method:

```/dev/null/bookmark_store.rs#L1-L35
fn check_displaced_bookmarks(
    &mut self,
    buffer: Entity<Buffer>,
    cx: &mut Context<Self>,
) {
    let Some(abs_path) = Self::abs_path_from_buffer(&buffer, cx) else {
        return;
    };
    let Some(BookmarkEntry::Loaded(buffer_bookmarks)) = self.bookmarks.get_mut(&abs_path) else {
        return;
    };

    let snapshot = buffer.read(cx).snapshot();
    let max_row = snapshot.max_point().row;
    let mut any_displaced = false;

    for tracked in &mut buffer_bookmarks.bookmarks {
        let current_row = snapshot
            .summary_for_anchor::<Point>(&tracked.anchor.anchor())
            .row;

        // Quick check: does the line snippet still match at the anchor's current row?
        let current_snippet = Self::compute_line_snippet(&snapshot, current_row, FINGERPRINT_LINE_LEN);
        if current_snippet.as_deref() == Some(&tracked.fingerprint.line_snippet) {
            // Still in place — update the last known row
            tracked.fingerprint.last_known_row = current_row;
            tracked.displaced = false;
        } else {
            // Content mismatch — this bookmark's text has moved
            tracked.displaced = true;
            any_displaced = true;
        }
    }

    if any_displaced {
        self.relocate_displaced_bookmarks(&abs_path.clone(), cx);
    }
}
```

**Performance note:** The quick check is O(bookmarks × snippet_length) per edit. Since `line_snippet` is ≤60 chars and `compute_line_snippet` uses `text_for_range` which is O(log n) on the rope, this is very cheap even with many bookmarks. The expensive full-scan only triggers when a mismatch is detected.

---

## Phase 3: Relocate Displaced Bookmarks

**Goal:** When a bookmark's content is no longer at its anchor position, scan the buffer to find where it went and re-anchor the bookmark there.

### Step 3.1: Implement the relocation scan

**File:** `crates/project/src/bookmark_store.rs`

```/dev/null/bookmark_store.rs#L1-L80
fn relocate_displaced_bookmarks(
    &mut self,
    abs_path: &Arc<Path>,
    cx: &mut Context<Self>,
) {
    let Some(BookmarkEntry::Loaded(buffer_bookmarks)) = self.bookmarks.get_mut(abs_path) else {
        return;
    };

    let snapshot = buffer_bookmarks.buffer.read(cx).snapshot();
    let text_snapshot = buffer_bookmarks.buffer.read(cx).text_snapshot();
    let max_row = snapshot.max_point().row;

    for tracked in &mut buffer_bookmarks.bookmarks {
        if !tracked.displaced {
            continue;
        }

        let needle = &tracked.fingerprint.line_snippet;
        if needle.is_empty() {
            // Can't relocate without a fingerprint
            tracked.displaced = false;
            continue;
        }

        // Scan the buffer for all lines matching the snippet
        let mut candidates: Vec<(u32, u32)> = Vec::new(); // (row, score)

        for row in 0..=max_row {
            let line_snippet = Self::compute_line_snippet(&snapshot, row, FINGERPRINT_LINE_LEN);
            if line_snippet.as_deref() == Some(needle) {
                candidates.push((row, 0));
            }
        }

        match candidates.len() {
            0 => {
                // Content was deleted or modified beyond recognition — leave bookmark in place
                tracked.displaced = false;
            }
            1 => {
                // Unique match — relocate
                let new_row = candidates[0].0;
                let new_anchor = snapshot.anchor_after(Point::new(new_row, 0));
                tracked.anchor = BookmarkAnchor(new_anchor);
                tracked.fingerprint.last_known_row = new_row;
                tracked.fingerprint.context_lines =
                    Self::compute_context_around(&snapshot, new_row);
                tracked.displaced = false;
            }
            _ => {
                // Multiple matches — use context_lines to disambiguate
                let mut best_row = None;
                let mut best_score = 0;

                for (row, _) in &candidates {
                    let context = Self::compute_context_around(&snapshot, *row);
                    let score = Self::context_similarity(
                        &tracked.fingerprint.context_lines,
                        &context,
                    );
                    if score > best_score {
                        best_score = score;
                        best_row = Some(*row);
                    }
                }

                if let Some(new_row) = best_row {
                    let new_anchor = snapshot.anchor_after(Point::new(new_row, 0));
                    tracked.anchor = BookmarkAnchor(new_anchor);
                    tracked.fingerprint.last_known_row = new_row;
                    tracked.fingerprint.context_lines =
                        Self::compute_context_around(&snapshot, new_row);
                }
                tracked.displaced = false;
            }
        }
    }

    cx.notify();
}
```

### Step 3.2: Implement context similarity scoring

**File:** `crates/project/src/bookmark_store.rs`

A simple line-level similarity score for disambiguation:

```/dev/null/bookmark_store.rs#L1-L18
/// Computes a similarity score between two context strings.
/// Higher is better. Uses line-level overlap counting.
fn context_similarity(original: &str, candidate: &str) -> u32 {
    let original_lines: HashSet<&str> = original.lines().collect();
    let candidate_lines: HashSet<&str> = candidate.lines().collect();
    original_lines.intersection(&candidate_lines).count() as u32
}

fn compute_context_around(snapshot: &BufferSnapshot, row: u32) -> String {
    let max_row = snapshot.max_point().row;
    let start = row.saturating_sub(FINGERPRINT_CONTEXT_LINES);
    let end = (row + FINGERPRINT_CONTEXT_LINES).min(max_row);
    let mut parts = Vec::new();
    for r in start..=end {
        if r == row { continue; }
        let line_start = Point::new(r, 0);
        let line_end = Point::new(r, snapshot.line_len(r));
        let text: String = snapshot.text_for_range(line_start..line_end).collect();
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    parts.join("\n")
}
```

### Step 3.3: Optimize the full-buffer scan

The naive scan in Step 3.1 iterates every row. For large files, this could be expensive. Two optimizations:

**Optimization A: Use `contains_str_at` to avoid string allocation.** Instead of computing the full line snippet for every row, use `BufferSnapshot::contains_str_at` (already implemented at `crates/text/src/text.rs:2100`) to check if the needle appears at the beginning of each line (after skipping whitespace). This avoids allocating a `String` per row.

**Optimization B: Use edit ranges to narrow the search.** When an edit inserts text, the relocated content must be within the inserted range. Track the `edits_since` from the saved `last_version` to identify insertion ranges, and only scan those ranges for candidates (in addition to checking the old position). This dramatically reduces the search space.

Here's the optimized approach:

```/dev/null/bookmark_store.rs#L1-L30
fn relocate_displaced_bookmarks(
    &mut self,
    abs_path: &Arc<Path>,
    cx: &mut Context<Self>,
) {
    let Some(BookmarkEntry::Loaded(buffer_bookmarks)) = self.bookmarks.get_mut(abs_path) else {
        return;
    };

    let snapshot = buffer_bookmarks.buffer.read(cx).snapshot();
    let text_snapshot = buffer_bookmarks.buffer.read(cx).text_snapshot();

    // Get the ranges that were inserted since the last version we checked
    let edits: Vec<Edit<Point>> = text_snapshot
        .edits_since::<Point>(&buffer_bookmarks.last_version)
        .collect();

    // Build a set of "interesting" row ranges — the new ranges from edits
    let mut search_rows: Vec<Range<u32>> = edits
        .iter()
        .filter(|e| e.new.start != e.new.end) // only insertions/replacements
        .map(|e| e.new.start.row..e.new.end.row)
        .collect();

    // ... then for each displaced bookmark, search only within search_rows
    // falling back to a full scan if not found in the edit ranges
    // ...

    buffer_bookmarks.last_version = text_snapshot.version().clone();
}
```

---

## Phase 4: Handle Edge Cases

### Step 4.1: Copy+Paste (duplication, not movement)

When text is **copied** (not cut) and pasted, the original text is still at the original location. The fingerprint check will find the line snippet still matches at the anchor's current row → the bookmark stays put. This is the correct behavior — the bookmark stays with the original, not the copy.

No special handling needed.

### Step 4.2: Cut+Paste (true movement)

When text is **cut**, the bookmarked line disappears from the anchor's position. The CRDT anchor collapses to the deletion point. The fingerprint check finds a mismatch → marks displaced → relocation scan finds the text at the paste location → bookmark moves there.

This is the primary use case and works naturally with the fingerprint system.

### Step 4.3: In-place editing of the bookmarked line

If the user edits the bookmarked line itself (e.g., renames a function), the fingerprint will no longer match. The relocation scan won't find the old snippet anywhere. The bookmark stays in place (the anchor is still valid), and the fingerprint is **refreshed** to match the new content.

Add fingerprint refresh logic: after relocation fails (0 candidates), recompute the fingerprint at the anchor's current position:

```/dev/null/bookmark_store.rs#L1-L8
0 => {
    // Content was modified in-place — refresh the fingerprint
    let current_row = text_snapshot
        .summary_for_anchor::<Point>(&tracked.anchor.anchor())
        .row;
    tracked.fingerprint = Self::compute_fingerprint(&snapshot, current_row);
    tracked.displaced = false;
}
```

### Step 4.4: Bookmark on an empty or whitespace-only line

If a bookmark is on a line that has no useful content (e.g., a blank line between functions), the `line_snippet` will be empty. In this case, the bookmark cannot be fingerprint-tracked and should fall back to pure anchor behavior (no relocation). The `compute_fingerprint` method should handle this by returning an empty snippet, and `check_displaced_bookmarks` should skip the displacement check for bookmarks with empty snippets.

### Step 4.5: Multiple bookmarks in the same moved block

If a function has 3 bookmarks and the entire function is cut+pasted, all 3 will be displaced simultaneously. The relocation scan should handle them independently — each will find its matching line in the pasted text. Since the bookmarks have different `line_snippet` values (different lines of the function), they'll each find unique matches.

If two bookmarks are on identical lines, the `context_lines` disambiguation will differentiate them since they have different surrounding lines.

### Step 4.6: Debouncing / Performance

The `check_displaced_bookmarks` fires on every `BufferEvent::Edited`. For fast typing, most checks will hit the fast path (snippet still matches → no scan needed). However, to be safe, consider adding a debounce:

**Option A: Timer-based debounce.** Use `cx.spawn` with a short delay (e.g., 100ms) and cancel/replace the task on each edit. This is similar to how fold serialization uses `SERIALIZATION_THROTTLE_TIME`.

**Option B: Transaction-boundary check.** Only check after a transaction is finalized (i.e., after the 300ms grouping window). This is harder to hook into.

**Recommendation:** Start with Option A. Add a `relocate_task: Option<Task<()>>` field to `BufferBookmarks` that gets replaced on each edit:

```/dev/null/bookmark_store.rs#L1-L5
pub struct BufferBookmarks {
    // ... existing fields ...
    relocate_task: Option<Task<()>>,
}
```

In `check_displaced_bookmarks`, instead of running immediately, schedule via the task:

```/dev/null/bookmark_store.rs#L1-L12
fn check_displaced_bookmarks(
    &mut self,
    buffer: Entity<Buffer>,
    cx: &mut Context<Self>,
) {
    // ... fast validation of all bookmarks (cheap, runs immediately) ...

    if any_displaced {
        let abs_path = abs_path.clone();
        // Debounce: replace any pending relocation task
        buffer_bookmarks.relocate_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            this.update(cx, |this, cx| {
                this.relocate_displaced_bookmarks(&abs_path, cx);
            }).ok();
        }));
    }
}
```

---

## Phase 5: Update Serialization

### Step 5.1: Persist fingerprints alongside bookmarks

**File:** `crates/project/src/bookmark_store.rs` — `all_serialized_bookmarks` (around line 646)

When serializing, include the `line_snippet` in `SerializedBookmark.context_snippet`. Since `context_snippet` already stores the first 30 chars of the line, you could either:
- Extend it to 60 chars (matching `FINGERPRINT_LINE_LEN`), or
- Add a separate `context_lines` field to `SerializedBookmark` for the surrounding-context fingerprint.

**Recommendation:** Extend `SerializedBookmark` with a `context_lines` field:

```/dev/null/bookmark_store.rs#L1-L10
pub struct SerializedBookmark {
    pub row: u32,
    pub symbol_path: Option<Vec<String>>,
    pub offset_in_symbol: Option<u32>,
    pub context_snippet: Option<String>,
    pub context_lines: Option<String>,        // NEW: surrounding context for disambiguation
}
```

### Step 5.2: Database migration

**File:** `crates/workspace/src/persistence.rs`

Add a migration to the bookmarks table:

```/dev/null/persistence.rs#L1-L3
sql!(
    ALTER TABLE bookmarks ADD COLUMN context_lines TEXT;
),
```

### Step 5.3: Restore fingerprints on load

**File:** `crates/project/src/bookmark_store.rs` — `resolve_anchors_if_needed` (around line 137)

When resolving serialized bookmarks into `TrackedBookmark`s, compute a fresh `BookmarkFingerprint` from the resolved position. The serialized `context_snippet` and `context_lines` are used during resolution (to find the right position), and then a live fingerprint is computed from the resolved position for ongoing tracking.

---

## Phase 6: Testing

### Test 6.1: Basic cut+paste relocation

1. Create a buffer with a function at line 10.
2. Toggle a bookmark on line 12 (inside the function).
3. Cut lines 10-20 (the whole function).
4. Paste at line 50.
5. Verify the bookmark is now at the corresponding line in the pasted function (line 52).

### Test 6.2: Copy+paste does NOT move bookmark

1. Create a buffer with a bookmarked line.
2. Copy the line.
3. Paste elsewhere.
4. Verify the bookmark remains at the original location.

### Test 6.3: In-place edit refreshes fingerprint

1. Create a buffer, bookmark a line.
2. Edit the bookmarked line (change function name).
3. Verify the bookmark stays on that line.
4. Cut+paste the modified function elsewhere.
5. Verify the bookmark follows (using the refreshed fingerprint).

### Test 6.4: Multiple identical lines — context disambiguation

1. Create a buffer with repeated patterns (e.g., multiple `}` lines).
2. Bookmark one specific `}`.
3. Move the block containing that `}`.
4. Verify the bookmark follows the correct `}` based on context.

### Test 6.5: Fingerprint not found — graceful degradation

1. Bookmark a line.
2. Delete the line entirely (without pasting it anywhere).
3. Verify the bookmark stays at the deletion point (anchor behavior) and the fingerprint is refreshed.

---

## File Change Summary

| File | Changes |
|---|---|
| `crates/project/src/bookmark_store.rs` | Major: add `BookmarkFingerprint`, `TrackedBookmark`, fingerprint computation, edit subscription, displacement detection, relocation scan, context similarity, fingerprint refresh |
| `crates/editor/src/editor.rs` | Minor: update `active_bookmarks` and `go_to_bookmark_impl` to work with `TrackedBookmark` instead of bare `BookmarkAnchor` |
| `crates/editor/src/element.rs` | Minimal: no changes if `bookmarks()` accessor still returns anchors |
| `crates/workspace/src/persistence.rs` | Minor: add `context_lines` column migration, update Bookmark struct and bind/column impls |
| `crates/workspace/src/persistence/model.rs` | Minor: update `SerializedBookmark` usage if the model changes |

## Recommended Implementation Order

1. **Phase 1** (Steps 1.1–1.5): Get `TrackedBookmark` and fingerprints compiling. All existing tests should still pass since this is additive.
2. **Phase 2** (Steps 2.1–2.3): Wire up the edit subscription and displacement detection. At this point you can manually test cut+paste.
3. **Phase 3** (Steps 3.1–3.3): Implement the relocation scan. This is the core feature — cut+paste should now move bookmarks.
4. **Phase 4** (Steps 4.1–4.6): Handle edge cases and add debouncing.
5. **Phase 5** (Steps 5.1–5.3): Update serialization so fingerprints survive across sessions.
6. **Phase 6**: Write tests.
