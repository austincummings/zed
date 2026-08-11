#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiElement {
    Stack {
        axis: UiAxis,
        gap: f32,
    },
    Label {
        text: String,
    },
    Button {
        label: String,
        handler_id: String,
        disabled: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiNode {
    pub id: Option<String>,
    pub element: UiElement,
    pub children: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiScene {
    pub roots: Vec<u32>,
    pub nodes: Vec<UiNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiEvent {
    pub handler_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiSceneUpdate {
    Unchanged,
    Replace(UiScene),
}
