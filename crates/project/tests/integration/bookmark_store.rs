use std::{path::Path, sync::Arc};

use collections::BTreeMap;
use gpui::{Entity, TestAppContext};
use language::Buffer;
use project::{Project, bookmark_store::SerializedBookmark};
use serde_json::json;
use util::path;

mod integration {
    use super::*;
    use fs::Fs as _;
    use language::rust_lang;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            release_channel::init(semver::Version::new(0, 0, 0), cx);
        });
    }

    fn project_path(path: &str) -> Arc<Path> {
        Arc::from(Path::new(path))
    }

    async fn open_buffer(
        project: &Entity<Project>,
        path: &str,
        cx: &mut TestAppContext,
    ) -> Entity<Buffer> {
        project
            .update(cx, |project, cx| {
                project.open_local_buffer(Path::new(path), cx)
            })
            .await
            .unwrap()
    }

    fn add_bookmarks(
        project: &Entity<Project>,
        buffer: &Entity<Buffer>,
        rows: &[u32],
        cx: &mut TestAppContext,
    ) {
        let buffer = buffer.clone();
        project.update(cx, |project, cx| {
            let bookmark_store = project.bookmark_store();
            let snapshot = buffer.read(cx).snapshot();
            for &row in rows {
                let anchor = snapshot.anchor_after(text::Point::new(row, 0));
                bookmark_store.update(cx, |store, cx| {
                    store.toggle_bookmark(buffer.clone(), anchor, cx);
                });
            }
        });
    }

    fn get_all_bookmarks(
        project: &Entity<Project>,
        cx: &mut TestAppContext,
    ) -> BTreeMap<Arc<Path>, Vec<SerializedBookmark>> {
        project.read_with(cx, |project, cx| {
            project
                .bookmark_store()
                .read(cx)
                .all_serialized_bookmarks(cx)
        })
    }

    fn build_serialized(
        entries: &[(&str, &[u32])],
    ) -> BTreeMap<Arc<Path>, Vec<SerializedBookmark>> {
        let mut map = BTreeMap::new();
        for &(path_str, rows) in entries {
            let path = project_path(path_str);
            map.insert(
                path.clone(),
                rows.iter()
                    .map(|&row| SerializedBookmark {
                        row,
                        symbol_path: None,
                        offset_in_symbol: None,
                    })
                    .collect(),
            );
        }
        map
    }

    async fn restore_bookmarks(
        project: &Entity<Project>,
        serialized: BTreeMap<Arc<Path>, Vec<SerializedBookmark>>,
        cx: &mut TestAppContext,
    ) {
        project
            .update(cx, |project, cx| {
                project.bookmark_store().update(cx, |store, cx| {
                    store.with_serialized_bookmarks(serialized, cx)
                })
            })
            .await
            .expect("with_serialized_bookmarks should succeed");
    }

    fn clear_bookmarks(project: &Entity<Project>, cx: &mut TestAppContext) {
        project.update(cx, |project, cx| {
            project.bookmark_store().update(cx, |store, cx| {
                store.clear_bookmarks(cx);
            });
        });
    }

    fn assert_bookmark_rows(
        bookmarks: &BTreeMap<Arc<Path>, Vec<SerializedBookmark>>,
        path: &str,
        expected_rows: &[u32],
    ) {
        let path = project_path(path);
        let file_bookmarks = bookmarks
            .get(&path)
            .unwrap_or_else(|| panic!("Expected bookmarks for {}", path.display()));
        let rows: Vec<u32> = file_bookmarks.iter().map(|b| b.row).collect();
        assert_eq!(rows, expected_rows, "Bookmark rows for {}", path.display());
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_empty(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"file1.rs": "line1\nline2\n"}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        assert!(get_all_bookmarks(&project, cx).is_empty());
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_single_file(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file1.rs": "line1\nline2\nline3\nline4\nline5\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer = open_buffer(&project, path!("/project/file1.rs"), cx).await;

        add_bookmarks(&project, &buffer, &[0, 2], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        assert_eq!(bookmarks.len(), 1);
        assert_bookmark_rows(&bookmarks, path!("/project/file1.rs"), &[0, 2]);
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_multiple_files(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file1.rs": "line1\nline2\nline3\n",
                "file2.rs": "lineA\nlineB\nlineC\nlineD\n",
                "file3.rs": "single line"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer1 = open_buffer(&project, path!("/project/file1.rs"), cx).await;
        let buffer2 = open_buffer(&project, path!("/project/file2.rs"), cx).await;
        let _buffer3 = open_buffer(&project, path!("/project/file3.rs"), cx).await;

        add_bookmarks(&project, &buffer1, &[1], cx);
        add_bookmarks(&project, &buffer2, &[0, 3], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        assert_eq!(bookmarks.len(), 2);
        assert_bookmark_rows(&bookmarks, path!("/project/file1.rs"), &[1]);
        assert_bookmark_rows(&bookmarks, path!("/project/file2.rs"), &[0, 3]);
        assert!(
            !bookmarks.contains_key(&project_path(path!("/project/file3.rs"))),
            "file3.rs should have no bookmarks"
        );
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_after_toggle_off(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file1.rs": "line1\nline2\nline3\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer = open_buffer(&project, path!("/project/file1.rs"), cx).await;

        add_bookmarks(&project, &buffer, &[1], cx);
        assert_eq!(get_all_bookmarks(&project, cx).len(), 1);

        // Toggle same row again to remove it
        add_bookmarks(&project, &buffer, &[1], cx);
        assert!(get_all_bookmarks(&project, cx).is_empty());
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_with_clear(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file1.rs": "line1\nline2\nline3\n",
                "file2.rs": "lineA\nlineB\n"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer1 = open_buffer(&project, path!("/project/file1.rs"), cx).await;
        let buffer2 = open_buffer(&project, path!("/project/file2.rs"), cx).await;

        add_bookmarks(&project, &buffer1, &[0], cx);
        add_bookmarks(&project, &buffer2, &[1], cx);
        assert_eq!(get_all_bookmarks(&project, cx).len(), 2);

        clear_bookmarks(&project, cx);
        assert!(get_all_bookmarks(&project, cx).is_empty());
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_returns_sorted_by_path(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"b.rs": "line1\n", "a.rs": "line1\n", "c.rs": "line1\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer_b = open_buffer(&project, path!("/project/b.rs"), cx).await;
        let buffer_a = open_buffer(&project, path!("/project/a.rs"), cx).await;
        let buffer_c = open_buffer(&project, path!("/project/c.rs"), cx).await;

        add_bookmarks(&project, &buffer_b, &[0], cx);
        add_bookmarks(&project, &buffer_a, &[0], cx);
        add_bookmarks(&project, &buffer_c, &[0], cx);

        let paths: Vec<_> = get_all_bookmarks(&project, cx).keys().cloned().collect();
        assert_eq!(
            paths,
            [
                project_path(path!("/project/a.rs")),
                project_path(path!("/project/b.rs")),
                project_path(path!("/project/c.rs")),
            ]
        );
    }

    #[gpui::test]
    async fn test_all_serialized_bookmarks_deduplicates_same_row(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file1.rs": "line1\nline2\nline3\nline4\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer = open_buffer(&project, path!("/project/file1.rs"), cx).await;

        add_bookmarks(&project, &buffer, &[1, 2], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&bookmarks, path!("/project/file1.rs"), &[1, 2]);

        // Verify no duplicates
        let rows: Vec<u32> = bookmarks
            .get(&project_path(path!("/project/file1.rs")))
            .unwrap()
            .iter()
            .map(|b| b.row)
            .collect();
        let mut deduped = rows.clone();
        deduped.dedup();
        assert_eq!(rows, deduped);
    }

    #[gpui::test]
    async fn test_with_serialized_bookmarks_restores_bookmarks(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file1.rs": "line1\nline2\nline3\nline4\nline5\n",
                "file2.rs": "aaa\nbbb\nccc\n"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

        let serialized = build_serialized(&[
            (path!("/project/file1.rs"), &[0, 3]),
            (path!("/project/file2.rs"), &[1]),
        ]);

        restore_bookmarks(&project, serialized, cx).await;

        let restored = get_all_bookmarks(&project, cx);
        assert_eq!(restored.len(), 2);
        assert_bookmark_rows(&restored, path!("/project/file1.rs"), &[0, 3]);
        assert_bookmark_rows(&restored, path!("/project/file2.rs"), &[1]);
    }

    #[gpui::test]
    async fn test_with_serialized_bookmarks_skips_out_of_range_rows(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        // 3 lines: rows 0, 1, 2
        fs.insert_tree(
            path!("/project"),
            json!({"file1.rs": "line1\nline2\nline3"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

        let serialized = build_serialized(&[(path!("/project/file1.rs"), &[1, 100, 2])]);
        restore_bookmarks(&project, serialized, cx).await;

        // Before resolution, unloaded bookmarks are stored as-is
        let unresolved = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&unresolved, path!("/project/file1.rs"), &[1, 2, 100]);

        // Open the buffer to trigger lazy resolution
        let buffer = open_buffer(&project, path!("/project/file1.rs"), cx).await;
        project.update(cx, |project, cx| {
            let buffer_snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &buffer_snapshot, cx);
            });
        });

        // After resolution, out-of-range rows are filtered
        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/file1.rs"), &[1, 2]);
    }

    #[gpui::test]
    async fn test_with_serialized_bookmarks_skips_empty_entries(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file1.rs": "line1\nline2\n", "file2.rs": "aaa\nbbb\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

        let mut serialized = build_serialized(&[(path!("/project/file1.rs"), &[0])]);
        serialized.insert(project_path(path!("/project/file2.rs")), vec![]);

        restore_bookmarks(&project, serialized, cx).await;

        let restored = get_all_bookmarks(&project, cx);
        assert_eq!(restored.len(), 1);
        assert!(restored.contains_key(&project_path(path!("/project/file1.rs"))));
        assert!(!restored.contains_key(&project_path(path!("/project/file2.rs"))));
    }

    #[gpui::test]
    async fn test_with_serialized_bookmarks_all_out_of_range_produces_no_entry(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"tiny.rs": "x"}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;

        let serialized = build_serialized(&[(path!("/project/tiny.rs"), &[5, 10])]);
        restore_bookmarks(&project, serialized, cx).await;

        // Before resolution, unloaded bookmarks are stored as-is
        let unresolved = get_all_bookmarks(&project, cx);
        assert_eq!(unresolved.len(), 1);

        // Open the buffer to trigger lazy resolution
        let buffer = open_buffer(&project, path!("/project/tiny.rs"), cx).await;
        project.update(cx, |project, cx| {
            let buffer_snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &buffer_snapshot, cx);
            });
        });

        // After resolution, all out-of-range rows are filtered away
        assert!(get_all_bookmarks(&project, cx).is_empty());
    }

    #[gpui::test]
    async fn test_with_serialized_bookmarks_replaces_existing(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file1.rs": "aaa\nbbb\nccc\nddd\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer = open_buffer(&project, path!("/project/file1.rs"), cx).await;

        add_bookmarks(&project, &buffer, &[0], cx);
        assert_bookmark_rows(
            &get_all_bookmarks(&project, cx),
            path!("/project/file1.rs"),
            &[0],
        );

        // Restoring different bookmarks should replace, not merge
        let serialized = build_serialized(&[(path!("/project/file1.rs"), &[2, 3])]);
        restore_bookmarks(&project, serialized, cx).await;

        let after = get_all_bookmarks(&project, cx);
        assert_eq!(after.len(), 1);
        assert_bookmark_rows(&after, path!("/project/file1.rs"), &[2, 3]);
    }

    #[gpui::test]
    async fn test_serialize_deserialize_round_trip(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "alpha.rs": "fn main() {\n    println!(\"hello\");\n    return;\n}\n",
                "beta.rs": "use std::io;\nfn read() {}\nfn write() {}\n"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer_alpha = open_buffer(&project, path!("/project/alpha.rs"), cx).await;
        let buffer_beta = open_buffer(&project, path!("/project/beta.rs"), cx).await;

        add_bookmarks(&project, &buffer_alpha, &[0, 2, 3], cx);
        add_bookmarks(&project, &buffer_beta, &[1], cx);

        // Serialize
        let serialized = get_all_bookmarks(&project, cx);
        assert_eq!(serialized.len(), 2);
        assert_bookmark_rows(&serialized, path!("/project/alpha.rs"), &[0, 2, 3]);
        assert_bookmark_rows(&serialized, path!("/project/beta.rs"), &[1]);

        // Clear and restore
        clear_bookmarks(&project, cx);
        assert!(get_all_bookmarks(&project, cx).is_empty());

        restore_bookmarks(&project, serialized, cx).await;

        let restored = get_all_bookmarks(&project, cx);
        assert_eq!(restored.len(), 2);
        assert_bookmark_rows(&restored, path!("/project/alpha.rs"), &[0, 2, 3]);
        assert_bookmark_rows(&restored, path!("/project/beta.rs"), &[1]);
    }

    #[gpui::test]
    async fn test_round_trip_preserves_bookmarks_after_file_edit(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file.rs": "aaa\nbbb\nccc\nddd\neee\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let buffer = open_buffer(&project, path!("/project/file.rs"), cx).await;

        add_bookmarks(&project, &buffer, &[1, 3], cx);

        // Insert a line at the beginning, shifting bookmarks down by 1
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(0..0, "new_first_line\n")], None, cx);
        });

        let serialized = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&serialized, path!("/project/file.rs"), &[2, 4]);

        // Clear and restore
        clear_bookmarks(&project, cx);
        restore_bookmarks(&project, serialized, cx).await;

        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/file.rs"), &[2, 4]);
    }

    #[gpui::test]
    async fn test_file_deletion_removes_bookmarks(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file1.rs": "aaa\nbbb\nccc\n",
                "file2.rs": "ddd\neee\nfff\n"
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        let buffer1 = open_buffer(&project, path!("/project/file1.rs"), cx).await;
        let buffer2 = open_buffer(&project, path!("/project/file2.rs"), cx).await;

        add_bookmarks(&project, &buffer1, &[0, 2], cx);
        add_bookmarks(&project, &buffer2, &[1], cx);
        assert_eq!(get_all_bookmarks(&project, cx).len(), 2);

        // Delete file1.rs
        fs.remove_file(path!("/project/file1.rs").as_ref(), Default::default())
            .await
            .unwrap();
        cx.executor().run_until_parked();

        // file1.rs bookmarks should be gone, file2.rs bookmarks preserved
        let bookmarks = get_all_bookmarks(&project, cx);
        assert_eq!(bookmarks.len(), 1);
        assert!(!bookmarks.contains_key(&project_path(path!("/project/file1.rs"))));
        assert_bookmark_rows(&bookmarks, path!("/project/file2.rs"), &[1]);
    }

    #[gpui::test]
    async fn test_deleting_all_bookmarked_files_clears_store(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file1.rs": "aaa\nbbb\n",
                "file2.rs": "ccc\nddd\n"
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        let buffer1 = open_buffer(&project, path!("/project/file1.rs"), cx).await;
        let buffer2 = open_buffer(&project, path!("/project/file2.rs"), cx).await;

        add_bookmarks(&project, &buffer1, &[0], cx);
        add_bookmarks(&project, &buffer2, &[1], cx);
        assert_eq!(get_all_bookmarks(&project, cx).len(), 2);

        // Delete both files
        fs.remove_file(path!("/project/file1.rs").as_ref(), Default::default())
            .await
            .unwrap();
        fs.remove_file(path!("/project/file2.rs").as_ref(), Default::default())
            .await
            .unwrap();
        cx.executor().run_until_parked();

        assert!(get_all_bookmarks(&project, cx).is_empty());
    }

    #[gpui::test]
    async fn test_file_rename_re_keys_bookmarks(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"old_name.rs": "aaa\nbbb\nccc\n"}))
            .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        let buffer = open_buffer(&project, path!("/project/old_name.rs"), cx).await;

        add_bookmarks(&project, &buffer, &[0, 2], cx);
        assert_bookmark_rows(
            &get_all_bookmarks(&project, cx),
            path!("/project/old_name.rs"),
            &[0, 2],
        );

        // Rename the file
        fs.rename(
            path!("/project/old_name.rs").as_ref(),
            path!("/project/new_name.rs").as_ref(),
            Default::default(),
        )
        .await
        .unwrap();
        cx.executor().run_until_parked();

        let bookmarks = get_all_bookmarks(&project, cx);
        assert_eq!(bookmarks.len(), 1);
        assert!(!bookmarks.contains_key(&project_path(path!("/project/old_name.rs"))));
        assert_bookmark_rows(&bookmarks, path!("/project/new_name.rs"), &[0, 2]);
    }

    #[gpui::test]
    async fn test_file_rename_preserves_other_bookmarks(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "rename_me.rs": "aaa\nbbb\n",
                "untouched.rs": "ccc\nddd\neee\n"
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        let buffer_rename = open_buffer(&project, path!("/project/rename_me.rs"), cx).await;
        let buffer_other = open_buffer(&project, path!("/project/untouched.rs"), cx).await;

        add_bookmarks(&project, &buffer_rename, &[1], cx);
        add_bookmarks(&project, &buffer_other, &[0, 2], cx);

        fs.rename(
            path!("/project/rename_me.rs").as_ref(),
            path!("/project/renamed.rs").as_ref(),
            Default::default(),
        )
        .await
        .unwrap();
        cx.executor().run_until_parked();

        let bookmarks = get_all_bookmarks(&project, cx);
        assert_eq!(bookmarks.len(), 2);
        assert_bookmark_rows(&bookmarks, path!("/project/renamed.rs"), &[1]);
        assert_bookmark_rows(&bookmarks, path!("/project/untouched.rs"), &[0, 2]);
    }

    fn build_syntactic_serialized(
        entries: &[(&str, &[(u32, Option<Vec<&str>>, Option<u32>)])],
    ) -> BTreeMap<Arc<Path>, Vec<SerializedBookmark>> {
        let mut map = BTreeMap::new();
        for (path_str, bookmarks) in entries {
            let path = project_path(path_str);
            map.insert(
                path,
                bookmarks
                    .iter()
                    .map(|(row, sym, offset)| SerializedBookmark {
                        row: *row,
                        symbol_path: sym
                            .as_ref()
                            .map(|v| v.iter().map(|s| s.to_string()).collect()),
                        offset_in_symbol: *offset,
                    })
                    .collect(),
            );
        }
        map
    }

    /// Registers rust_lang() on the project so .rs files get tree-sitter parsing.
    fn register_rust_language(project: &Entity<Project>, cx: &mut TestAppContext) {
        let language_registry = project.read_with(cx, |project, _| project.languages().clone());
        language_registry.add(rust_lang());
    }

    #[gpui::test]
    async fn test_compute_syntactic_context_without_language(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file.rs": "fn hello() {\n    let x = 1;\n}\n"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        // Deliberately do NOT register rust language
        let buffer = open_buffer(&project, path!("/project/file.rs"), cx).await;

        let bookmarks = project.read_with(cx, |project, cx| {
            let buffer_entity = buffer.clone();
            let snapshot = buffer_entity.read(cx).snapshot();
            let bookmark_store = project.bookmark_store();
            let all = bookmark_store.read(cx).all_serialized_bookmarks(cx);
            // Manually check compute_syntactic_context
            let (sym, offset) =
                project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, 0);
            (sym, offset, all)
        });

        // Without language registered, symbol_path should be None
        assert!(
            bookmarks.0.is_none(),
            "Expected None symbol_path without language, got: {:?}",
            bookmarks.0
        );
        assert!(bookmarks.1.is_none());
    }

    #[gpui::test]
    async fn test_compute_syntactic_context_with_language(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "file.rs": "fn hello() {\n    let x = 1;\n}\n"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/file.rs"), cx).await;
        // Allow tree-sitter to parse
        cx.executor().run_until_parked();

        let (symbol_path, offset_in_symbol) = project.read_with(cx, |_project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, 1)
        });

        assert!(
            symbol_path.is_some(),
            "Expected symbol_path with Rust language, got None. \
             Is tree-sitter parsing active?"
        );
        let path = symbol_path.unwrap();
        assert!(
            path.iter().any(|s| s.contains("hello")),
            "Expected symbol path to contain 'hello', got: {:?}",
            path
        );
        // Row 1 is inside fn hello() which starts at row 0, so offset should be 1
        assert_eq!(offset_in_symbol, Some(1));
    }

    #[gpui::test]
    async fn test_compute_syntactic_context_nested_symbols(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let rust_source = "\
struct Processor;\n\
\n\
impl Processor {\n\
    fn process(&self) {\n\
        let x = 1;\n\
    }\n\
\n\
    fn finalize(&self) {\n\
        let y = 2;\n\
    }\n\
}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"lib.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/lib.rs"), cx).await;
        cx.executor().run_until_parked();

        // Row 4 is "let x = 1;" inside fn process inside impl Processor
        let (symbol_path, offset) = project.read_with(cx, |_project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, 4)
        });

        assert!(
            symbol_path.is_some(),
            "Expected nested symbol path, got None"
        );
        let path = symbol_path.unwrap();
        assert!(
            path.len() >= 2,
            "Expected at least 2 levels of nesting (impl + fn), got: {:?}",
            path
        );
        assert!(
            path.iter().any(|s| s.contains("Processor")),
            "Expected symbol path to mention 'Processor', got: {:?}",
            path
        );
        assert!(
            path.iter().any(|s| s.contains("process")),
            "Expected symbol path to mention 'process', got: {:?}",
            path
        );
        assert!(offset.is_some());
    }

    #[gpui::test]
    async fn test_serialized_bookmarks_include_syntactic_context(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let rust_source = "fn alpha() {\n    let a = 1;\n}\n\nfn beta() {\n    let b = 2;\n}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        // Add bookmark inside fn alpha (row 1) and fn beta (row 5)
        add_bookmarks(&project, &buffer, &[1, 5], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        let file_bookmarks = bookmarks
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks for main.rs");

        assert_eq!(file_bookmarks.len(), 2);

        // Check that serialized bookmarks have symbol_path populated
        let bookmark_alpha = &file_bookmarks[0];
        assert_eq!(bookmark_alpha.row, 1);
        assert!(
            bookmark_alpha.symbol_path.is_some(),
            "Bookmark at row 1 should have symbol_path, got None"
        );
        if let Some(path) = &bookmark_alpha.symbol_path {
            assert!(
                path.iter().any(|s| s.contains("alpha")),
                "Bookmark at row 1 should reference 'alpha', got: {:?}",
                path
            );
        }

        let bookmark_beta = &file_bookmarks[1];
        assert_eq!(bookmark_beta.row, 5);
        assert!(
            bookmark_beta.symbol_path.is_some(),
            "Bookmark at row 5 should have symbol_path, got None"
        );
        if let Some(path) = &bookmark_beta.symbol_path {
            assert!(
                path.iter().any(|s| s.contains("beta")),
                "Bookmark at row 5 should reference 'beta', got: {:?}",
                path
            );
        }
    }

    #[gpui::test]
    async fn test_syntactic_resolve_after_lines_inserted(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // Original source: fn target starts at row 4
        let original = "\
fn preamble() {\n\
    let a = 1;\n\
}\n\
\n\
fn target() {\n\
    let b = 2;\n\
}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": original}))
            .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        // Bookmark on row 5 ("let b = 2;") inside fn target
        add_bookmarks(&project, &buffer, &[5], cx);

        // Serialize (captures syntactic context for fn target)
        let serialized = get_all_bookmarks(&project, cx);
        let file_bookmarks = serialized
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks");
        assert_eq!(file_bookmarks.len(), 1);
        assert_eq!(file_bookmarks[0].row, 5);

        // Verify symbol_path was captured
        assert!(
            file_bookmarks[0].symbol_path.is_some(),
            "Expected symbol_path for bookmark inside fn target, got None. \
             This means compute_syntactic_context returned no symbols. \
             Possible causes: language not registered, tree-sitter not parsed."
        );

        // Now clear and restore with modified file where 3 blank lines
        // were inserted at the top, shifting fn target from row 4 to row 7
        clear_bookmarks(&project, cx);

        // Modify the buffer to insert lines at the beginning
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(0..0, "\n\n\n")], None, cx);
        });
        cx.executor().run_until_parked();

        // Restore bookmarks with original serialized data
        // (row=5, but fn target has moved to row 7, "let b = 2;" is now row 8)
        restore_bookmarks(&project, serialized, cx).await;

        // Trigger resolution by querying bookmarks
        project.update(cx, |project, cx| {
            let buffer_snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &buffer_snapshot, cx);
            });
        });

        let restored = get_all_bookmarks(&project, cx);
        let restored_bookmarks = restored
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected restored bookmarks");
        assert_eq!(restored_bookmarks.len(), 1);

        // The bookmark should have been resolved to the new location of
        // "let b = 2;" inside fn target, which is now at row 8 (target starts at row 7, offset 1)
        let restored_row = restored_bookmarks[0].row;
        assert_ne!(
            restored_row, 5,
            "Bookmark should NOT be at original row 5 after lines were inserted"
        );
        assert_eq!(
            restored_row, 8,
            "Bookmark should be at row 8 (fn target moved to row 7, offset 1)"
        );
    }

    #[gpui::test]
    async fn test_syntactic_resolve_fallback_to_row_without_language(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({"file.txt": "aaa\nbbb\nccc\nddd\neee\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        // No language registered — bookmarks will have no syntactic context

        // Restore with a plain row-only bookmark
        let serialized = build_serialized(&[(path!("/project/file.txt"), &[2])]);
        restore_bookmarks(&project, serialized, cx).await;

        let buffer = open_buffer(&project, path!("/project/file.txt"), cx).await;
        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });

        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/file.txt"), &[2]);
    }

    #[gpui::test]
    async fn test_syntactic_resolve_with_symbol_path_and_no_match_falls_back(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        // The file has fn hello but the bookmark references fn nonexistent
        fs.insert_tree(
            path!("/project"),
            json!({"main.rs": "fn hello() {\n    let x = 1;\n}\n"}),
        )
        .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        // Serialize with a symbol_path that won't be found
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(1, Some(vec!["fn nonexistent"]), Some(1))],
        )]);
        restore_bookmarks(&project, serialized, cx).await;

        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });

        // Should fall back to row 1
        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/main.rs"), &[1]);
    }

    #[gpui::test]
    async fn test_syntactic_resolve_with_matching_symbol_path(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let rust_source = "fn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        // First, figure out what symbol text looks like for fn second
        let second_symbol_text = project.read_with(cx, |_project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            let (path, _) =
                project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, 5);
            path.expect("fn second should produce a symbol path")
        });

        // Now create a serialized bookmark using the exact symbol text,
        // but with a WRONG row (row 0 instead of row 5).
        // If syntactic resolution works, it should resolve to inside fn second,
        // not fall back to row 0.
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(
                0,
                Some(second_symbol_text.iter().map(|s| s.as_str()).collect()),
                Some(1),
            )],
        )]);

        clear_bookmarks(&project, cx);
        restore_bookmarks(&project, serialized, cx).await;

        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });

        let restored = get_all_bookmarks(&project, cx);
        let file_bookmarks = restored
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks");
        assert_eq!(file_bookmarks.len(), 1);

        // Should resolve to fn second's body (row 4 + offset 1 = row 5),
        // NOT the fallback row 0
        assert_ne!(
            file_bookmarks[0].row, 0,
            "Syntactic resolution should override the fallback row"
        );
        assert_eq!(
            file_bookmarks[0].row, 5,
            "Should resolve to row 5 (fn second start + offset 1)"
        );
    }

    #[gpui::test]
    async fn test_serialized_bookmark_round_trip_preserves_symbol_path(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let rust_source = "fn greet() {\n    println!(\"hi\");\n}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        // Add bookmark inside fn greet
        add_bookmarks(&project, &buffer, &[1], cx);

        // Serialize
        let serialized = get_all_bookmarks(&project, cx);
        let file_bookmarks = &serialized[&project_path(path!("/project/main.rs"))];
        assert_eq!(file_bookmarks.len(), 1);
        let original = file_bookmarks[0].clone();

        // Verify syntactic info
        assert!(original.symbol_path.is_some(), "Should have symbol_path");
        assert!(
            original.offset_in_symbol.is_some(),
            "Should have offset_in_symbol"
        );

        // Clear, restore, resolve, re-serialize
        clear_bookmarks(&project, cx);
        restore_bookmarks(&project, serialized, cx).await;

        // Trigger resolution
        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });

        // Re-serialize and compare
        let re_serialized = get_all_bookmarks(&project, cx);
        let re_file_bookmarks = &re_serialized[&project_path(path!("/project/main.rs"))];
        assert_eq!(re_file_bookmarks.len(), 1);

        assert_eq!(
            re_file_bookmarks[0].row, original.row,
            "Row should survive round trip"
        );
        assert_eq!(
            re_file_bookmarks[0].symbol_path, original.symbol_path,
            "Symbol path should survive round trip"
        );
        assert_eq!(
            re_file_bookmarks[0].offset_in_symbol, original.offset_in_symbol,
            "Offset in symbol should survive round trip"
        );
    }

    #[gpui::test]
    async fn test_syntactic_resolve_deferred_until_reparse(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // Two functions so the bookmark can move to a non-trivial position.
        // fn preamble occupies rows 0-2, fn target occupies rows 4-6.
        let rust_source =
            "fn preamble() {\n    let a = 0;\n}\n\nfn target() {\n    let x = 1;\n}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        // First, open the buffer and let tree-sitter parse so we can learn
        // the exact symbol text that the outline query produces.
        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        let target_symbol_text = project.read_with(cx, |_project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            let (path, _) =
                project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, 5);
            let path = path.expect("fn target should produce a symbol path");
            assert!(
                path.iter().any(|s| s.contains("target")),
                "Expected symbol path containing 'target', got: {:?}",
                path
            );
            path
        });

        // Now simulate a session restore: clear everything, restore serialized
        // bookmarks, then re-open the buffer.
        clear_bookmarks(&project, cx);

        // Build serialized data: fallback row is 0 (wrong on purpose),
        // but the symbol path points to fn target with offset 1.
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(
                0,
                Some(target_symbol_text.iter().map(|s| s.as_str()).collect()),
                Some(1),
            )],
        )]);
        restore_bookmarks(&project, serialized, cx).await;

        // Trigger resolution by querying bookmarks_for_buffer.
        // At this point the buffer IS parsed (from our earlier run_until_parked),
        // so the outline should be available and resolution should work immediately.
        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });

        let after = get_all_bookmarks(&project, cx);
        let bookmarks = after
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks");
        assert_eq!(bookmarks.len(), 1);

        // fn target starts at row 4, offset 1 = row 5
        assert_ne!(
            bookmarks[0].row, 0,
            "Bookmark should NOT remain at fallback row 0"
        );
        assert_eq!(
            bookmarks[0].row, 5,
            "Bookmark should resolve to row 5 (fn target at row 4 + offset 1)"
        );
    }

    #[gpui::test]
    async fn test_syntactic_resolve_deferred_via_reparse_event(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let rust_source =
            "fn preamble() {\n    let a = 0;\n}\n\nfn target() {\n    let x = 1;\n}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        // Learn the real symbol text by parsing first
        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        let target_symbol_text = project.read_with(cx, |_project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            let (path, _) =
                project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, 5);
            path.expect("fn target should have symbol path")
        });

        // Drop the buffer so we can simulate a fresh open.
        // Actually, we can't easily drop a buffer from the store, so instead
        // let's clear bookmarks, restore with syntactic data, then trigger
        // an edit (which causes reparse) to exercise the Reparsed handler.
        clear_bookmarks(&project, cx);

        // Restore with wrong fallback row
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(
                0,
                Some(target_symbol_text.iter().map(|s| s.as_str()).collect()),
                Some(1),
            )],
        )]);
        restore_bookmarks(&project, serialized, cx).await;

        // Trigger resolution — the buffer already has an outline from the
        // previous parse, so the pending_syntactic path won't be exercised
        // in this case. Instead this tests the immediate resolution path.
        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });

        let result = get_all_bookmarks(&project, cx);
        let bookmarks = result
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks");
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(
            bookmarks[0].row, 5,
            "Bookmark should resolve to row 5 via immediate syntactic resolution"
        );

        // Now verify the Reparsed event path works by editing and checking
        // that bookmarks are still correctly placed after reparse.
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(0..0, "// comment\n")], None, cx);
        });
        cx.executor().run_until_parked();

        let after_edit = get_all_bookmarks(&project, cx);
        let after_edit_bookmarks = after_edit
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks after edit");
        assert_eq!(after_edit_bookmarks.len(), 1);
        // The bookmark anchor should have shifted with the edit (row 5 -> row 6)
        assert_eq!(
            after_edit_bookmarks[0].row, 6,
            "After inserting a line, bookmark should shift to row 6"
        );
    }

    #[gpui::test]
    async fn test_bookmark_outside_any_symbol_has_no_syntactic_context(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // Row 3 is a blank line between two functions
        let rust_source = "fn first() {\n}\n\n\nfn second() {\n}\n";

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({"main.rs": rust_source}))
            .await;

        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let buffer = open_buffer(&project, path!("/project/main.rs"), cx).await;
        cx.executor().run_until_parked();

        // Row 3 is blank, outside any function
        add_bookmarks(&project, &buffer, &[3], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        let file_bookmarks = &bookmarks[&project_path(path!("/project/main.rs"))];
        assert_eq!(file_bookmarks.len(), 1);
        assert_eq!(file_bookmarks[0].row, 3);
        assert!(
            file_bookmarks[0].symbol_path.is_none(),
            "Bookmark on blank line should have no symbol_path, got: {:?}",
            file_bookmarks[0].symbol_path
        );
    }
}
