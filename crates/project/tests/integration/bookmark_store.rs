use std::{collections::BTreeMap, path::Path, sync::Arc};

use fs::FakeFs;
use gpui::TestAppContext;
use language::Point;
use project::{Project, bookmark_store::SourceBookmark};
use serde_json::json;
use settings::SettingsStore;
use util::{path, rel_path::rel_path};

fn init_test(cx: &mut TestAppContext) {
    zlog::init_test();
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
    });
}

#[gpui::test]
async fn test_toggle_bookmark_add_and_remove(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let anchor_row1 = snapshot.anchor_after(Point::new(1, 0));

    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Toggle on: adds a bookmark
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor_row1, None, cx);
    });

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks_for_file: Vec<_> = all.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks_for_file.len(), 1);
    assert_eq!(bookmarks_for_file[0].row, 1);

    // Toggle off: removes the bookmark
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor_row1, None, cx);
    });

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks_for_file: Vec<_> = all.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks_for_file.len(), 0);
}

#[gpui::test]
async fn test_toggle_bookmark_row_deduplication(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());

    // Add bookmark at row 1, column 5
    let anchor_col5 = snapshot.anchor_after(Point::new(1, 5));
    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor_col5, None, cx);
    });

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(all.values().flat_map(|v| v.iter()).count(), 1);

    // Toggle at row 1, column 0 — should remove the existing bookmark (same row)
    let anchor_col0 = snapshot.anchor_after(Point::new(1, 0));
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor_col0, None, cx);
    });

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(all.values().flat_map(|v| v.iter()).count(), 0);
}

#[gpui::test]
async fn test_multiple_bookmarks_different_rows(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3\nline 4" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add bookmarks on rows 0, 2, and 4
    for row in [0, 2, 4] {
        let anchor = snapshot.anchor_after(Point::new(row, 0));
        bookmark_store.update(cx, |store, cx| {
            store.toggle_bookmark(buffer.clone(), anchor, None, cx);
        });
    }

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks: Vec<_> = all.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks.len(), 3);

    let mut rows: Vec<u32> = bookmarks.iter().map(|b| b.row).collect();
    rows.sort();
    assert_eq!(rows, vec![0, 2, 4]);
}

#[gpui::test]
async fn test_bookmarks_with_range_filter(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add bookmarks on rows 0, 5, and 10
    for row in [0, 5, 10] {
        let anchor = snapshot.anchor_after(Point::new(row, 0));
        bookmark_store.update(cx, |store, cx| {
            store.toggle_bookmark(buffer.clone(), anchor, None, cx);
        });
    }

    // Query with range covering rows 3-7
    let buffer_snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
    let range_start = buffer_snapshot.anchor_after(Point::new(3, 0));
    let range_end = buffer_snapshot.anchor_after(Point::new(7, 0));

    let filtered: Vec<_> = bookmark_store.read_with(cx, |store, cx| {
        store
            .bookmarks(&buffer, Some(range_start..range_end), &buffer_snapshot, cx)
            .cloned()
            .collect()
    });

    assert_eq!(filtered.len(), 1);
    let row = filtered[0].position.summary::<Point>(&buffer_snapshot).row;
    assert_eq!(row, 5);
}

#[gpui::test]
async fn test_bookmark_at_row(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    let anchor = snapshot.anchor_after(Point::new(2, 0));
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor, None, cx);
    });

    // Compute the abs_path for the buffer
    let abs_path = bookmark_store.read_with(cx, |_, cx| {
        project::bookmark_store::BookmarkStore::abs_path_from_buffer(&buffer, cx).unwrap()
    });

    // Should find a bookmark at row 2
    let found = bookmark_store.read_with(cx, |store, cx| store.bookmark_at_row(&abs_path, 2, cx));
    assert!(found.is_some());

    // Should not find a bookmark at row 0
    let not_found =
        bookmark_store.read_with(cx, |store, cx| store.bookmark_at_row(&abs_path, 0, cx));
    assert!(not_found.is_none());
}

#[gpui::test]
async fn test_clear_bookmarks(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add several bookmarks
    for row in [0, 1, 2, 3] {
        let anchor = snapshot.anchor_after(Point::new(row, 0));
        bookmark_store.update(cx, |store, cx| {
            store.toggle_bookmark(buffer.clone(), anchor, None, cx);
        });
    }

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(all.values().flat_map(|v| v.iter()).count(), 4);

    // Clear all bookmarks
    bookmark_store.update(cx, |store, cx| {
        store.clear_bookmarks(cx);
    });

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(all.values().flat_map(|v| v.iter()).count(), 0);
}

#[gpui::test]
async fn test_bookmark_positions_after_edits(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2\nline 3" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add bookmark at row 2
    let anchor = snapshot.anchor_after(Point::new(2, 0));
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor, None, cx);
    });

    // Insert two lines at the beginning of the buffer
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..0, "new line A\nnew line B\n")], None, cx);
    });

    // The bookmark should now resolve to row 4 (was row 2, shifted by 2 inserted lines)
    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks: Vec<_> = all.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].row, 4);
}

