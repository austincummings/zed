use crate::AnyAgentTool;
use collections::BTreeMap;
use gpui::{App, Context, EventEmitter, SharedString};
use std::sync::Arc;

/// Where a runtime-registered tool came from.
///
/// Mirrors pi's `sourceInfo.source` provenance field. Built-in tools live
/// directly on [`crate::Thread`] and MCP tools live in
/// [`crate::ContextServerRegistry`]; this enum only describes the sources that
/// register through the [`RuntimeToolRegistry`] (Source 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolSource {
    /// Registered by in-process Rust: internal features, lifecycle gates, or
    /// tests. This is the Layer 1 caller.
    InProcess,
    /// Contributed by a WASM extension (Layer 2). Carried so the whole set can
    /// be dropped when the extension unloads, mirroring
    /// `unregister_context_server`.
    Extension { extension_id: Arc<str> },
    /// Contributed by an external ACP/SDK client.
    Sdk { client: Arc<str> },
}

/// A tool registered at runtime, plus its provenance.
pub struct RegisteredTool {
    pub tool: Arc<dyn AnyAgentTool>,
    pub source: ToolSource,
}

pub enum RuntimeToolRegistryEvent {
    /// The set of registered tools changed. [`crate::Thread`] subscribes and
    /// calls `refresh_turn_tools` so a mid-turn registration is picked up, the
    /// same way MCP's `tools/list_changed` is handled.
    ToolsChanged,
}

/// Source 3 of the agent's tools: tools registered dynamically at runtime,
/// alongside built-ins ([`crate::Thread::tools`]) and MCP
/// ([`crate::ContextServerRegistry`]).
///
/// This is the in-process (Layer 1) foundation. The WASM extension path
/// (Layer 2) is just another caller of [`RuntimeToolRegistry::register`]: the
/// extension host wraps a WASM tool as an [`AnyAgentTool`] and registers it
/// with `ToolSource::Extension`.
#[derive(Default)]
pub struct RuntimeToolRegistry {
    tools: BTreeMap<SharedString, RegisteredTool>,
}

impl EventEmitter<RuntimeToolRegistryEvent> for RuntimeToolRegistry {}

