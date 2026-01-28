use editor::Editor;
use gpui::{App, Context, Entity, Window, actions};

actions!(undo_tree_view, [Toggle]);

pub fn init(cx: &mut App) {
    cx.observe_new(UndoTreeView::register).detach();
}

pub struct UndoTreeView;

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
}

pub fn toggle(editor: Entity<Editor>, _: &Toggle, window: &mut Window, cx: &mut App) {
    let multi_buffer = editor.read(cx).buffer().read(cx);

    // TODO: Handle multibuffers
    let Some(buffer_handle) = multi_buffer.as_singleton() else {
        return;
    };

    let buffer = buffer_handle.read(cx);
    let undo_tree = buffer.history().undo_tree();

    println!("{:?}", undo_tree);
}
