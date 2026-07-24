//! Sub-agent system — enables the main agent to spawn child agents.
//!
//! pigs = pig (phased orchestration) + s (free sub-agents).
//!
//! Sub-agents are fully equivalent to the main agent: same tools, same model,
//! same pig phased orchestration. They can recursively create their own
//! sub-agents. The TUI can switch between viewing the main agent's conversation
//! and any sub-agent's conversation via `/sub <id>`.

use std::collections::HashMap;
use tokio::sync::mpsc;

use pigs_core::Message;
use pigs_session::Session;

/// Sub-agent execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentMode {
    /// Main agent waits for the sub-agent to finish, then uses the result.
    Foreground,
    /// Main agent does NOT wait; sub-agent runs asynchronously and
    /// notifies the main agent upon completion.
    Background,
}

impl SubAgentMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
        }
    }
}

/// Sub-agent execution status.
#[derive(Debug, Clone, PartialEq)]
pub enum SubAgentStatus {
    /// Created but not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Completed successfully.
    Done,
    /// Failed with an error message.
    Error(String),
}

impl SubAgentStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Error(_))
    }

    pub fn display(&self) -> String {
        match self {
            Self::Pending => "pending".to_string(),
            Self::Running => "running".to_string(),
            Self::Done => "✓ done".to_string(),
            Self::Error(e) => format!("✗ error: {e}"),
        }
    }
}

/// A single sub-agent record.
#[derive(Debug, Clone)]
pub struct SubAgent {
    /// Unique ID (e.g. "sub-001").
    pub id: String,
    /// Task description given to the sub-agent.
    pub task: String,
    /// Execution mode.
    pub mode: SubAgentMode,
    /// Current status.
    pub status: SubAgentStatus,
    /// The sub-agent's own session (optionally shared from parent).
    pub session: Session,
    /// Result text (set when status becomes Done).
    pub result: Option<String>,
    /// Parent agent ID ("main" or another sub-agent's ID).
    pub parent_id: String,
    /// Child sub-agent IDs (for recursive spawning).
    pub children: Vec<String>,
    /// Chat entries for TUI display (text deltas accumulated during run).
    pub chat_log: Vec<String>,
    /// Custom system prompt (if using a custom agent type, empty = default).
    pub system_prompt: String,
    /// Optional model override (e.g. "haiku"). None = inherit parent.
    pub model_override: Option<String>,
    /// Restricted tool set (empty = all tools).
    pub allowed_tools: Vec<String>,
    /// Agent type name (e.g. "general", "scout").
    pub agent_type: String,
}

impl SubAgent {
    /// Create a new sub-agent with the given parameters.
    pub fn new(
        id: String,
        task: String,
        mode: SubAgentMode,
        parent_id: String,
        model: &str,
        share_context: bool,
        parent_messages: &[Message],
        sessions_dir: &std::path::Path,
    ) -> Self {
        let mut session = Session::new(model, sessions_dir);
        session.agent_type = "sub".to_string();
        session.parent_id = Some(parent_id.clone());
        if share_context {
            session.messages = parent_messages.to_vec();
        } else {
            // Start with just the task as the first user message
            session.add_message(Message::user(&task));
        }

        Self {
            id,
            task,
            mode,
            status: SubAgentStatus::Pending,
            session,
            result: None,
            parent_id,
            children: Vec::new(),
            chat_log: Vec::new(),
            system_prompt: String::new(),
            model_override: None,
            allowed_tools: Vec::new(),
            agent_type: "general".to_string(),
        }
    }

    /// Apply a custom agent definition to this sub-agent.
    pub fn apply_definition(&mut self, def: &pigs_config::sub_agent_def::SubAgentDefinition) {
        self.agent_type = def.name.clone();
        if !def.system_prompt.is_empty() {
            self.system_prompt = def.system_prompt.clone();
        }
        if def.model.is_some() {
            self.model_override = def.model.clone();
        }
        if !def.tools.is_empty() {
            self.allowed_tools = def.tools.clone();
        }
    }
}