#[gpui::test]
async fn test_all_source_bookmarks(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({
            "file_a.rs": "a0\na1\na2",
            "file_b.rs": "b0\nb1\nb2\nb3"
        }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer_a = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("file_a.rs")), cx)
        })
        .await
        .unwrap();
    let buffer_b = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("file_b.rs")), cx)
        })
        .await
        .unwrap();

    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add bookmarks in file_a at rows 0 and 2
    let snapshot_a = buffer_a.read_with(cx, |buffer, _| buffer.text_snapshot());
    for row in [0, 2] {
        let anchor = snapshot_a.anchor_after(Point::new(row, 0));
        bookmark_store.update(cx, |store, cx| {
            store.toggle_bookmark(buffer_a.clone(), anchor, None, cx);
        });
    }

    // Add bookmark in file_b at row 3
    let snapshot_b = buffer_b.read_with(cx, |buffer, _| buffer.text_snapshot());
    let anchor = snapshot_b.anchor_after(Point::new(3, 0));
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer_b.clone(), anchor, None, cx);
    });

    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(all.len(), 2); // Two files

    let total_bookmarks: usize = all.values().map(|v| v.len()).sum();
    assert_eq!(total_bookmarks, 3);

    // Verify file_b has one bookmark at row 3
    let file_b_bookmarks: Vec<_> = all
        .iter()
        .filter(|(path, _)| path.to_string_lossy().contains("file_b"))
        .flat_map(|(_, v)| v.iter())
        .collect();
    assert_eq!(file_b_bookmarks.len(), 1);
    assert_eq!(file_b_bookmarks[0].row, 3);
}

#[gpui::test]
async fn test_serialization_round_trip(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({
            "file_a.rs": "a0\na1\na2\na3\na4",
            "file_b.rs": "b0\nb1\nb2"
        }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer_a = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("file_a.rs")), cx)
        })
        .await
        .unwrap();
    let buffer_b = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("file_b.rs")), cx)
        })
        .await
        .unwrap();

    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add bookmarks
    let snapshot_a = buffer_a.read_with(cx, |buffer, _| buffer.text_snapshot());
    for row in [1, 3] {
        let anchor = snapshot_a.anchor_after(Point::new(row, 0));
        bookmark_store.update(cx, |store, cx| {
            store.toggle_bookmark(buffer_a.clone(), anchor, None, cx);
        });
    }

    let snapshot_b = buffer_b.read_with(cx, |buffer, _| buffer.text_snapshot());
    let anchor = snapshot_b.anchor_after(Point::new(2, 0));
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer_b.clone(), anchor, None, cx);
    });

    // Serialize
    let serialized = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(serialized.values().flat_map(|v| v.iter()).count(), 3);

    // Clear
    bookmark_store.update(cx, |store, cx| {
        store.clear_bookmarks(cx);
    });
    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(all.values().flat_map(|v| v.iter()).count(), 0);

    // Deserialize
    let task = bookmark_store.update(cx, |store, cx| {
        store.with_serialized_bookmarks(serialized.clone(), cx)
    });
    task.await.unwrap();

    // Verify round-trip
    let restored = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    assert_eq!(restored.values().flat_map(|v| v.iter()).count(), 3);

    // Verify exact rows
    for (path, original_bookmarks) in &serialized {
        let restored_bookmarks = restored.get(path).expect("path should be restored");
        let original_rows: Vec<u32> = original_bookmarks.iter().map(|b| b.row).collect();
        let restored_rows: Vec<u32> = restored_bookmarks.iter().map(|b| b.row).collect();
        assert_eq!(original_rows, restored_rows);
    }
}

#[gpui::test]
async fn test_deserialize_out_of_range_row(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    // File has only 3 lines (rows 0, 1, 2)
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    let abs_path: Arc<Path> = bookmark_store.read_with(cx, |_, cx| {
        project::bookmark_store::BookmarkStore::abs_path_from_buffer(&buffer, cx).unwrap()
    });

    // Create serialized bookmarks with an out-of-range row
    let mut serialized: BTreeMap<Arc<Path>, Vec<SourceBookmark>> = BTreeMap::new();
    serialized.insert(
        abs_path.clone(),
        vec![
            SourceBookmark {
                row: 1,
                path: abs_path.clone(),
                label: None,
            },
            SourceBookmark {
                row: 999,
                path: abs_path.clone(),
                label: None,
            },
        ],
    );

    let task = bookmark_store.update(cx, |store, cx| {
        store.with_serialized_bookmarks(serialized, cx)
    });
    task.await.unwrap();

    // Only the valid bookmark should be restored (row 999 is out of range)
    let all = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks: Vec<_> = all.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].row, 1);
}

#[gpui::test]
async fn test_bookmark_label_round_trip(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/project"),
        json!({ "main.rs": "line 0\nline 1\nline 2" }),
    )
    .await;
    let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

    let worktree_id = project.update(cx, |project, cx| {
        project.worktrees(cx).next().unwrap().read(cx).id()
    });
    let buffer = project
        .update(cx, |project, cx| {
            project.open_buffer((worktree_id, rel_path("main.rs")), cx)
        })
        .await
        .unwrap();

    let bookmark_store = project.read_with(cx, |project, _| project.bookmark_store());

    // Add bookmark with a label
    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let anchor = snapshot.anchor_after(Point::new(1, 0));
    let label: Arc<str> = Arc::from("my important bookmark");
    bookmark_store.update(cx, |store, cx| {
        store.toggle_bookmark(buffer.clone(), anchor, Some(label.clone()), cx);
    });

    // Serialize
    let serialized = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks: Vec<_> = serialized.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].label.as_deref(), Some("my important bookmark"));

    // Clear and restore
    bookmark_store.update(cx, |store, cx| {
        store.clear_bookmarks(cx);
    });

    let task = bookmark_store.update(cx, |store, cx| {
        store.with_serialized_bookmarks(serialized, cx)
    });
    task.await.unwrap();

    // Verify label is preserved
    let restored = bookmark_store.read_with(cx, |store, cx| store.all_source_bookmarks(cx));
    let bookmarks: Vec<_> = restored.values().flat_map(|v| v.iter()).collect();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].label.as_deref(), Some("my important bookmark"));
}
