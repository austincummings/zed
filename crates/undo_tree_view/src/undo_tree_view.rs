use std::time::Instant;

use editor::Editor;
use gpui::{
    App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, Task, WeakEntity, Window, actions,
};
use language::Buffer;
use picker::{Picker, PickerDelegate};
use ui::{
    Color, Icon, IconName, IntoElement, Label, LabelCommon, ListItem, ListItemSpacing,
    ParentElement, Render, Styled as _, Toggleable, rems, v_flex, vh,
};
use undo_tree::{TransactionId, UndoTree};

use workspace::{DismissDecision, ModalView, Workspace};

actions!(undo_tree_view, [Toggle]);

pub fn init(cx: &mut App) {
    cx.observe_new(UndoTreeView::register).detach();
}

pub fn toggle(editor: Entity<Editor>, _: &Toggle, window: &mut Window, cx: &mut App) {
    let (undo_tree, buffer) = {
        let editor = editor.read(cx);
        let multi_buffer = editor.buffer().read(cx);
        // TODO: Handle multibuffers
        let Some(buffer_handle) = multi_buffer.as_singleton() else {
            return;
        };

        let buffer = buffer_handle.read(cx);
        (buffer.history().undo_tree().clone(), buffer_handle.clone())
    };

    let workspace = window.root::<Workspace>().flatten();
    if let Some(workspace) = workspace {
        workspace.update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, |window, cx| {
                UndoTreeView::new(undo_tree, buffer, window, cx)
            });
        })
    }
}

pub struct UndoTreeView {
    picker: Entity<Picker<UndoTreeViewDelegate>>,
}

impl Focusable for UndoTreeView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for UndoTreeView {}
impl ModalView for UndoTreeView {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> DismissDecision {
        DismissDecision::Dismiss(true)
    }
}

impl Render for UndoTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(rems(34.))
            .on_action(cx.listener(
                |_this: &mut UndoTreeView,
                 _: &Toggle,
                 _window: &mut Window,
                 cx: &mut Context<UndoTreeView>| {
                    cx.emit(DismissEvent);
                },
            ))
            .child(self.picker.clone())
    }
}

impl UndoTreeView {
    fn register(editor: &mut Editor, _window: Option<&mut Window>, cx: &mut Context<Editor>) {
        if editor.mode().is_full() {
            let handle = cx.entity().downgrade();
            editor
                .register_action(move |action: &Toggle, window, cx| {
                    if let Some(editor) = handle.upgrade() {
                        toggle(editor, action, window, cx);
                    }
                })
                .detach();
        }
    }

    fn new(
        undo_tree: UndoTree,
        buffer: Entity<Buffer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = UndoTreeViewDelegate::new(undo_tree, buffer, cx);
        let picker = cx.new(|cx| {
            Picker::uniform_list(delegate, window, cx)
                .max_height(Some(vh(0.75, window)))
                .show_scrollbar(true)
        });
        UndoTreeView { picker }
    }
}

#[derive(Clone)]
struct TreeEntry {
    id: TransactionId,
    depth: usize,
    is_current: bool,
    is_branch_point: bool,
    child_index: usize,
    sibling_count: usize,
    timestamp: Option<Instant>,
}

struct UndoTreeViewDelegate {
    undo_tree: UndoTree,
    buffer: WeakEntity<Buffer>,
    entries: Vec<TreeEntry>,
    selected_index: usize,
}

impl UndoTreeViewDelegate {
    fn new(undo_tree: UndoTree, buffer: Entity<Buffer>, _cx: &mut Context<UndoTreeView>) -> Self {
        let entries = Self::build_entries(&undo_tree);
        let current = undo_tree.current();

        // Find the index of the current transaction
        let selected_index = entries
            .iter()
            .position(|e| Some(e.id) == current)
            .unwrap_or(entries.len().saturating_sub(1));

        Self {
            undo_tree,
            buffer: buffer.downgrade(),
            entries,
            selected_index,
        }
    }

