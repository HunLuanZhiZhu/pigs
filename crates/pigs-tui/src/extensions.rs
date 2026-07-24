//! Extension system — lifecycle events, custom tools, slash commands.
//!
//! Provides a trait-based extension API inspired by PI's extension system.
//! Extensions can subscribe to lifecycle events, register custom tools,
//! and add slash commands. Unlike PI's TypeScript extension system (loaded
//! via jiti), Rust extensions are compiled into the binary or loaded as
//! dynamic libraries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Lifecycle events that extensions can subscribe to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LifecycleEvent {
    /// Session started.
    SessionStart,
    /// Session ended.
    SessionEnd,
    /// Agent turn started.
    TurnStart,
    /// Agent turn ended.
    TurnEnd,
    /// Message started (assistant streaming begins).
    MessageStart,
    /// Message updated (streaming delta).
    MessageUpdate,
    /// Message ended (streaming complete).
    MessageEnd,
    /// Tool execution started.
    ToolStart,
    /// Tool execution ended.
    ToolEnd,
    /// Before LLM API request.
    BeforeRequest,
    /// After LLM API response.
    AfterResponse,
    /// Context compaction triggered.
    Compaction,
    /// Config reloaded.
    Reload,
}

/// Context passed to extension callbacks.
#[derive(Debug, Clone)]
pub struct ExtensionContext {
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    pub event: LifecycleEvent,
    /// Optional event payload (e.g., tool name, message text).
    pub payload: Option<String>,
}

/// Result of an extension callback.
#[derive(Debug, Clone, Default)]
pub struct ExtensionResult {
    /// Whether the extension handled the event (prevents further processing).
    pub handled: bool,
    /// Optional message to display to the user.
    pub message: Option<String>,
    /// Optional error.
    pub error: Option<String>,
}

/// Extension trait — implemented by extensions to hook into the agent lifecycle.
pub trait Extension: Send + Sync {
    /// Extension name (unique identifier).
    fn name(&self) -> &str;

    /// Extension description.
    fn description(&self) -> &str {
        ""
    }

    /// Handle a lifecycle event.
    fn on_event(&self, _ctx: &ExtensionContext) -> ExtensionResult {
        ExtensionResult::default()
    }

    /// List of events this extension subscribes to.
    /// If empty, subscribes to all events.
    fn subscribed_events(&self) -> Vec<LifecycleEvent> {
        Vec::new()
    }

    /// Whether this extension is enabled.
    fn is_enabled(&self) -> bool {
        true
    }
}

/// Registry of loaded extensions.
pub struct ExtensionRegistry {
    extensions: Vec<Arc<dyn Extension>>,
    /// Quick lookup by name.
    by_name: HashMap<String, usize>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    /// Register a new extension.
    pub fn register(&mut self, ext: Arc<dyn Extension>) -> bool {
        let name = ext.name().to_string();
        if self.by_name.contains_key(&name) {
            return false; // Already registered
        }
        let idx = self.extensions.len();
        self.by_name.insert(name, idx);
        self.extensions.push(ext);
        true
    }

    /// Unregister an extension by name.
    pub fn unregister(&mut self, name: &str) -> bool {
        if let Some(&idx) = self.by_name.get(name) {
            self.extensions.remove(idx);
            // Rebuild index
            self.by_name.clear();
            for (i, ext) in self.extensions.iter().enumerate() {
                self.by_name.insert(ext.name().to_string(), i);
            }
            true
        } else {
            false
        }
    }

    /// Get an extension by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Extension>> {
        self.by_name.get(name).map(|&idx| &self.extensions[idx])
    }

    /// List all extension names.
    pub fn list(&self) -> Vec<&str> {
        self.extensions.iter().map(|e| e.name()).collect()
    }

    /// Dispatch a lifecycle event to all subscribed extensions.
    pub fn dispatch(&self, ctx: &ExtensionContext) -> Vec<ExtensionResult> {
        let mut results = Vec::new();
        for ext in &self.extensions {
            if !ext.is_enabled() {
                continue;
            }
            let subscribed = ext.subscribed_events();
            if subscribed.is_empty() || subscribed.contains(&ctx.event) {
                let result = ext.on_event(ctx);
                if result.handled {
                    results.push(result);
                    break;
                }
                results.push(result);
            }
        }
        results
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Whether any extensions are registered.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A thread-safe shared extension registry.
pub type SharedRegistry = Arc<Mutex<ExtensionRegistry>>;

/// Create a new shared registry.
pub fn shared_registry() -> SharedRegistry {
    Arc::new(Mutex::new(ExtensionRegistry::new()))
}

/// A simple built-in extension that logs all events to stderr.
pub struct LoggingExtension {
    enabled: bool,
}

impl LoggingExtension {
    pub fn new() -> Self {
        Self { enabled: false }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
}

impl Default for LoggingExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl Extension for LoggingExtension {
    fn name(&self) -> &str {
        "logging"
    }

    fn description(&self) -> &str {
        "Logs all lifecycle events to stderr"
    }

    fn on_event(&self, ctx: &ExtensionContext) -> ExtensionResult {
        if self.enabled {
            eprintln!("[ext:logging] {:?} session={}", ctx.event, ctx.session_id);
        }
        ExtensionResult::default()
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_register_and_list() {
        let mut registry = ExtensionRegistry::new();
        let ext = Arc::new(LoggingExtension::new());
        assert!(registry.register(ext));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.list(), vec!["logging"]);
    }

    #[test]
    fn registry_duplicate_name() {
        let mut registry = ExtensionRegistry::new();
        let ext = Arc::new(LoggingExtension::new());
        assert!(registry.register(ext.clone()));
        assert!(!registry.register(ext)); // Duplicate
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registry_unregister() {
        let mut registry = ExtensionRegistry::new();
        let ext = Arc::new(LoggingExtension::new());
        registry.register(ext);
        assert!(registry.unregister("logging"));
        assert!(registry.is_empty());
        assert!(!registry.unregister("nonexistent"));
    }

    #[test]
    fn registry_dispatch() {
        let mut registry = ExtensionRegistry::new();
        let mut logging = LoggingExtension::new();
        logging.enable();
        registry.register(Arc::new(logging));

        let ctx = ExtensionContext {
            session_id: "test".to_string(),
            model: "gpt-4o".to_string(),
            cwd: "/tmp".to_string(),
            event: LifecycleEvent::TurnStart,
            payload: None,
        };

        let results = registry.dispatch(&ctx);
        assert!(!results.is_empty());
    }

    #[test]
    fn shared_registry_thread_safety() {
        let registry = shared_registry();
        {
            let mut r = registry.lock().unwrap();
            r.register(Arc::new(LoggingExtension::new()));
        }
        {
            let r = registry.lock().unwrap();
            assert_eq!(r.len(), 1);
        }
    }

    #[test]
    fn extension_subscribed_events_default_empty() {
        let ext = LoggingExtension::new();
        assert!(ext.subscribed_events().is_empty());
    }

    #[test]
    fn extension_is_enabled_default_false_for_logging() {
        let ext = LoggingExtension::new();
        assert!(!ext.is_enabled());
    }
}
