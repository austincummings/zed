use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    ops::Range,
    path::Path,
    sync::Arc,
};

use anyhow::Result;
use futures::{StreamExt, TryFutureExt, TryStreamExt, stream::FuturesUnordered};
use gpui::{App, AppContext, Context, Entity, SharedString, Subscription, Task};
use itertools::Itertools;
use language::{Buffer, BufferEvent};
use std::collections::HashMap;
use text::{BufferSnapshot, Point};

use crate::{ProjectPath, buffer_store::BufferStore, worktree_store::WorktreeStore};

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub anchor: text::Anchor,
    pub label: String,
    pub syntactic_location: SyntacticLocation,
}

/// Number of lines above and below the bookmarked line hashed into
/// [`ContentMarker::context_hash`].
const CONTEXT_WINDOW: u32 = 2;

/// A durable, serializable description of where a bookmark should be placed,
/// re-resolved against a buffer's outline at open/reload time.
///
/// Unlike [`text::Anchor`], this does not track edits in a live buffer; it is
/// re-resolved from scratch. While a buffer is open the live anchor is the
/// source of truth, and this description is only consulted when reattaching a
/// bookmark to a freshly opened buffer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SyntacticLocation {
    /// `None` when there is no enclosing symbol — either the language has no
    /// parser/outline (e.g. plaintext), or the position sits between symbols.
    pub symbol: Option<SymbolRef>,
    pub content_marker: ContentMarker,
    /// Last known absolute row; the final fallback when nothing else matches.
    pub last_known_row: u32,
}

/// Identifies the symbol a bookmark is bound to, plus where inside it the
/// bookmark sits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SymbolRef {
    /// Dotted outline path of the enclosing symbol, innermost last, e.g.
    /// `"impl BookmarkStore › fn toggle_bookmark"`.
    pub symbol_path: SharedString,
    /// The nth occurrence (0-based) of an identical `symbol_path` in the file,
    /// used to disambiguate paths that collapse to the same text.
    pub symbol_ordinal: u32,
    /// Line offset from the symbol's start row to the bookmarked row.
    pub line_offset_in_symbol: u32,
}

/// A fingerprint of the bookmarked line and its surroundings, used to snap a
/// bookmark back to the exact line when the symbol-relative offset drifts.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentMarker {
    /// Trimmed, whitespace-normalized text of the bookmarked line. The primary
    /// matcher when snapping the offset to the exact line.
    pub line_text: SharedString,
    /// Hash of a normalized window of surrounding lines (±[`CONTEXT_WINDOW`]).
    /// A tiebreaker only, consulted when `line_text` is ambiguous within the
    /// search window.
    pub context_hash: u64,
}

/// Trims a line and collapses internal runs of whitespace into a single space,
/// so reindentation or trailing-whitespace cleanup doesn't invalidate a marker.
fn normalize_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line_text(snapshot: &BufferSnapshot, row: u32) -> String {
    let start = Point::new(row, 0);
    let end = Point::new(row, snapshot.line_len(row));
    snapshot.text_for_range(start..end).collect::<String>()
}

fn syntax_lookup_point(snapshot: &BufferSnapshot, row: u32) -> Point {
    let line = line_text(snapshot, row);
    let column = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(0);
    let column = u32::try_from(column).unwrap_or(snapshot.line_len(row));
    Point::new(row, column)
}

/// Computes the [`SyntacticLocation`] for `anchor` in `snapshot`. This is
/// derived fresh from the current buffer contents; callers store it alongside
/// the live anchor so it can be persisted and later re-resolved.
pub fn compute_syntactic_location(
    snapshot: &language::BufferSnapshot,
    anchor: text::Anchor,
) -> SyntacticLocation {
    let row = anchor.summary::<Point>(snapshot).row;

    let symbol = compute_symbol_ref(snapshot, row);
    let content_marker = compute_content_marker(snapshot, row);

    SyntacticLocation {
        symbol,
        content_marker,
        last_known_row: row,
    }
}

const SYMBOL_PATH_SEPARATOR: &str = " › ";