/// Notification sent when a background sub-agent completes.
#[derive(Debug, Clone)]
pub struct SubAgentNotification {
    pub id: String,
    pub success: bool,
    pub result: String,
}

/// Manager for all sub-agents. Held by the main Agent.
pub struct SubAgentManager {
    /// All sub-agents by ID.
    pub agents: HashMap<String, SubAgent>,
    /// Navigation history stack: list of agent codes the user has visited.
    /// The current position is `nav_pos`. `nav_history[nav_pos]` is the current focus.
    nav_history: Vec<String>,
    /// Current position in the navigation history.
    nav_pos: usize,
    /// Receiver for background completion notifications.
    pub notification_rx: Option<mpsc::UnboundedReceiver<SubAgentNotification>>,
    /// Sender for background completion notifications (cloned for each background agent).
    notification_tx: mpsc::UnboundedSender<SubAgentNotification>,
}

impl SubAgentManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            agents: HashMap::new(),
            nav_history: Vec::new(),
            nav_pos: 0,
            notification_rx: Some(rx),
            notification_tx: tx,
        }
    }

    /// Initialize the navigation history with the main agent's ID.
    /// Called when a session is loaded or created.
    pub fn init_nav(&mut self, main_id: &str) {
        self.nav_history = vec![main_id.to_string()];
        self.nav_pos = 0;
    }

    /// Get the current focus agent ID (top of navigation history).
    pub fn current_focus(&self) -> &str {
        self.nav_history.get(self.nav_pos).map(|s| s.as_str()).unwrap_or("main")
    }

    /// Check if an agent ID is currently in memory (in the agents map or is the main session).
    pub fn is_in_memory(&self, id: &str) -> bool {
        self.agents.contains_key(id)
    }

    /// Navigate to a specific agent by ID.
    /// Truncates any forward history (positions after current) and appends the new ID.
    /// This is the "browser navigation" model: /ses <id> clears forward history.
    pub fn switch_to(&mut self, id: &str) -> bool {
        // Truncate forward history
        if self.nav_pos + 1 < self.nav_history.len() {
            self.nav_history.truncate(self.nav_pos + 1);
        }
        // Append the new ID (avoid duplicates of the current)
        if self.nav_history.last().map(|s| s.as_str()) != Some(id) {
            self.nav_history.push(id.to_string());
            self.nav_pos = self.nav_history.len() - 1;
        }
        true
    }

    /// Navigate back in history (decrement nav_pos).
    /// Returns the agent ID to switch to, or None if already at the beginning.
    /// Does NOT modify the agents map — only moves the navigation pointer.
    pub fn switch_back(&mut self) -> Option<String> {
        if self.nav_pos > 0 {
            self.nav_pos -= 1;
            Some(self.nav_history[self.nav_pos].clone())
        } else {
            None
        }
    }

    /// Navigate forward in history (increment nav_pos).
    /// Returns the agent ID to switch to, or None if already at the end.
    pub fn switch_forward(&mut self) -> Option<String> {
        if self.nav_pos + 1 < self.nav_history.len() {
            self.nav_pos += 1;
            Some(self.nav_history[self.nav_pos].clone())
        } else {
            None
        }
    }

    /// Check if can navigate back.
    pub fn can_go_back(&self) -> bool {
        self.nav_pos > 0
    }

    /// Check if can navigate forward.
    pub fn can_go_forward(&self) -> bool {
        self.nav_pos + 1 < self.nav_history.len()
    }

    /// Spawn one or more sub-agents. Returns the list of created IDs.
    pub fn spawn(
        &mut self,
        tasks: Vec<(String, SubAgentMode, bool)>,
        model: &str,
        parent_messages: &[Message],
        sessions_dir: &std::path::Path,
    ) -> Vec<String> {
        let parent_id = self.current_focus().to_string();
        let mut ids = Vec::new();

        for (task, mode, share_context) in tasks {
            let id = Session::generate_agent_code(sessions_dir);
            let sub = SubAgent::new(
                id.clone(),
                task,
                mode,
                parent_id.clone(),
                model,
                share_context,
                parent_messages,
                sessions_dir,
            );
            self.agents.insert(id.clone(), sub);
            ids.push(id);
        }

        ids
    }

    /// Get a sub-agent by ID.
    pub fn get(&self, id: &str) -> Option<&SubAgent> {
        self.agents.get(id)
    }

    /// Get a mutable sub-agent by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut SubAgent> {
        self.agents.get_mut(id)
    }

    /// List all sub-agents with their status.
    pub fn list(&self) -> Vec<(&str, &str, &SubAgentStatus, &SubAgentMode)> {
        self.agents
            .iter()
            .map(|(id, sub)| {
                (id.as_str(), sub.task.as_str(), &sub.status, &sub.mode)
            })
            .collect()
    }

    /// Get the notification sender (for background agents to report completion).
    pub fn notification_sender(&self) -> mpsc::UnboundedSender<SubAgentNotification> {
        self.notification_tx.clone()
    }

    /// Check if the current focus is the main agent.
    /// The main agent's ID is nav_history[0].
    pub fn is_main_focus(&self) -> bool {
        self.nav_pos == 0
    }

    /// Number of sub-agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Whether any sub-agents exist.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Mark a sub-agent as running.
    pub fn mark_running(&mut self, id: &str) {
        if let Some(sub) = self.agents.get_mut(id) {
            sub.status = SubAgentStatus::Running;
            sub.session.status = "active".to_string();
        }
    }

    /// Mark a sub-agent as done with a result.
    pub fn mark_done(&mut self, id: &str, result: String) {
        if let Some(sub) = self.agents.get_mut(id) {
            sub.status = SubAgentStatus::Done;
            sub.result = Some(result);
            sub.session.status = "done".to_string();
        }
    }

    /// Mark a sub-agent as failed with an error.
    pub fn mark_error(&mut self, id: &str, error: String) {
        if let Some(sub) = self.agents.get_mut(id) {
            sub.status = SubAgentStatus::Error(error);
            sub.session.status = "error".to_string();
        }
    }

    /// Append a chat log entry to a sub-agent (for TUI streaming display).
    pub fn append_log(&mut self, id: &str, text: &str) {
        if let Some(sub) = self.agents.get_mut(id) {
            sub.chat_log.push(text.to_string());
        }
    }

    /// Save all in-memory sub-agent sessions to disk.
    pub fn save_all(&self, sessions_dir: &std::path::Path) {
        for sub in self.agents.values() {
            let mut session = sub.session.clone();
            let _ = session.save(sessions_dir);
        }
    }

    /// Load all sub-agents from disk whose parent_id matches the given parent.
    /// Used when resuming a main agent session — restores sub-agent state as read-only.
    pub fn load_children(
        sessions_dir: &std::path::Path,
        parent_id: &str,
    ) -> Vec<SubAgent> {
        let sessions = match Session::list(sessions_dir) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        sessions
            .iter()
            .filter(|m| m.parent_id.as_deref() == Some(parent_id))
            .filter_map(|m| {
                let session = Session::load(sessions_dir, &m.session_id).ok()?;
                Some(SubAgent::from_session(session))
            })
            .collect()
    }

    /// Merge loaded sub-agents into the manager (for session resume).
    pub fn merge_loaded(&mut self, children: Vec<SubAgent>) {
        for child in children {
            self.agents.insert(child.id.clone(), child);
        }
    }

    /// Spawn sub-agents with agent_type and definitions.
    /// Returns the list of created IDs.
    pub fn spawn_with_types(
        &mut self,
        tasks: Vec<SpawnRequest>,
        model: &str,
        parent_messages: &[Message],
        definitions: &[pigs_config::sub_agent_def::SubAgentDefinition],
        sessions_dir: &std::path::Path,
    ) -> Vec<String> {
        let parent_id = self.current_focus().to_string();
        let mut ids = Vec::new();

        for req in tasks {
            let id = Session::generate_agent_code(sessions_dir);
            let mut sub = SubAgent::new(
                id.clone(),
                req.task,
                req.mode,
                parent_id.clone(),
                model,
                req.share_context,
                parent_messages,
                sessions_dir,
            );

            // Apply custom agent definition if agent_type is not "general"
            if req.agent_type != "general" {
                if let Some(def) = definitions.iter().find(|d| d.name == req.agent_type) {
                    sub.apply_definition(def);
                }
            }

            self.agents.insert(id.clone(), sub);
            ids.push(id);
        }

        ids
    }
}