impl RuntimeToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a tool under its own [`AnyAgentTool::name`].
    ///
    /// Returns the registered name. Emits [`RuntimeToolRegistryEvent::ToolsChanged`].
    /// Note: name collisions with built-ins are resolved at the merge points in
    /// `Thread`; this registry does not itself namespace or reject names.
    pub fn register(
        &mut self,
        tool: Arc<dyn AnyAgentTool>,
        source: ToolSource,
        cx: &mut Context<Self>,
    ) -> SharedString {
        let name = tool.name();
        self.tools
            .insert(name.clone(), RegisteredTool { tool, source });
        cx.emit(RuntimeToolRegistryEvent::ToolsChanged);
        cx.notify();
        name
    }

    /// Remove a single tool by name. Returns whether a tool was removed.
    pub fn unregister(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        let removed = self.tools.remove(name).is_some();
        if removed {
            cx.emit(RuntimeToolRegistryEvent::ToolsChanged);
            cx.notify();
        }
        removed
    }

    /// Remove every tool contributed by `source` — e.g. when an extension
    /// unloads. Returns the number removed.
    pub fn unregister_source(&mut self, source: &ToolSource, cx: &mut Context<Self>) -> usize {
        let before = self.tools.len();
        self.tools.retain(|_, registered| &registered.source != source);
        let removed = before - self.tools.len();
        if removed > 0 {
            cx.emit(RuntimeToolRegistryEvent::ToolsChanged);
            cx.notify();
        }
        removed
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn AnyAgentTool>> {
        self.tools.get(name).map(|registered| &registered.tool)
    }

    /// Provenance of a registered tool, if present.
    pub fn source_of(&self, name: &str) -> Option<&ToolSource> {
        self.tools.get(name).map(|registered| &registered.source)
    }

    /// Iterate `(name, tool)` for all registered tools, in name order.
    pub fn tools(&self) -> impl Iterator<Item = (&SharedString, &Arc<dyn AnyAgentTool>)> {
        self.tools
            .iter()
            .map(|(name, registered)| (name, &registered.tool))
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentToolOutput, ToolCallEventStream, ToolInput};
    use agent_client_protocol::schema::v1 as acp;
    use anyhow::Result;
    use gpui::{AppContext, Task, TestAppContext};
    use language_model::{LanguageModelProviderId, LanguageModelToolSchemaFormat};

    /// Minimal [`AnyAgentTool`] whose `run`/`replay` are inert; only its
    /// identity matters for registry bookkeeping tests.
    struct TestTool {
        name: SharedString,
    }

    impl TestTool {
        fn erased(name: &str) -> Arc<dyn AnyAgentTool> {
            Arc::new(TestTool { name: name.into() })
        }
    }

    impl AnyAgentTool for TestTool {
        fn name(&self) -> SharedString {
            self.name.clone()
        }
        fn description(&self) -> SharedString {
            "test tool".into()
        }
        fn kind(&self) -> acp::ToolKind {
            acp::ToolKind::Other
        }
        fn initial_title(&self, _input: serde_json::Value, _cx: &mut App) -> SharedString {
            self.name.clone()
        }
        fn input_schema(
            &self,
            _format: LanguageModelToolSchemaFormat,
        ) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "type": "object", "properties": {} }))
        }
        fn supports_provider(&self, _provider: &LanguageModelProviderId) -> bool {
            true
        }
        fn run(
            self: Arc<Self>,
            _input: ToolInput<serde_json::Value>,
            _event_stream: ToolCallEventStream,
            _cx: &mut App,
        ) -> Task<std::result::Result<AgentToolOutput, AgentToolOutput>> {
            Task::ready(Ok(AgentToolOutput {
                llm_output: Vec::new(),
                raw_output: serde_json::Value::Null,
            }))
        }
        fn replay(
            &self,
            _input: serde_json::Value,
            _output: serde_json::Value,
            _event_stream: ToolCallEventStream,
            _cx: &mut App,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[gpui::test]
    fn test_register_get_unregister(cx: &mut TestAppContext) {
        let registry = cx.new(|_| RuntimeToolRegistry::new());
        registry.update(cx, |registry, cx| {
            assert!(registry.is_empty());

            let name = registry.register(TestTool::erased("greet"), ToolSource::InProcess, cx);
            assert_eq!(name.as_ref(), "greet");
            assert_eq!(registry.len(), 1);
            assert!(registry.get("greet").is_some());
            assert_eq!(registry.source_of("greet"), Some(&ToolSource::InProcess));

            assert!(registry.unregister("greet", cx));
            assert!(!registry.unregister("greet", cx));
            assert!(registry.is_empty());
        });
    }

    #[gpui::test]
    fn test_register_replaces_by_name(cx: &mut TestAppContext) {
        let registry = cx.new(|_| RuntimeToolRegistry::new());
        registry.update(cx, |registry, cx| {
            registry.register(TestTool::erased("dup"), ToolSource::InProcess, cx);
            registry.register(
                TestTool::erased("dup"),
                ToolSource::Extension {
                    extension_id: "ext-a".into(),
                },
                cx,
            );
            // Same name registered twice collapses to one entry, last wins.
            assert_eq!(registry.len(), 1);
            assert_eq!(
                registry.source_of("dup"),
                Some(&ToolSource::Extension {
                    extension_id: "ext-a".into()
                })
            );
        });
    }

    #[gpui::test]
    fn test_unregister_source(cx: &mut TestAppContext) {
        let registry = cx.new(|_| RuntimeToolRegistry::new());
        registry.update(cx, |registry, cx| {
            let ext = ToolSource::Extension {
                extension_id: "ext-a".into(),
            };
            registry.register(TestTool::erased("a1"), ext.clone(), cx);
            registry.register(TestTool::erased("a2"), ext.clone(), cx);
            registry.register(TestTool::erased("keep"), ToolSource::InProcess, cx);

            assert_eq!(registry.unregister_source(&ext, cx), 2);
            assert_eq!(registry.len(), 1);
            assert!(registry.get("keep").is_some());
            // Removing a source with nothing left is a no-op.
            assert_eq!(registry.unregister_source(&ext, cx), 0);
        });
    }
}