fn compute_symbol_ref(snapshot: &language::BufferSnapshot, row: u32) -> Option<SymbolRef> {
    let containing = snapshot.symbols_containing(syntax_lookup_point(snapshot, row), None);
    let innermost = containing.last()?;
    let symbol_path: SharedString = join_symbol_path(containing.iter().map(|item| &item.text));

    let symbol_start_row = innermost.range.start.summary::<Point>(snapshot).row;
    let line_offset_in_symbol = row.saturating_sub(symbol_start_row);

    // Count how many earlier symbols in the file collapse to the same full
    // path, so the resolver can pick the right one when a path isn't unique.
    let symbol_ordinal = outline_item_paths(snapshot)
        .into_iter()
        .take_while(|(item_start_row, _)| *item_start_row < symbol_start_row)
        .filter(|(_, path)| *path == symbol_path)
        .count() as u32;

    Some(SymbolRef {
        symbol_path,
        symbol_ordinal,
        line_offset_in_symbol,
    })
}

fn join_symbol_path<'a>(segments: impl Iterator<Item = &'a SharedString>) -> SharedString {
    segments
        .map(|text| text.as_ref())
        .collect::<Vec<_>>()
        .join(SYMBOL_PATH_SEPARATOR)
        .into()
}

/// Builds the full dotted path of every outline item in the file, paired with
/// its start row. Mirrors the depth-based path-stack construction used by
/// [`language::Outline::new`], so the paths line up with the one produced from
/// [`language::BufferSnapshot::symbols_containing`].
fn outline_item_paths(snapshot: &language::BufferSnapshot) -> Vec<(u32, SharedString)> {
    let items = snapshot.outline(None).items;
    let mut paths = Vec::with_capacity(items.len());
    let mut path_stack: Vec<SharedString> = Vec::new();
    for item in &items {
        path_stack.truncate(item.depth);
        path_stack.push(item.text.clone());
        let start_row = item.range.start.summary::<Point>(snapshot).row;
        paths.push((start_row, join_symbol_path(path_stack.iter())));
    }
    paths
}