impl SubAgent {
    /// Reconstruct a SubAgent from a loaded Session (for resume).
    pub fn from_session(session: Session) -> Self {
        let id = session.session_id.clone();
        let task = session
            .title
            .clone()
            .unwrap_or_else(|| "(resumed)".to_string());
        let parent_id = session.parent_id.clone().unwrap_or_default();
        let status_str = session.status.as_str();
        let status = match status_str {
            "done" => SubAgentStatus::Done,
            "error" => SubAgentStatus::Error("loaded from disk".to_string()),
            "active" => SubAgentStatus::Running,
            _ => SubAgentStatus::Pending,
        };
        Self {
            id,
            task,
            mode: SubAgentMode::Foreground,
            status,
            session,
            result: None,
            parent_id,
            children: Vec::new(),
            chat_log: Vec::new(),
            system_prompt: String::new(),
            model_override: None,
            allowed_tools: Vec::new(),
            agent_type: "general".to_string(),
        }
    }
}

/// A spawn request with agent type information.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub task: String,
    pub mode: SubAgentMode,
    pub share_context: bool,
    pub agent_type: String,
}

impl Default for SpawnRequest {
    fn default() -> Self {
        Self {
            task: String::new(),
            mode: SubAgentMode::Foreground,
            share_context: false,
            agent_type: "general".to_string(),
        }
    }
}