    fn build_entries(undo_tree: &UndoTree) -> Vec<TreeEntry> {
        let mut entries = Vec::new();
        let current = undo_tree.current();

        // Find all root nodes (nodes with no parent)
        let path_to_current = undo_tree.cursor().path_from_root();

        // If tree is empty, return empty entries
        if path_to_current.is_empty() {
            return entries;
        }

        // Start from the first node in the path (root)
        let root = path_to_current[0];
        Self::build_entries_recursive(undo_tree, root, 0, current, &mut entries, 0, 1);

        entries
    }

    fn build_entries_recursive(
        undo_tree: &UndoTree,
        node: TransactionId,
        depth: usize,
        current: Option<TransactionId>,
        entries: &mut Vec<TreeEntry>,
        child_index: usize,
        sibling_count: usize,
    ) {
        let cursor = undo_tree.cursor_at(Some(node));
        let children = cursor.children();
        let is_branch_point = children.len() > 1;
        let timestamp = cursor.timestamp();

        entries.push(TreeEntry {
            id: node,
            depth,
            is_current: Some(node) == current,
            is_branch_point,
            child_index,
            sibling_count,
            timestamp,
        });

        let child_count = children.len();
        for (index, child) in children.iter().copied().enumerate() {
            Self::build_entries_recursive(
                undo_tree,
                child,
                depth + 1,
                current,
                entries,
                index,
                child_count,
            );
        }
    }
}

impl PickerDelegate for UndoTreeViewDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.entries.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> std::sync::Arc<str> {
        "Navigate undo history".into()
    }

    fn update_matches(
        &mut self,
        _query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        // For now, we don't filter - just show all entries
        // Future: could filter by transaction ID or timestamp
        cx.spawn(async move |_, _| {})
    }

    fn confirm(&mut self, _secondary: bool, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(entry) = self.entries.get(self.selected_index) {
            let target_id = entry.id;

            if let Some(buffer) = self.buffer.upgrade() {
                buffer.update(cx, |buffer: &mut Buffer, cx| {
                    buffer.goto_undo_tree_transaction(target_id, cx);
                });
            }

            // Update our local tree state
            self.undo_tree.navigate_to(Some(target_id));

            // Rebuild entries to reflect new current position
            self.entries = Self::build_entries(&self.undo_tree);

            // Keep selection on the same transaction
            self.selected_index = self
                .entries
                .iter()
                .position(|e| e.id == target_id)
                .unwrap_or(0);
        }

        // Dismiss the modal
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let entry = self.entries.get(ix)?;

        // Build the indentation prefix
        let indent = "  ".repeat(entry.depth);

        // Build the tree connector character
        let connector = if entry.depth == 0 {
            ""
        } else if entry.child_index == entry.sibling_count - 1 {
            "└─ "
        } else {
            "├─ "
        };

        // Build the label text
        let marker = if entry.is_current { "●" } else { "○" };
        let branch_indicator = if entry.is_branch_point { " ⑂" } else { "" };
        let timestamp = if let Some(timestamp) = entry.timestamp {
            format_relative_time(timestamp)
        } else {
            "".to_string()
        };
        let label_text = format!(
            "{}{}{} #{}{} {}",
            indent, connector, marker, entry.id.value, branch_indicator, timestamp
        );

        let label = if entry.is_current {
            Label::new(label_text).color(Color::Accent)
        } else {
            Label::new(label_text)
        };

        let mut item = ListItem::new(ix)
            .inset(true)
            .spacing(ListItemSpacing::Sparse)
            .toggle_state(selected)
            .child(label);

        if entry.is_current {
            item = item.end_slot(
                Icon::new(IconName::Check)
                    .size(ui::IconSize::Small)
                    .color(Color::Accent),
            );
        }

        Some(item)
    }
}

fn format_relative_time(instant: Instant) -> String {
    let elapsed = instant.elapsed();
    if elapsed.as_secs() < 60 {
        format!("{}s ago", elapsed.as_secs())
    } else if elapsed.as_secs() < 3600 {
        format!("{}m ago", elapsed.as_secs() / 60)
    } else {
        format!("{}h ago", elapsed.as_secs() / 3600)
    }
}
