use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Result, anyhow, bail};
use extension::{UiAxis, UiElement, UiEvent, UiScene, UiSceneUpdate};
use extension_host::ExtensionStore;
use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Task, WeakEntity, Window, px,
};
use schemars::JsonSchema;
use serde::Deserialize;
use ui::prelude::*;
use util::ResultExt;
use workspace::{
    Workspace,
    item::{Item, ItemEvent},
};

static NEXT_VIEW_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
const MAX_SCENE_NODES: usize = 2_048;
const MAX_SCENE_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema, Action)]
#[action(namespace = extension, name = "OpenView")]
#[serde(deny_unknown_fields)]
pub struct OpenExtensionView {
    pub extension_id: String,
    pub view_id: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, _cx| {
        if window.is_none() {
            return;
        }

        workspace.register_action(|workspace, action: &OpenExtensionView, window, cx| {
            let store = ExtensionStore::global(cx);
            let Some((extension, extension_name)) = store.read_with(cx, |store, _cx| {
                let extension = store.wasm_extension_for_id(&action.extension_id)?;
                let name = store
                    .extension_manifest_for_id(&action.extension_id)?
                    .name
                    .clone();
                Some((extension, name))
            }) else {
                workspace.show_error(
                    format!("Extension '{}' is not loaded", action.extension_id),
                    cx,
                );
                return;
            };

            let title = action
                .title
                .clone()
                .unwrap_or_else(|| format!("{extension_name}: {}", action.view_id));
            let view = ExtensionView::new(
                action.view_id.clone().into(),
                title.into(),
                Arc::new(extension),
                cx,
            );
            workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
        });
    })
    .detach();
}

struct ExtensionView {
    view_id: Arc<str>,
    instance_id: u64,
    title: SharedString,
    extension: Arc<dyn extension::Extension>,
    scene: Option<UiScene>,
    error: Option<SharedString>,
    event_pending: bool,
    focus_handle: FocusHandle,
    _load_task: Task<()>,
    event_task: Option<Task<()>>,
}

impl ExtensionView {
    fn new(
        view_id: Arc<str>,
        title: SharedString,
        extension: Arc<dyn extension::Extension>,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let instance_id = NEXT_VIEW_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
            let load_task = cx.spawn({
                let extension = extension.clone();
                let view_id = view_id.clone();
                async move |this: WeakEntity<Self>, cx| {
                    let scene = extension.render_ui_view(view_id, instance_id).await;
                    this.update(cx, |this, cx| {
                        this.set_scene_result(scene, cx);
                    })
                    .log_err();
                }
            });

            Self {
                view_id,
                instance_id,
                title,
                extension,
                scene: None,
                error: None,
                event_pending: false,
                focus_handle: cx.focus_handle(),
                _load_task: load_task,
                event_task: None,
            }
        })
    }

    fn set_scene_result(&mut self, scene: Result<UiScene>, cx: &mut Context<Self>) {
        match scene.and_then(|scene| {
            validate_scene(&scene)?;
            Ok(scene)
        }) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("{error:#}").into());
            }
        }
        self.event_pending = false;
        cx.notify();
    }

    fn dispatch_event(&mut self, handler_id: String, cx: &mut Context<Self>) {
        if self.event_pending {
            return;
        }

        self.event_pending = true;
        cx.notify();
        self.event_task = Some(cx.spawn({
            let extension = self.extension.clone();
            let view_id = self.view_id.clone();
            let instance_id = self.instance_id;
            async move |this, cx| {
                let update = extension
                    .handle_ui_view_event(view_id, instance_id, UiEvent { handler_id })
                    .await;
                this.update(cx, |this, cx| match update {
                    Ok(UiSceneUpdate::Unchanged) => {
                        this.event_pending = false;
                        cx.notify();
                    }
                    Ok(UiSceneUpdate::Replace(scene)) => {
                        this.set_scene_result(Ok(scene), cx);
                    }
                    Err(error) => {
                        this.event_pending = false;
                        this.error = Some(format!("{error:#}").into());
                        cx.notify();
                    }
                })
                .log_err();
            }
        }));
    }

    fn render_node(&self, index: u32, cx: &mut Context<Self>) -> AnyElement {
        let Some(node) = self
            .scene
            .as_ref()
            .and_then(|scene| scene.nodes.get(index as usize))
            .cloned()
        else {
            return Label::new("Invalid extension UI node")
                .color(Color::Error)
                .into_any_element();
        };

        let children = node
            .children
            .iter()
            .map(|child| self.render_node(*child, cx))
            .collect::<Vec<_>>();

        match node.element {
            UiElement::Stack { axis, gap } => match axis {
                UiAxis::Horizontal => h_flex().gap(px(gap)).children(children).into_any_element(),
                UiAxis::Vertical => v_flex().gap(px(gap)).children(children).into_any_element(),
            },
            UiElement::Label { text } => Label::new(text).into_any_element(),
            UiElement::Button {
                label,
                handler_id,
                disabled,
            } => {
                let id = node
                    .id
                    .map(SharedString::from)
                    .unwrap_or_else(|| format!("extension-ui-button-{index}").into());
                Button::new(id, label)
                    .disabled(disabled || self.event_pending)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.dispatch_event(handler_id.clone(), cx);
                    }))
                    .into_any_element()
            }
        }
    }
}

