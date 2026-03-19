use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    path::Path,
    sync::Arc,
};

use anyhow::Result;
use gpui::{App, Context, Entity, Subscription, Task};
use language::{Buffer, BufferEvent, OutlineItem};
use text::{Anchor, BufferSnapshot, Point, ToPoint};

use crate::{ProjectPath, buffer_store::BufferStore, worktree_store::WorktreeStore};

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct BookmarkAnchor(text::Anchor);

impl BookmarkAnchor {
    pub fn anchor(&self) -> text::Anchor {
        self.0
    }
}

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

pub struct BookmarkStore {
    buffer_store: Entity<BufferStore>,
    worktree_store: Entity<WorktreeStore>,
    bookmarks: BTreeMap<Arc<Path>, BookmarkEntry>,
}

impl BookmarkStore {
    pub fn new(worktree_store: Entity<WorktreeStore>, buffer_store: Entity<BufferStore>) -> Self {
        Self {
            buffer_store,
            worktree_store,
            bookmarks: BTreeMap::new(),
        }
    }

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

    /// Attempt to resolve a bookmark using its syntactic context.
    /// Returns the resolved row if a matching outline symbol is found.
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

    /// Check whether the ancestor chain of an outline item matches the given symbol path.
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

    /// Opens buffers for all unloaded bookmark entries and resolves them to anchors.
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

    pub fn abs_path_from_buffer(buffer: &Entity<Buffer>, cx: &App) -> Option<Arc<Path>> {
        worktree::File::from_dyn(buffer.read(cx).file())
            .map(|file| file.worktree.read(cx).absolutize(&file.path))
            .map(Arc::<Path>::from)
    }

    /// Compute the syntactic context for a bookmark at the given position.
    /// Returns the symbol path and offset within the innermost symbol.
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

    const CONTEXT_SNIPPET_MAX_LEN: usize = 30;

    /// Extract the first 30 characters of text at the given row, trimmed of
    /// leading/trailing whitespace. Used to disambiguate bookmarks in duplicate
    /// outline sections that share the same symbol path.
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

    /// Toggle a bookmark at the given anchor in the buffer.
    /// If a bookmark already exists on the same row, it will be removed.
    /// Otherwise, a new bookmark will be added.
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

    pub fn bookmarks(&self) -> &BTreeMap<Arc<Path>, BookmarkEntry> {
        &self.bookmarks
    }

    /// Returns the bookmarks for a given buffer within an optional range.
    /// Only returns bookmarks that have been resolved to anchors (loaded).
    /// Unloaded bookmarks for the given buffer will be resolved first.
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

    /// Called when a buffer finishes tree-sitter parsing. Re-resolves any
    /// bookmarks that were placed at fallback row positions because the outline
    /// was not yet available.
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

    pub fn clear_bookmarks(&mut self, cx: &mut Context<Self>) {
        self.bookmarks.clear();
        cx.notify();
    }
}
