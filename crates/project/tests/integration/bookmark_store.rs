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
                        context_snippet: None,
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
                        context_snippet: None,
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

    /// Sets up a test project with Rust language support and a single file.
    /// Returns the project, the FakeFs, and the opened + parsed buffer.
    async fn setup_rust_project(
        source: &str,
        filename: &str,
        cx: &mut TestAppContext,
    ) -> (Entity<Project>, Arc<fs::FakeFs>, Entity<Buffer>) {
        init_test(cx);
        cx.executor().allow_parking();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), json!({ filename: source }))
            .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        register_rust_language(&project, cx);

        let full_path = format!("/project/{filename}");
        let buffer = open_buffer(&project, &full_path, cx).await;
        cx.executor().run_until_parked();

        (project, fs, buffer)
    }

    /// Triggers syntactic bookmark resolution by calling `bookmarks_for_buffer`.
    fn trigger_resolution(
        project: &Entity<Project>,
        buffer: &Entity<Buffer>,
        cx: &mut TestAppContext,
    ) {
        project.update(cx, |project, cx| {
            let snapshot = buffer.read(cx).snapshot();
            project.bookmark_store().update(cx, |store, cx| {
                store.bookmarks_for_buffer(buffer.clone(), None, &snapshot, cx);
            });
        });
    }

    /// Computes the syntactic context (symbol_path, offset_in_symbol) for a row.
    fn compute_syntactic_context(
        buffer: &Entity<Buffer>,
        row: u32,
        cx: &mut TestAppContext,
    ) -> (Option<Vec<String>>, Option<u32>) {
        buffer.read_with(cx, |buffer, _cx| {
            let snapshot = buffer.snapshot();
            project::bookmark_store::BookmarkStore::compute_syntactic_context(&snapshot, row)
        })
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

        let (symbol_path, offset) = compute_syntactic_context(&buffer, 0, cx);

        assert!(
            symbol_path.is_none(),
            "Expected None symbol_path without language, got: {:?}",
            symbol_path
        );
        assert!(offset.is_none());
    }

    #[gpui::test]
    async fn test_compute_syntactic_context_with_language(cx: &mut TestAppContext) {
        let (_project, _fs, buffer) =
            setup_rust_project("fn hello() {\n    let x = 1;\n}\n", "file.rs", cx).await;

        let (symbol_path, offset_in_symbol) = compute_syntactic_context(&buffer, 1, cx);

        let path = symbol_path.expect("Expected symbol_path with Rust language");
        assert!(
            path.iter().any(|s| s.contains("hello")),
            "Expected symbol path to contain 'hello', got: {:?}",
            path
        );
        assert_eq!(offset_in_symbol, Some(1));
    }

    #[gpui::test]
    async fn test_compute_syntactic_context_nested_symbols(cx: &mut TestAppContext) {
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

        let (_project, _fs, buffer) = setup_rust_project(rust_source, "lib.rs", cx).await;

        // Row 4 is "let x = 1;" inside fn process inside impl Processor
        let (symbol_path, offset) = compute_syntactic_context(&buffer, 4, cx);

        let path = symbol_path.expect("Expected nested symbol path");
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
        let rust_source = "fn alpha() {\n    let a = 1;\n}\n\nfn beta() {\n    let b = 2;\n}\n";
        let (project, _fs, buffer) = setup_rust_project(rust_source, "main.rs", cx).await;

        add_bookmarks(&project, &buffer, &[1, 5], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        let file_bookmarks = bookmarks
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks for main.rs");

        assert_eq!(file_bookmarks.len(), 2);

        let bookmark_alpha = &file_bookmarks[0];
        assert_eq!(bookmark_alpha.row, 1);
        let alpha_path = bookmark_alpha
            .symbol_path
            .as_ref()
            .expect("Bookmark at row 1 should have symbol_path");
        assert!(
            alpha_path.iter().any(|s| s.contains("alpha")),
            "Bookmark at row 1 should reference 'alpha', got: {:?}",
            alpha_path
        );

        let bookmark_beta = &file_bookmarks[1];
        assert_eq!(bookmark_beta.row, 5);
        let beta_path = bookmark_beta
            .symbol_path
            .as_ref()
            .expect("Bookmark at row 5 should have symbol_path");
        assert!(
            beta_path.iter().any(|s| s.contains("beta")),
            "Bookmark at row 5 should reference 'beta', got: {:?}",
            beta_path
        );
    }

    #[gpui::test]
    async fn test_syntactic_resolve_after_lines_inserted(cx: &mut TestAppContext) {
        // Original source: fn target starts at row 4
        let original = "\
fn preamble() {\n\
    let a = 1;\n\
}\n\
\n\
fn target() {\n\
    let b = 2;\n\
}\n";

        let (project, _fs, buffer) = setup_rust_project(original, "main.rs", cx).await;

        // Bookmark on row 5 ("let b = 2;") inside fn target
        add_bookmarks(&project, &buffer, &[5], cx);

        let serialized = get_all_bookmarks(&project, cx);
        let file_bookmarks = serialized
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks");
        assert_eq!(file_bookmarks.len(), 1);
        assert_eq!(file_bookmarks[0].row, 5);
        assert!(
            file_bookmarks[0].symbol_path.is_some(),
            "Expected symbol_path for bookmark inside fn target"
        );

        // Insert 3 blank lines at top, shifting fn target from row 4 to row 7
        clear_bookmarks(&project, cx);
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(0..0, "\n\n\n")], None, cx);
        });
        cx.executor().run_until_parked();

        // Restore with original serialized data and resolve
        restore_bookmarks(&project, serialized, cx).await;
        trigger_resolution(&project, &buffer, cx);

        let restored = get_all_bookmarks(&project, cx);
        let restored_bookmarks = restored
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected restored bookmarks");
        assert_eq!(restored_bookmarks.len(), 1);
        assert_eq!(
            restored_bookmarks[0].row, 8,
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

        let serialized = build_serialized(&[(path!("/project/file.txt"), &[2])]);
        restore_bookmarks(&project, serialized, cx).await;

        let buffer = open_buffer(&project, path!("/project/file.txt"), cx).await;
        trigger_resolution(&project, &buffer, cx);

        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/file.txt"), &[2]);
    }

    #[gpui::test]
    async fn test_syntactic_resolve_with_symbol_path_and_no_match_falls_back(
        cx: &mut TestAppContext,
    ) {
        let (project, _fs, buffer) =
            setup_rust_project("fn hello() {\n    let x = 1;\n}\n", "main.rs", cx).await;

        // Restore with a symbol_path that won't be found in the file
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(1, Some(vec!["fn nonexistent"]), Some(1))],
        )]);
        restore_bookmarks(&project, serialized, cx).await;
        trigger_resolution(&project, &buffer, cx);

        // Should fall back to row 1
        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/main.rs"), &[1]);
    }

    #[gpui::test]
    async fn test_syntactic_resolve_with_matching_symbol_path(cx: &mut TestAppContext) {
        let rust_source = "fn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n}\n";
        let (project, _fs, buffer) = setup_rust_project(rust_source, "main.rs", cx).await;

        let (second_symbol_path, _) = compute_syntactic_context(&buffer, 5, cx);
        let second_symbol_text =
            second_symbol_path.expect("fn second should produce a symbol path");

        // Create serialized bookmark with the correct symbol text but a WRONG
        // fallback row (0 instead of 5). Syntactic resolution should override it.
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
        trigger_resolution(&project, &buffer, cx);

        let restored = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&restored, path!("/project/main.rs"), &[5]);
    }

    #[gpui::test]
    async fn test_serialized_bookmark_round_trip_preserves_symbol_path(cx: &mut TestAppContext) {
        let (project, _fs, buffer) =
            setup_rust_project("fn greet() {\n    println!(\"hi\");\n}\n", "main.rs", cx).await;

        add_bookmarks(&project, &buffer, &[1], cx);

        let serialized = get_all_bookmarks(&project, cx);
        let original = serialized[&project_path(path!("/project/main.rs"))][0].clone();
        assert!(original.symbol_path.is_some(), "Should have symbol_path");
        assert!(
            original.offset_in_symbol.is_some(),
            "Should have offset_in_symbol"
        );

        // Clear, restore, resolve, re-serialize
        clear_bookmarks(&project, cx);
        restore_bookmarks(&project, serialized, cx).await;
        trigger_resolution(&project, &buffer, cx);

        let re_serialized = get_all_bookmarks(&project, cx);
        let round_tripped = &re_serialized[&project_path(path!("/project/main.rs"))][0];
        assert_eq!(round_tripped.row, original.row);
        assert_eq!(round_tripped.symbol_path, original.symbol_path);
        assert_eq!(round_tripped.offset_in_symbol, original.offset_in_symbol);
    }

    #[gpui::test]
    async fn test_syntactic_resolve_deferred_until_reparse(cx: &mut TestAppContext) {
        let rust_source =
            "fn preamble() {\n    let a = 0;\n}\n\nfn target() {\n    let x = 1;\n}\n";
        let (project, _fs, buffer) = setup_rust_project(rust_source, "main.rs", cx).await;

        let (target_path, _) = compute_syntactic_context(&buffer, 5, cx);
        let target_symbol_text = target_path.expect("fn target should produce a symbol path");
        assert!(
            target_symbol_text.iter().any(|s| s.contains("target")),
            "Expected symbol path containing 'target', got: {:?}",
            target_symbol_text
        );

        // Restore with wrong fallback row (0); symbol path should resolve to row 5.
        clear_bookmarks(&project, cx);
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(
                0,
                Some(target_symbol_text.iter().map(|s| s.as_str()).collect()),
                Some(1),
            )],
        )]);
        restore_bookmarks(&project, serialized, cx).await;
        trigger_resolution(&project, &buffer, cx);

        let after = get_all_bookmarks(&project, cx);
        assert_bookmark_rows(&after, path!("/project/main.rs"), &[5]);
    }

    #[gpui::test]
    async fn test_syntactic_resolve_deferred_via_reparse_event(cx: &mut TestAppContext) {
        let rust_source =
            "fn preamble() {\n    let a = 0;\n}\n\nfn target() {\n    let x = 1;\n}\n";
        let (project, _fs, buffer) = setup_rust_project(rust_source, "main.rs", cx).await;

        let (target_path, _) = compute_syntactic_context(&buffer, 5, cx);
        let target_symbol_text = target_path.expect("fn target should have symbol path");

        // Restore with wrong fallback row and resolve immediately
        clear_bookmarks(&project, cx);
        let serialized = build_syntactic_serialized(&[(
            path!("/project/main.rs"),
            &[(
                0,
                Some(target_symbol_text.iter().map(|s| s.as_str()).collect()),
                Some(1),
            )],
        )]);
        restore_bookmarks(&project, serialized, cx).await;
        trigger_resolution(&project, &buffer, cx);

        assert_bookmark_rows(
            &get_all_bookmarks(&project, cx),
            path!("/project/main.rs"),
            &[5],
        );

        // Verify the Reparsed event path: edit shifts bookmark row 5 -> 6
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(0..0, "// comment\n")], None, cx);
        });
        cx.executor().run_until_parked();

        assert_bookmark_rows(
            &get_all_bookmarks(&project, cx),
            path!("/project/main.rs"),
            &[6],
        );
    }

    #[gpui::test]
    async fn test_bookmark_outside_any_symbol_has_no_syntactic_context(cx: &mut TestAppContext) {
        // Row 3 is a blank line between two functions
        let (project, _fs, buffer) =
            setup_rust_project("fn first() {\n}\n\n\nfn second() {\n}\n", "main.rs", cx).await;

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

    #[gpui::test]
    async fn test_serialized_bookmarks_include_context_snippet(cx: &mut TestAppContext) {
        let rust_source = "fn alpha() {\n    let a = 1;\n}\n\nfn beta() {\n    let b = 2;\n}\n";
        let (project, _fs, buffer) = setup_rust_project(rust_source, "main.rs", cx).await;

        // Row 1 = "    let a = 1;" inside fn alpha
        // Row 5 = "    let b = 2;" inside fn beta
        add_bookmarks(&project, &buffer, &[1, 5], cx);

        let bookmarks = get_all_bookmarks(&project, cx);
        let file_bookmarks = bookmarks
            .get(&project_path(path!("/project/main.rs")))
            .expect("Expected bookmarks for main.rs");
        assert_eq!(file_bookmarks.len(), 2);

        let snippet_a = file_bookmarks[0]
            .context_snippet
            .as_ref()
            .expect("Bookmark at row 1 should have a context_snippet");
        assert!(
            snippet_a.contains("let a"),
            "Expected snippet to contain 'let a', got: {:?}",
            snippet_a
        );
        assert!(
            snippet_a.len() <= 30,
            "Snippet should be at most 30 characters, got {} chars",
            snippet_a.len()
        );

        let snippet_b = file_bookmarks[1]
            .context_snippet
            .as_ref()
            .expect("Bookmark at row 5 should have a context_snippet");
        assert!(
            snippet_b.contains("let b"),
            "Expected snippet to contain 'let b', got: {:?}",
            snippet_b
        );
        assert!(
            snippet_b.len() <= 30,
            "Snippet should be at most 30 characters, got {} chars",
            snippet_b.len()
        );
    }

    fn build_syntactic_serialized_with_snippets(
        entries: &[(&str, &[(u32, Option<Vec<&str>>, Option<u32>, Option<&str>)])],
    ) -> BTreeMap<Arc<Path>, Vec<SerializedBookmark>> {
        let mut map = BTreeMap::new();
        for (path_str, bookmarks) in entries {
            let path = project_path(path_str);
            map.insert(
                path,
                bookmarks
                    .iter()
                    .map(|(row, sym, offset, snippet)| SerializedBookmark {
                        row: *row,
                        symbol_path: sym
                            .as_ref()
                            .map(|v| v.iter().map(|s| s.to_string()).collect()),
                        offset_in_symbol: *offset,
                        context_snippet: snippet.map(|s| s.to_string()),
                    })
                    .collect(),
            );
        }
        map
    }

    #[gpui::test]
    async fn test_context_snippet_disambiguates_duplicate_symbol_paths(cx: &mut TestAppContext) {
        // Two `fn process()` definitions in the same file via #[cfg] attributes.
        // Tree-sitter produces outline items for both, each with text "fn process".
        let rust_source = "\
#[cfg(feature = \"a\")]\n\
fn process() {\n\
    let config_a = true;\n\
}\n\
\n\
#[cfg(feature = \"b\")]\n\
fn process() {\n\
    let config_b = true;\n\
}\n";

        let (project, _fs, buffer) = setup_rust_project(rust_source, "main.rs", cx).await;

        // Verify both fn process appear in the outline with the same text
        let outline_items = buffer.read_with(cx, |buffer, _cx| {
            let snapshot = buffer.snapshot();
            let outline = snapshot.outline(None);
            outline.items
        });
        let process_items: Vec<_> = outline_items
            .iter()
            .filter(|item| item.text.contains("process"))
            .collect();
        assert!(
            process_items.len() >= 2,
            "Expected at least 2 'process' outline items, got {}: {:?}",
            process_items.len(),
            process_items.iter().map(|i| &i.text).collect::<Vec<_>>()
        );

        // Get the outline text used for both (they should be identical)
        let process_outline_text = &process_items[0].text;
        assert_eq!(
            process_outline_text, &process_items[1].text,
            "Both fn process items should have the same outline text"
        );

        // Add a bookmark inside the SECOND fn process (row 7 = "let config_b = true;")
        add_bookmarks(&project, &buffer, &[7], cx);

        // Serialize and verify the context_snippet is populated
        let serialized = get_all_bookmarks(&project, cx);
        let file_bookmarks = &serialized[&project_path(path!("/project/main.rs"))];
        assert_eq!(file_bookmarks.len(), 1);
        assert_eq!(file_bookmarks[0].row, 7);
        let original_snippet = file_bookmarks[0]
            .context_snippet
            .as_ref()
            .expect("Bookmark should have context_snippet");
        assert!(
            original_snippet.contains("config_b"),
            "Expected snippet for row 7 to contain 'config_b', got: {:?}",
            original_snippet
        );

        // Now simulate restoring: construct a serialized bookmark targeting
        // the second fn process with the correct context_snippet but a wrong
        // fallback row (row 0). Both fn process share the same symbol path,
        // so the snippet should disambiguate.
        let symbol_path = file_bookmarks[0]
            .symbol_path
            .as_ref()
            .expect("Should have symbol_path");

        let serialized_with_snippet = build_syntactic_serialized_with_snippets(&[(
            path!("/project/main.rs"),
            &[(
                0, // wrong fallback row
                Some(symbol_path.iter().map(|s| s.as_str()).collect()),
                Some(1), // offset within symbol
                Some(original_snippet.as_str()),
            )],
        )]);

        clear_bookmarks(&project, cx);
        restore_bookmarks(&project, serialized_with_snippet, cx).await;
        trigger_resolution(&project, &buffer, cx);

        let restored = get_all_bookmarks(&project, cx);
        let restored_bookmarks = &restored[&project_path(path!("/project/main.rs"))];
        assert_eq!(restored_bookmarks.len(), 1);

        // The bookmark should resolve to row 7 (inside the SECOND fn process),
        // not row 2 (inside the first fn process), because the snippet matched.
        assert_eq!(
            restored_bookmarks[0].row, 7,
            "Snippet should disambiguate to the second fn process (row 7), not the first (row 2)"
        );

        // Now verify that WITHOUT a snippet, it would pick the first match.
        let serialized_without_snippet = build_syntactic_serialized_with_snippets(&[(
            path!("/project/main.rs"),
            &[(
                0,
                Some(symbol_path.iter().map(|s| s.as_str()).collect()),
                Some(1),
                None, // no snippet
            )],
        )]);

        clear_bookmarks(&project, cx);
        restore_bookmarks(&project, serialized_without_snippet, cx).await;
        trigger_resolution(&project, &buffer, cx);

        let restored_no_snippet = get_all_bookmarks(&project, cx);
        let restored_no_snippet_bookmarks =
            &restored_no_snippet[&project_path(path!("/project/main.rs"))];
        assert_eq!(restored_no_snippet_bookmarks.len(), 1);

        // Without snippet, should fall back to first match (row 2 inside first fn process)
        assert_eq!(
            restored_no_snippet_bookmarks[0].row, 2,
            "Without snippet, should resolve to first match (row 2)"
        );
    }
}