impl Render for ExtensionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(error) = self.error.clone() {
            Label::new(error).color(Color::Error).into_any_element()
        } else if let Some(scene) = &self.scene {
            let roots = scene.roots.clone();
            v_flex()
                .gap_2()
                .children(roots.into_iter().map(|root| self.render_node(root, cx)))
                .into_any_element()
        } else {
            Label::new("Loading extension view…")
                .color(Color::Muted)
                .into_any_element()
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .p_4()
            .bg(cx.theme().colors().editor_background)
            .child(content)
    }
}

impl EventEmitter<ItemEvent> for ExtensionView {}

impl Focusable for ExtensionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ExtensionView {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.title.clone()
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("Extension View Opened")
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        emit(*event)
    }
}

fn validate_scene(scene: &UiScene) -> Result<()> {
    if scene.nodes.len() > MAX_SCENE_NODES {
        bail!(
            "extension UI scene has {} nodes; the maximum is {MAX_SCENE_NODES}",
            scene.nodes.len()
        );
    }

    let mut states = vec![VisitState::Unvisited; scene.nodes.len()];
    let mut ids = HashSet::new();
    for root in &scene.roots {
        validate_node(scene, *root, 0, &mut states, &mut ids)?;
    }

    if states.contains(&VisitState::Unvisited) {
        bail!("extension UI scene contains nodes that are not reachable from a root");
    }

    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited,
    Visiting,
    Visited,
}

fn validate_node(
    scene: &UiScene,
    index: u32,
    depth: usize,
    states: &mut [VisitState],
    ids: &mut HashSet<String>,
) -> Result<()> {
    if depth > MAX_SCENE_DEPTH {
        bail!("extension UI scene exceeds the maximum depth of {MAX_SCENE_DEPTH}");
    }

    let Some(state) = states.get_mut(index as usize) else {
        bail!("extension UI scene references missing node {index}");
    };
    match state {
        VisitState::Visiting => bail!("extension UI scene contains a cycle at node {index}"),
        VisitState::Visited => bail!("extension UI node {index} has more than one parent"),
        VisitState::Unvisited => *state = VisitState::Visiting,
    }

    let node = scene
        .nodes
        .get(index as usize)
        .ok_or_else(|| anyhow!("extension UI scene references missing node {index}"))?;
    if let Some(id) = &node.id
        && !ids.insert(id.clone())
    {
        bail!("extension UI scene contains duplicate node ID '{id}'");
    }

    match &node.element {
        UiElement::Stack { gap, .. } => {
            if !gap.is_finite() || *gap < 0.0 || *gap > 512.0 {
                bail!("extension UI stack at node {index} has invalid gap {gap}");
            }
        }
        UiElement::Label { .. } | UiElement::Button { .. } if !node.children.is_empty() => {
            bail!("extension UI leaf node {index} cannot have children");
        }
        UiElement::Button { handler_id, .. } if handler_id.is_empty() => {
            bail!("extension UI button at node {index} has an empty handler ID");
        }
        UiElement::Label { .. } | UiElement::Button { .. } => {}
    }

    for child in &node.children {
        validate_node(scene, *child, depth + 1, states, ids)?;
    }
    states[index as usize] = VisitState::Visited;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use extension::UiNode;

    #[test]
    fn rejects_scene_cycles() {
        let scene = UiScene {
            roots: vec![0],
            nodes: vec![UiNode {
                id: None,
                element: UiElement::Stack {
                    axis: UiAxis::Vertical,
                    gap: 0.0,
                },
                children: vec![0],
            }],
        };

        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn accepts_valid_scene() {
        let scene = UiScene {
            roots: vec![0],
            nodes: vec![
                UiNode {
                    id: None,
                    element: UiElement::Stack {
                        axis: UiAxis::Vertical,
                        gap: 8.0,
                    },
                    children: vec![1],
                },
                UiNode {
                    id: Some("button".to_string()),
                    element: UiElement::Button {
                        label: "Run".to_string(),
                        handler_id: "run".to_string(),
                        disabled: false,
                    },
                    children: Vec::new(),
                },
            ],
        };

        assert!(validate_scene(&scene).is_ok());
    }
}