impl Default for SubAgentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_sessions_dir() -> std::path::PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "pigs_subagent_test_{}_{}",
            std::process::id(),
            counter
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn manager_spawn_and_get() {
        let dir = test_sessions_dir();
        let mut mgr = SubAgentManager::new();
        mgr.init_nav("main");
        let ids = mgr.spawn(
            vec![
                ("task 1".to_string(), SubAgentMode::Foreground, false),
                ("task 2".to_string(), SubAgentMode::Background, true),
            ],
            "gpt-4o",
            &[],
            &dir,
        );
        assert_eq!(ids.len(), 2);
        // IDs are random 6-char codes, not sequential
        assert_ne!(ids[0], ids[1]);

        let sub1 = mgr.get(&ids[0]).unwrap();
        assert_eq!(sub1.task, "task 1");
        assert_eq!(sub1.mode, SubAgentMode::Foreground);
        assert_eq!(sub1.status, SubAgentStatus::Pending);
        assert!(!sub1.session.messages.is_empty()); // task message

        let sub2 = mgr.get(&ids[1]).unwrap();
        assert_eq!(sub2.mode, SubAgentMode::Background);
        assert!(sub2.session.messages.is_empty()); // shared context but parent had no messages

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_switch_focus() {
        let dir = test_sessions_dir();
        let mut mgr = SubAgentManager::new();
        mgr.init_nav("main");
        let ids = mgr.spawn(
            vec![("task".to_string(), SubAgentMode::Foreground, false)],
            "gpt-4o",
            &[],
            &dir,
        );

        assert!(mgr.is_main_focus());
        assert!(mgr.switch_to(&ids[0]));
        assert!(!mgr.is_main_focus());
        assert_eq!(mgr.current_focus(), ids[0]);

        // Back to main
        mgr.switch_back();
        assert!(mgr.is_main_focus());
        assert_eq!(mgr.current_focus(), "main");

        // Forward again
        mgr.switch_forward();
        assert_eq!(mgr.current_focus(), ids[0]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_nav_truncate_and_append() {
        let dir = test_sessions_dir();
        let mut mgr = SubAgentManager::new();
        mgr.init_nav("main");
        let ids = mgr.spawn(
            vec![
                ("a".to_string(), SubAgentMode::Foreground, false),
                ("b".to_string(), SubAgentMode::Foreground, false),
            ],
            "gpt-4o",
            &[],
            &dir,
        );

        // Navigate: main -> id0 -> id1
        mgr.switch_to(&ids[0]);
        mgr.switch_to(&ids[1]);
        assert_eq!(mgr.current_focus(), ids[1]);

        // Back to id0
        mgr.switch_back();
        assert_eq!(mgr.current_focus(), ids[0]);

        // Navigate to a new agent — should truncate forward (id1) and append
        mgr.switch_to(&ids[1]); // re-navigate to id1 (same, no truncation needed)
        assert_eq!(mgr.current_focus(), ids[1]);

        // Can go back to id0 and main
        mgr.switch_back();
        assert_eq!(mgr.current_focus(), ids[0]);
        mgr.switch_back();
        assert!(mgr.is_main_focus());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_status_transitions() {
        let dir = test_sessions_dir();
        let mut mgr = SubAgentManager::new();
        mgr.init_nav("main");
        let ids = mgr.spawn(
            vec![("task".to_string(), SubAgentMode::Foreground, false)],
            "gpt-4o",
            &[],
            &dir,
        );
        let id = &ids[0];

        mgr.mark_running(id);
        assert!(matches!(mgr.get(id).unwrap().status, SubAgentStatus::Running));

        mgr.mark_done(id, "result text".to_string());
        assert!(matches!(mgr.get(id).unwrap().status, SubAgentStatus::Done));
        assert_eq!(mgr.get(id).unwrap().result.as_deref(), Some("result text"));

        let ids2 = mgr.spawn(
            vec![("task2".to_string(), SubAgentMode::Foreground, false)],
            "gpt-4o",
            &[],
            &dir,
        );
        let id2 = &ids2[0];
        mgr.mark_error(id2, "something went wrong".to_string());
        assert!(mgr.get(id2).unwrap().status.is_terminal());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_list() {
        let dir = test_sessions_dir();
        let mut mgr = SubAgentManager::new();
        mgr.spawn(
            vec![
                ("a".to_string(), SubAgentMode::Foreground, false),
                ("b".to_string(), SubAgentMode::Background, false),
            ],
            "gpt-4o",
            &[],
            &dir,
        );

        let list = mgr.list();
        assert_eq!(list.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sub_agent_shared_context() {
        let dir = test_sessions_dir();
        let messages = vec![Message::user("hello"), Message::assistant(vec![])];
        let sub = SubAgent::new(
            "abc123".to_string(),
            "do something".to_string(),
            SubAgentMode::Foreground,
            "main".to_string(),
            "gpt-4o",
            true,
            &messages,
            &dir,
        );
        // Should have parent's messages
        assert_eq!(sub.session.messages.len(), 2);

        let sub2 = SubAgent::new(
            "def456".to_string(),
            "do something else".to_string(),
            SubAgentMode::Foreground,
            "main".to_string(),
            "gpt-4o",
            false,
            &messages,
            &dir,
        );
        // Should only have the task message
        assert_eq!(sub2.session.messages.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_all_and_load_children() {
        let dir = test_sessions_dir();
        let mut mgr = SubAgentManager::new();
        mgr.init_nav("main");
        let ids = mgr.spawn(
            vec![
                ("task a".to_string(), SubAgentMode::Foreground, false),
                ("task b".to_string(), SubAgentMode::Background, false),
            ],
            "gpt-4o",
            &[],
            &dir,
        );

        // Mark one as done
        mgr.mark_done(&ids[0], "result".to_string());

        // Save all to disk
        mgr.save_all(&dir);

        // Load children of "main"
        let loaded = SubAgentManager::load_children(&dir, "main");
        assert_eq!(loaded.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