fn compute_content_marker(snapshot: &BufferSnapshot, row: u32) -> ContentMarker {
    let normalized = normalize_line(&line_text(snapshot, row));

    let mut hasher = collections::FxHasher::default();
    let max_row = snapshot.max_point().row;
    let start = row.saturating_sub(CONTEXT_WINDOW);
    let end = (row + CONTEXT_WINDOW).min(max_row);
    for context_row in start..=end {
        normalize_line(&line_text(snapshot, context_row)).hash(&mut hasher);
        0xffu8.hash(&mut hasher);
    }

    ContentMarker {
        line_text: normalized.into(),
        context_hash: hasher.finish(),
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct SerializedBookmark {
    pub row: u32,
    pub label: String,
}

#[derive(Debug)]
pub struct BufferBookmarks {
    buffer: Entity<Buffer>,
    bookmarks: Vec<Bookmark>,
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
                _ => {}
            },
        );

        Self {
            buffer,
            bookmarks: Vec::new(),
            _subscription: subscription,
        }
    }

    pub fn buffer(&self) -> &Entity<Buffer> {
        &self.buffer
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
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

    fn loaded(&self) -> Option<&BufferBookmarks> {
        match self {
            BookmarkEntry::Loaded(buffer_bookmarks) => Some(buffer_bookmarks),
            BookmarkEntry::Unloaded(_) => None,
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

    pub fn load_serialized_bookmarks(
        &mut self,
        bookmark_rows: BTreeMap<Arc<Path>, Vec<SerializedBookmark>>,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.bookmarks.clear();

        for (path, rows) in bookmark_rows {
            if rows.is_empty() {
                continue;
            }

            let count = rows.len();
            log::debug!("Stored {count} unloaded bookmark(s) at {}", path.display());

            self.bookmarks.insert(path, BookmarkEntry::Unloaded(rows));
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
        let Some(BookmarkEntry::Unloaded(bookmarks)) = self.bookmarks.get(abs_path) else {
            return;
        };

        let snapshot = buffer.read(cx).snapshot();
        let max_point = snapshot.max_point();

        let bookmarks: Vec<Bookmark> = bookmarks
            .iter()
            .filter_map(|bookmark| {
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
                // Phase 1: still resolve by row; recompute the syntactic
                // location from the freshly opened buffer so the field is
                // populated. The resolution ladder that consumes it lands later.
                let syntactic_location = compute_syntactic_location(&snapshot, anchor);
                Some(Bookmark {
                    anchor,
                    label: bookmark.label.clone(),
                    syntactic_location,
                })
            })
            .collect();

        if bookmarks.is_empty() {
            self.bookmarks.remove(abs_path);
        } else {
            let mut buffer_bookmarks = BufferBookmarks::new(buffer.clone(), cx);
            buffer_bookmarks.bookmarks = bookmarks;
            self.bookmarks
                .insert(abs_path.clone(), BookmarkEntry::Loaded(buffer_bookmarks));
        }
    }

    pub fn abs_path_from_buffer(buffer: &Entity<Buffer>, cx: &App) -> Option<Arc<Path>> {
        worktree::File::from_dyn(buffer.read(cx).file())
            .map(|file| file.worktree.read(cx).absolutize(&file.path))
            .map(Arc::<Path>::from)
    }

    /// Toggle a bookmark at the given anchor in the buffer.
    /// If a bookmark already exists on the same row, it will be removed.
    /// Otherwise, a new bookmark will be added with the given label.
    pub fn toggle_bookmark(
        &mut self,
        buffer: Entity<Buffer>,
        anchor: text::Anchor,
        label: String,
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

        let snapshot = buffer.read(cx).text_snapshot();

        let existing_index = buffer_bookmarks.bookmarks.iter().position(|existing| {
            existing.anchor.summary::<Point>(&snapshot).row
                == anchor.summary::<Point>(&snapshot).row
        });

        if let Some(index) = existing_index {
            buffer_bookmarks.bookmarks.remove(index);
            if buffer_bookmarks.bookmarks.is_empty() {
                self.bookmarks.remove(&abs_path);
            }
        } else {
            let syntactic_location =
                compute_syntactic_location(&buffer.read(cx).snapshot(), anchor);
            log::debug!(
                "Computed syntactic location for bookmark at {} row {}: {syntactic_location:?}",
                abs_path.display(),
                anchor.summary::<Point>(&snapshot).row,
            );
            buffer_bookmarks.bookmarks.push(Bookmark {
                anchor,
                label,
                syntactic_location,
            });
        }

        cx.notify();
    }

    pub fn find_bookmark(
        &mut self,
        buffer: &Entity<Buffer>,
        anchor: text::Anchor,
        cx: &mut Context<Self>,
    ) -> Option<&Bookmark> {
        let Some(abs_path) = Self::abs_path_from_buffer(buffer, cx) else {
            return None;
        };

        self.resolve_anchors_if_needed(&abs_path, buffer, cx);

        let entry = self
            .bookmarks
            .entry(abs_path.clone())
            .or_insert_with(|| BookmarkEntry::Loaded(BufferBookmarks::new(buffer.clone(), cx)));

        let BookmarkEntry::Loaded(buffer_bookmarks) = entry else {
            unreachable!("resolve_if_needed should have converted to Loaded");
        };

        let snapshot = buffer.read(cx).text_snapshot();

        buffer_bookmarks.bookmarks.iter().find(|existing| {
            existing.anchor.summary::<Point>(&snapshot).row
                == anchor.summary::<Point>(&snapshot).row
        })
    }

    pub fn edit_bookmark(
        &mut self,
        buffer: &Entity<Buffer>,
        anchor: text::Anchor,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let Some(abs_path) = Self::abs_path_from_buffer(buffer, cx) else {
            return;
        };

        self.resolve_anchors_if_needed(&abs_path, buffer, cx);

        let Some(BookmarkEntry::Loaded(buffer_bookmarks)) = self.bookmarks.get_mut(&abs_path)
        else {
            return;
        };

        let snapshot = buffer.read(cx).text_snapshot();
        let row = anchor.summary::<Point>(&snapshot).row;

        if let Some(bookmark) = buffer_bookmarks
            .bookmarks
            .iter_mut()
            .find(|existing| existing.anchor.summary::<Point>(&snapshot).row == row)
        {
            bookmark.label = label;
            cx.notify();
        }
    }

    /// Returns the bookmarks for a given buffer within an optional range.
    /// Only returns bookmarks that have been resolved to anchors (loaded).
    /// Unloaded bookmarks for the given buffer will be resolved first.
    pub fn bookmarks_for_buffer(
        &mut self,
        buffer: Entity<Buffer>,
        range: Range<text::Anchor>,
        buffer_snapshot: &BufferSnapshot,
        cx: &mut Context<Self>,
    ) -> Vec<Bookmark> {
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
                    if !buffer_snapshot.can_resolve(&bookmark.anchor) {
                        return None;
                    }

                    if bookmark.anchor.cmp(&range.start, buffer_snapshot).is_lt()
                        || bookmark.anchor.cmp(&range.end, buffer_snapshot).is_gt()
                    {
                        return None;
                    }

                    Some(bookmark.clone())
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

    pub fn all_serialized_bookmarks(
        &self,
        cx: &App,
    ) -> BTreeMap<Arc<Path>, Vec<SerializedBookmark>> {
        self.bookmarks
            .iter()
            .filter_map(|(path, entry)| {
                let mut rows = match entry {
                    BookmarkEntry::Unloaded(rows) => rows.clone(),
                    BookmarkEntry::Loaded(buffer_bookmarks) => {
                        let snapshot = buffer_bookmarks.buffer.read(cx).snapshot();
                        buffer_bookmarks
                            .bookmarks
                            .iter()
                            .filter_map(|bookmark| {
                                if !snapshot.can_resolve(&bookmark.anchor) {
                                    return None;
                                }
                                let row =
                                    snapshot.summary_for_anchor::<Point>(&bookmark.anchor).row;
                                Some(SerializedBookmark {
                                    row,
                                    label: bookmark.label.clone(),
                                })
                            })
                            .collect()
                    }
                };

                rows.sort_unstable_by_key(|a| a.row);
                rows.dedup_by_key(|a| a.row);

                if rows.is_empty() {
                    None
                } else {
                    Some((path.clone(), rows))
                }
            })
            .collect()
    }

    pub async fn all_bookmark_locations(
        this: Entity<BookmarkStore>,
        cx: &mut (impl AppContext + Clone),
    ) -> Result<HashMap<Entity<Buffer>, Vec<Range<Point>>>> {
        Self::resolve_all(&this, cx).await?;

        cx.read_entity(&this, |this, cx| {
            let mut locations: HashMap<_, Vec<_>> = HashMap::new();
            for bookmarks in this.bookmarks.values().filter_map(BookmarkEntry::loaded) {
                let snapshot = cx.read_entity(bookmarks.buffer(), |b, _| b.snapshot());
                let ranges: Vec<Range<Point>> = bookmarks
                    .bookmarks()
                    .iter()
                    .map(|bookmark| {
                        let row = snapshot.summary_for_anchor::<Point>(&bookmark.anchor).row;
                        Point::row_range(row..row)
                    })
                    .collect();

                locations
                    .entry(bookmarks.buffer().clone())
                    .or_default()
                    .extend(ranges);
            }

            Ok(locations)
        })
    }

    /// Opens buffers for all unloaded bookmark entries and resolves them to anchors. This is used to show all bookmarks in a large multi-buffer.
    async fn resolve_all(this: &Entity<Self>, cx: &mut (impl AppContext + Clone)) -> Result<()> {
        let unloaded_paths: Vec<Arc<Path>> = cx.read_entity(&this, |this, _| {
            this.bookmarks
                .iter()
                .filter_map(|(path, entry)| match entry {
                    BookmarkEntry::Unloaded(_) => Some(path.clone()),
                    BookmarkEntry::Loaded(_) => None,
                })
                .collect_vec()
        });

        if unloaded_paths.is_empty() {
            return Ok(());
        }

        let worktree_store = cx.read_entity(&this, |this, _| this.worktree_store.clone());
        let buffer_store = cx.read_entity(&this, |this, _| this.buffer_store.clone());

        let open_tasks: FuturesUnordered<_> = unloaded_paths
            .iter()
            .map(|path| {
                open_path(path, &worktree_store, &buffer_store, cx.clone())
                    .map_err(move |e| (path, e))
                    .map_ok(move |b| (path, b))
            })
            .collect();

        let opened: Vec<_> = open_tasks
            .inspect_err(|(path, error)| {
                log::warn!(
                    "Could not open buffer for bookmarked path {}: {error}",
                    path.display()
                )
            })
            .filter_map(|res| async move { res.ok() })
            .collect()
            .await;

        cx.update_entity(&this, |this, cx| {
            for (path, buffer) in opened {
                this.resolve_anchors_if_needed(&path, &buffer, cx);
            }
            cx.notify();
        });

        Ok(())
    }

    pub fn clear_bookmarks(&mut self, cx: &mut Context<Self>) {
        self.bookmarks.clear();
        cx.notify();
    }
}

async fn open_path(
    path: &Path,
    worktree_store: &Entity<WorktreeStore>,
    buffer_store: &Entity<BufferStore>,
    mut cx: impl AppContext,
) -> Result<Entity<Buffer>> {
    let (worktree, worktree_path) = cx
        .update_entity(&worktree_store, |worktree_store, cx| {
            worktree_store.find_or_create_worktree(path, false, cx)
        })
        .await?;

    let project_path = ProjectPath {
        worktree_id: cx.read_entity(&worktree, |worktree, _| worktree.id()),
        path: worktree_path,
    };

    let buffer = cx
        .update_entity(&buffer_store, |buffer_store, cx| {
            buffer_store.open_buffer(project_path, cx)
        })
        .await?;

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use language::{Language, LanguageConfig, rust_lang};

    fn syntactic_location_at_row(
        text: &str,
        row: u32,
        with_rust_language: bool,
        cx: &mut TestAppContext,
    ) -> SyntacticLocation {
        let buffer = cx.new(|cx| {
            let buffer = Buffer::local(text, cx);
            if with_rust_language {
                buffer.with_language(rust_lang(), cx)
            } else {
                buffer
            }
        });
        cx.update(|cx| {
            let snapshot = buffer.read(cx).snapshot();
            let anchor = snapshot.anchor_after(Point::new(row, 0));
            compute_syntactic_location(&snapshot, anchor)
        })
    }

    fn typescript_lang() -> Arc<Language> {
        Arc::new(
            Language::new(
                LanguageConfig {
                    name: "TypeScript".into(),
                    ..Default::default()
                },
                Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            )
            .with_outline_query(include_str!("../../grammars/src/typescript/outline.scm"))
            .expect("valid TypeScript outline query"),
        )
    }

    fn typescript_syntactic_location_at_row(
        text: &str,
        row: u32,
        cx: &mut TestAppContext,
    ) -> SyntacticLocation {
        let language = typescript_lang();
        let buffer = cx.new(|cx| Buffer::local(text, cx).with_language(language, cx));
        cx.update(|cx| {
            let snapshot = buffer.read(cx).snapshot();
            let anchor = snapshot.anchor_after(Point::new(row, 0));
            compute_syntactic_location(&snapshot, anchor)
        })
    }

    #[gpui::test]
    fn test_syntactic_location_inside_rust_function(cx: &mut TestAppContext) {
        let location = syntactic_location_at_row(
            "fn bookmarked() {\n    let value = 1;\n    dbg!(value);\n}\n",
            2,
            true,
            cx,
        );

        assert_eq!(
            location.symbol,
            Some(SymbolRef {
                symbol_path: "fn bookmarked".into(),
                symbol_ordinal: 0,
                line_offset_in_symbol: 2,
            })
        );
        assert_eq!(location.content_marker.line_text, "dbg!(value);");
        assert_eq!(location.last_known_row, 2);
    }

    #[gpui::test]
    fn test_syntactic_location_uses_nested_symbol_path(cx: &mut TestAppContext) {
        let location = syntactic_location_at_row(
            "struct Store;\n\nimpl Store {\n    fn bookmarked(&self) {\n        let value = 1;\n    }\n}\n",
            3,
            true,
            cx,
        );

        let symbol = location.symbol.expect("expected an enclosing Rust symbol");
        assert_eq!(symbol.symbol_path, "impl Store › fn bookmarked");
        assert_eq!(symbol.symbol_ordinal, 0);
        assert_eq!(symbol.line_offset_in_symbol, 0);
    }

    #[gpui::test]
    fn test_syntactic_location_disambiguates_duplicate_symbol_paths(cx: &mut TestAppContext) {
        let text =
            "fn duplicate() {\n    let first = 1;\n}\n\nfn duplicate() {\n    let second = 2;\n}\n";
        let first = syntactic_location_at_row(text, 1, true, cx);
        let second = syntactic_location_at_row(text, 5, true, cx);

        let first_symbol = first.symbol.expect("expected the first function");
        let second_symbol = second.symbol.expect("expected the second function");
        assert_eq!(first_symbol.symbol_path, second_symbol.symbol_path);
        assert_eq!(first_symbol.symbol_ordinal, 0);
        assert_eq!(second_symbol.symbol_ordinal, 1);
    }

    #[gpui::test]
    fn test_typescript_syntactic_location_uses_nested_class_method_path(cx: &mut TestAppContext) {
        let location = typescript_syntactic_location_at_row(
            "class Store {\n    bookmarked(): void {\n        const value = 1;\n    }\n}\n",
            1,
            cx,
        );

        let symbol = location
            .symbol
            .expect("expected an enclosing TypeScript symbol");
        assert_eq!(symbol.symbol_path, "class Store › bookmarked()");
        assert_eq!(symbol.symbol_ordinal, 0);
        assert_eq!(symbol.line_offset_in_symbol, 0);
    }

    #[gpui::test]
    fn test_typescript_syntactic_location_disambiguates_duplicate_functions(
        cx: &mut TestAppContext,
    ) {
        let text = "function duplicate() {\n    return 1;\n}\n\nfunction duplicate() {\n    return 2;\n}\n";
        let first = typescript_syntactic_location_at_row(text, 1, cx);
        let second = typescript_syntactic_location_at_row(text, 5, cx);

        let first_symbol = first.symbol.expect("expected the first function");
        let second_symbol = second.symbol.expect("expected the second function");
        assert_eq!(first_symbol.symbol_path, second_symbol.symbol_path);
        assert_eq!(first_symbol.symbol_ordinal, 0);
        assert_eq!(second_symbol.symbol_ordinal, 1);
    }

    #[gpui::test]
    fn test_syntactic_location_without_parser_has_no_symbol(cx: &mut TestAppContext) {
        let location = syntactic_location_at_row("heading\nbookmarked text\n", 1, false, cx);

        assert_eq!(location.symbol, None);
        assert_eq!(location.content_marker.line_text, "bookmarked text");
        assert_eq!(location.last_known_row, 1);
    }

    #[gpui::test]
    fn test_syntactic_location_on_whitespace_between_symbols_has_no_symbol(
        cx: &mut TestAppContext,
    ) {
        let location =
            syntactic_location_at_row("fn first() {}\n    \nfn second() {}\n", 1, true, cx);

        assert_eq!(location.symbol, None);
    }

    #[gpui::test]
    fn test_content_marker_normalizes_whitespace(cx: &mut TestAppContext) {
        let first = syntactic_location_at_row("    let   value = 1;   \n", 0, false, cx);
        let second = syntactic_location_at_row("\tlet value = 1;\n", 0, false, cx);

        assert_eq!(first.content_marker.line_text, "let value = 1;");
        assert_eq!(
            first.content_marker.line_text,
            second.content_marker.line_text
        );
        assert_eq!(
            first.content_marker.context_hash,
            second.content_marker.context_hash
        );
    }

    #[gpui::test]
    fn test_content_marker_context_disambiguates_identical_lines(cx: &mut TestAppContext) {
        let first =
            syntactic_location_at_row("before one\nreturn value;\nafter one\n", 1, false, cx);
        let second =
            syntactic_location_at_row("before two\nreturn value;\nafter two\n", 1, false, cx);

        assert_eq!(
            first.content_marker.line_text,
            second.content_marker.line_text
        );
        assert_ne!(
            first.content_marker.context_hash,
            second.content_marker.context_hash
        );
    }

    #[gpui::test]
    fn test_content_marker_clamps_context_at_buffer_boundaries(cx: &mut TestAppContext) {
        let first = syntactic_location_at_row("one\ntwo\nthree\n", 0, false, cx);
        let first_again = syntactic_location_at_row("one\ntwo\nthree\n", 0, false, cx);
        let last = syntactic_location_at_row("one\ntwo\nthree\n", 2, false, cx);

        assert_eq!(
            first.content_marker.context_hash,
            first_again.content_marker.context_hash
        );
        assert_ne!(
            first.content_marker.context_hash,
            last.content_marker.context_hash
        );
    }
}
