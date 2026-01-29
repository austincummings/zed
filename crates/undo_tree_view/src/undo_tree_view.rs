use editor::Editor;
use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Window,
    actions,
};
use picker::{Picker, PickerDelegate};
use ui::{IntoElement, ListItem, ParentElement, Render, Styled as _, rems, v_flex, vh};
use undo_tree::UndoTree;
use workspace::{DismissDecision, ModalView, Workspace};

actions!(undo_tree_view, [Toggle]);

pub fn init(cx: &mut App) {
    cx.observe_new(UndoTreeView::register).detach();
}

pub fn toggle(editor: Entity<Editor>, _: &Toggle, window: &mut Window, cx: &mut App) {
    let undo_tree = {
        let editor = editor.read(cx);
        let multi_buffer = editor.buffer().read(cx);
        // TODO: Handle multibuffers
        let Some(buffer_handle) = multi_buffer.as_singleton() else {
            return;
        };

        let buffer = buffer_handle.read(cx);
        buffer.history().undo_tree().clone()
    };

    println!("{:?}", undo_tree);

    let workspace = window.root::<Workspace>().flatten();
    if let Some(workspace) = workspace {
        workspace.update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, |window, cx| {
                UndoTreeView::new(undo_tree, editor, window, cx)
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DismissDecision {
        // self.picker.update(cx, |picker, cx| {
        //     picker.delegate.restore_active_editor(window, cx)
        // });
        DismissDecision::Dismiss(true)
    }
}

impl Render for UndoTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().w(rems(34.)).child(self.picker.clone())
    }
}

impl UndoTreeView {
    fn register(editor: &mut Editor, _window: Option<&mut Window>, cx: &mut Context<Editor>) {
        println!("Registering undo tree view");
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
        editor: Entity<Editor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = UndoTreeViewDelegate::new();
        let picker = cx.new(|cx| {
            Picker::uniform_list(delegate, window, cx)
                .max_height(Some(vh(0.75, window)))
                .show_scrollbar(true)
        });
        UndoTreeView { picker }
    }
}

struct UndoTreeViewDelegate {}

impl UndoTreeViewDelegate {
    fn new() -> Self {
        Self {}
    }
}

impl PickerDelegate for UndoTreeViewDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        4
    }

    fn selected_index(&self) -> usize {
        0
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        // todo!()
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> std::sync::Arc<str> {
        "Search undo history".into()
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> gpui::Task<()> {
        // todo!()
        cx.spawn(async move |_, _| {})
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        // todo!()
    }

    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        // todo!()
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        // todo!()
        None
    }
}
