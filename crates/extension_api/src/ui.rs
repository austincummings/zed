use crate::wit;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    #[default]
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    Stack {
        axis: Axis,
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
pub struct Node {
    pub id: Option<String>,
    pub element: Element,
    pub children: Vec<Node>,
}

impl Node {
    pub fn column(children: impl IntoIterator<Item = Node>) -> Self {
        Self::stack(Axis::Vertical, children)
    }

    pub fn row(children: impl IntoIterator<Item = Node>) -> Self {
        Self::stack(Axis::Horizontal, children)
    }

    fn stack(axis: Axis, children: impl IntoIterator<Item = Node>) -> Self {
        Self {
            id: None,
            element: Element::Stack { axis, gap: 0.0 },
            children: children.into_iter().collect(),
        }
    }

    pub fn label(text: impl Into<String>) -> Self {
        Self {
            id: None,
            element: Element::Label { text: text.into() },
            children: Vec::new(),
        }
    }

    pub fn button(
        id: impl Into<String>,
        label: impl Into<String>,
        handler_id: impl Into<String>,
    ) -> Self {
        Self {
            id: Some(id.into()),
            element: Element::Button {
                label: label.into(),
                handler_id: handler_id.into(),
                disabled: false,
            },
            children: Vec::new(),
        }
    }

    pub fn gap(mut self, gap: f32) -> Self {
        if let Element::Stack { gap: stack_gap, .. } = &mut self.element {
            *stack_gap = gap;
        }
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        if let Element::Button {
            disabled: button_disabled,
            ..
        } = &mut self.element
        {
            *button_disabled = disabled;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub roots: Vec<Node>,
}

impl Scene {
    pub fn new(root: Node) -> Self {
        Self { roots: vec![root] }
    }

    pub fn from_roots(roots: impl IntoIterator<Item = Node>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
        }
    }

    pub(crate) fn into_wit(self) -> Result<wit::zed::extension::ui::Scene, String> {
        let mut nodes = Vec::new();
        let mut roots = Vec::with_capacity(self.roots.len());
        for root in self.roots {
            roots.push(flatten_node(root, &mut nodes)?);
        }
        Ok(wit::zed::extension::ui::Scene { roots, nodes })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub handler_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneUpdate {
    Unchanged,
    Replace(Scene),
}

impl SceneUpdate {
    pub(crate) fn into_wit(self) -> Result<wit::zed::extension::ui::SceneUpdate, String> {
        Ok(match self {
            Self::Unchanged => wit::zed::extension::ui::SceneUpdate::Unchanged,
            Self::Replace(scene) => {
                wit::zed::extension::ui::SceneUpdate::Replace(scene.into_wit()?)
            }
        })
    }
}

fn flatten_node(node: Node, nodes: &mut Vec<wit::zed::extension::ui::Node>) -> Result<u32, String> {
    let index = u32::try_from(nodes.len()).map_err(|_| "UI scene contains too many nodes")?;
    nodes.push(wit::zed::extension::ui::Node {
        id: node.id,
        element: element_into_wit(node.element),
        children: Vec::new(),
    });

    let mut children = Vec::with_capacity(node.children.len());
    for child in node.children {
        children.push(flatten_node(child, nodes)?);
    }
    nodes[index as usize].children = children;
    Ok(index)
}

fn element_into_wit(element: Element) -> wit::zed::extension::ui::Element {
    use wit::zed::extension::ui;

    match element {
        Element::Stack { axis, gap } => ui::Element::Stack(ui::Stack {
            axis: match axis {
                Axis::Horizontal => ui::Axis::Horizontal,
                Axis::Vertical => ui::Axis::Vertical,
            },
            gap,
        }),
        Element::Label { text } => ui::Element::Label(ui::Label { text }),
        Element::Button {
            label,
            handler_id,
            disabled,
        } => ui::Element::Button(ui::Button {
            label,
            handler_id,
            disabled,
        }),
    }
}

#[macro_export]
macro_rules! ui_scene {
    ($root:expr $(,)?) => {
        $crate::ui::Scene::new($root)
    };
    ($first:expr, $($root:expr),+ $(,)?) => {
        $crate::ui::Scene::from_roots([$first, $($root),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_scene_into_preorder_arena() {
        let scene = Scene::new(
            Node::column([Node::label("one"), Node::row([Node::label("two")]).gap(4.0)]).gap(8.0),
        )
        .into_wit()
        .expect("scene should flatten");

        assert_eq!(scene.roots, vec![0]);
        assert_eq!(scene.nodes.len(), 4);
        assert_eq!(scene.nodes[0].children, vec![1, 2]);
        assert_eq!(scene.nodes[2].children, vec![3]);
    }
}
