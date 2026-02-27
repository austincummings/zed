use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::Result;
use gpui::{App, Context, Entity, EventEmitter, Subscription, Task};
use language::{Buffer, BufferEvent, BufferSnapshot, Point};
use text;
use worktree;

use crate::{ProjectPath, buffer_store::BufferStore, worktree_store::WorktreeStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub label: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkWithPosition {
    pub position: text::Anchor,
    pub bookmark: Bookmark,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBookmark {
    pub row: u32,
    pub path: Arc<Path>,
    pub label: Option<Arc<str>>,
}

struct BookmarksInFile {
    buffer: Entity<Buffer>,
    bookmarks: Vec<BookmarkWithPosition>,
    _subscription: Arc<Subscription>,
}

impl BookmarksInFile {
    fn new(buffer: Entity<Buffer>, cx: &mut Context<BookmarkStore>) -> Self {
        let subscription =
            Arc::from(
                cx.subscribe(&buffer, |bookmark_store, buffer, event, cx| match event {
                    BufferEvent::FileHandleChanged => {
                        let entity_id = buffer.entity_id();

                        if buffer
                            .read(cx)
                            .file()
                            .is_none_or(|f| f.disk_state().is_deleted())
                        {
                            bookmark_store.bookmarks.retain(|_, bookmarks_in_file| {
                                bookmarks_in_file.buffer.entity_id() != entity_id
                            });
                            cx.emit(BookmarkStoreEvent::BookmarksUpdated);
                            cx.notify();
                            return;
                        }

                        if let Some(abs_path) = BookmarkStore::abs_path_from_buffer(&buffer, cx) {
                            if bookmark_store.bookmarks.contains_key(&abs_path) {
                                return;
                            }

                            if let Some(old_path) = bookmark_store
                                .bookmarks
                                .iter()
                                .find(|(_, in_file)| in_file.buffer.entity_id() == entity_id)
                                .map(|values| values.0)
                                .cloned()
                            {
                                if let Some(bookmarks_in_file) =
                                    bookmark_store.bookmarks.remove(&old_path)
                                {
                                    bookmark_store.bookmarks.insert(abs_path, bookmarks_in_file);
                                    cx.emit(BookmarkStoreEvent::BookmarksUpdated);
                                    cx.notify();
                                }
                            }
                        }
                    }
                    _ => {}
                }),
            );

        BookmarksInFile {
            buffer,
            bookmarks: Vec::new(),
            _subscription: subscription,
        }
    }
}

pub struct BookmarkStore {
    buffer_store: Entity<BufferStore>,
    worktree_store: Entity<WorktreeStore>,
    bookmarks: BTreeMap<Arc<Path>, BookmarksInFile>,
}

#[derive(Clone, Debug)]
pub enum BookmarkStoreEvent {
    BookmarksUpdated,
}

impl EventEmitter<BookmarkStoreEvent> for BookmarkStore {}

impl BookmarkStore {
    pub fn new(worktree_store: Entity<WorktreeStore>, buffer_store: Entity<BufferStore>) -> Self {
        BookmarkStore {
            bookmarks: BTreeMap::new(),
            buffer_store,
            worktree_store,
        }
    }

    pub fn abs_path_from_buffer(buffer: &Entity<Buffer>, cx: &App) -> Option<Arc<Path>> {
        worktree::File::from_dyn(buffer.read(cx).file())
            .map(|file| file.worktree.read(cx).absolutize(&file.path))
            .map(Arc::<Path>::from)
    }

    pub fn toggle_bookmark(
        &mut self,
        buffer: Entity<Buffer>,
        position: text::Anchor,
        label: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) {
        let Some(abs_path) = Self::abs_path_from_buffer(&buffer, cx) else {
            return;
        };

        let bookmark_set = self
            .bookmarks
            .entry(abs_path)
            .or_insert_with(|| BookmarksInFile::new(buffer, cx));

        let snapshot = bookmark_set.buffer.read(cx).text_snapshot();

        let existing_index = bookmark_set.bookmarks.iter().position(|existing| {
            existing.position.summary::<Point>(&snapshot).row
                == position.summary::<Point>(&snapshot).row
        });

        if let Some(index) = existing_index {
            bookmark_set.bookmarks.remove(index);
        } else {
            bookmark_set.bookmarks.push(BookmarkWithPosition {
                position,
                bookmark: Bookmark { label },
            });
        }

        cx.emit(BookmarkStoreEvent::BookmarksUpdated);
        cx.notify();
    }

    pub fn bookmarks<'a>(
        &'a self,
        buffer: &'a Entity<Buffer>,
        range: Option<std::ops::Range<text::Anchor>>,
        buffer_snapshot: &'a BufferSnapshot,
        cx: &App,
    ) -> impl Iterator<Item = &'a BookmarkWithPosition> + 'a {
        let abs_path = Self::abs_path_from_buffer(buffer, cx);
        abs_path
            .and_then(|path| self.bookmarks.get(&path))
            .into_iter()
            .flat_map(move |file_bookmarks| {
                file_bookmarks.bookmarks.iter().filter({
                    let range = range.clone();
                    move |bp| {
                        if !buffer_snapshot.can_resolve(&bp.position) {
                            return false;
                        }

                        if let Some(range) = &range {
                            if bp.position.cmp(&range.start, buffer_snapshot).is_lt()
                                || bp.position.cmp(&range.end, buffer_snapshot).is_gt()
                            {
                                return false;
                            }
                        }

                        true
                    }
                })
            })
    }

    pub fn bookmark_at_row(
        &self,
        path: &Path,
        row: u32,
        cx: &App,
    ) -> Option<(Entity<Buffer>, BookmarkWithPosition)> {
        self.bookmarks.get(path).and_then(|bookmarks_in_file| {
            let snapshot = bookmarks_in_file.buffer.read(cx).text_snapshot();
            bookmarks_in_file
                .bookmarks
                .iter()
                .find(|bp| bp.position.summary::<Point>(&snapshot).row == row)
                .map(|bookmark| (bookmarks_in_file.buffer.clone(), bookmark.clone()))
        })
    }

    pub fn all_source_bookmarks(&self, cx: &App) -> BTreeMap<Arc<Path>, Vec<SourceBookmark>> {
        self.bookmarks
            .iter()
            .map(|(path, bookmarks_in_file)| {
                let snapshot = bookmarks_in_file.buffer.read(cx).snapshot();
                (
                    path.clone(),
                    bookmarks_in_file
                        .bookmarks
                        .iter()
                        .map(|bookmark| {
                            let row = snapshot.summary_for_anchor::<Point>(&bookmark.position).row;
                            SourceBookmark {
                                row,
                                path: path.clone(),
                                label: bookmark.bookmark.label.clone(),
                            }
                        })
                        .collect(),
                )
            })
            .collect()
    }

    pub fn all_bookmarks(&self) -> BTreeMap<Arc<Path>, Vec<BookmarkWithPosition>> {
        self.bookmarks
            .iter()
            .map(|(path, bookmarks_in_file)| (path.clone(), bookmarks_in_file.bookmarks.clone()))
            .collect()
    }

    pub fn with_serialized_bookmarks(
        &self,
        bookmarks: BTreeMap<Arc<Path>, Vec<SourceBookmark>>,
        cx: &mut Context<BookmarkStore>,
    ) -> Task<Result<()>> {
        let worktree_store = self.worktree_store.downgrade();
        let buffer_store = self.buffer_store.downgrade();
        cx.spawn(async move |this, cx| {
            let mut new_bookmarks = BTreeMap::default();
            for (path, source_bookmarks) in bookmarks {
                if source_bookmarks.is_empty() {
                    continue;
                }
                let (worktree, relative_path) = worktree_store
                    .update(cx, |this, cx| {
                        this.find_or_create_worktree(&path, false, cx)
                    })?
                    .await?;
                let buffer = buffer_store
                    .update(cx, |this, cx| {
                        let path = ProjectPath {
                            worktree_id: worktree.read(cx).id(),
                            path: relative_path,
                        };
                        this.open_buffer(path, cx)
                    })?
                    .await;
                let Ok(buffer) = buffer else {
                    log::error!(
                        "Could not open buffer for serialized bookmarks at path: {}",
                        path.to_string_lossy()
                    );
                    continue;
                };
                let snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());

                let mut bookmarks_for_file =
                    this.update(cx, |_, cx| BookmarksInFile::new(buffer, cx))?;

                for source_bookmark in source_bookmarks {
                    let max_point = snapshot.max_point();
                    let point = Point::new(source_bookmark.row, 0);
                    if point > max_point {
                        log::error!("Skipping a deserialized bookmark that's out of range");
                        continue;
                    }
                    let position = snapshot.anchor_after(point);
                    bookmarks_for_file.bookmarks.push(BookmarkWithPosition {
                        position,
                        bookmark: Bookmark {
                            label: source_bookmark.label,
                        },
                    });
                }
                new_bookmarks.insert(path, bookmarks_for_file);
            }
            this.update(cx, |this, cx| {
                for (path, count) in new_bookmarks
                    .iter()
                    .map(|(path, bm)| (path.to_string_lossy(), bm.bookmarks.len()))
                {
                    let bookmark_str = if count > 1 { "bookmarks" } else { "bookmark" };
                    log::debug!("Deserialized {count} {bookmark_str} at path: {path}");
                }

                this.bookmarks = new_bookmarks;
                cx.emit(BookmarkStoreEvent::BookmarksUpdated);
                cx.notify();
            })?;

            Ok(())
        })
    }

    pub fn clear_bookmarks(&mut self, cx: &mut Context<Self>) {
        self.bookmarks.clear();
        cx.emit(BookmarkStoreEvent::BookmarksUpdated);
        cx.notify();
    }
}
