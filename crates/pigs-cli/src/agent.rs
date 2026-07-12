//! Agent core — orchestrates the LLM, tools, permissions, and session.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, warn};

use pigs_config::AppConfig;
use pigs_core::{
    ApiClient, ApiError, ApiRequest, ContentBlock, Message,
    StreamCallback, StreamEvent, TokenUsage,
};
use pigs_config::{ApiFormat, Language, ResolvedModel};
use pigs_llm::{create_client_for_endpoint, Provider as LlmProvider};
use pigs_permissions::{CliPermissionPrompter, PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPrompter};
use pigs_session::{compact_session, CompactConfig, Session};
use pigs_mcp::{McpClient, McpServerConfig, McpToolHandler};
use pigs_tools::create_default_registry_with_todos;
use pigs_tools::todo_write::TodoList;

use crate::cli::CliArgs;

/// The main agent, orchestrating all components.
pub struct Agent {
    pub config: AppConfig,
    pub session: Session,
    pub api_client: Arc<dyn ApiClient>,
    pub tool_registry: pigs_core::ToolRegistry,
    pub permission_policy: PermissionPolicy,
    pub system_prompt: String,
    pub max_turns: u32,
    pub no_tools: bool,
    pub one_shot_prompt: Option<String>,
    pub output_format: String,
    pub sessions_dir: PathBuf,
    #[allow(dead_code)]
    pub workspace_root: PathBuf,
    pub todo_list: pigs_tools::todo_write::TodoList,
    pub mcp_client: Arc<McpClient>,
    pub skills: Vec<pigs_config::Skill>,
    /// Shared skill catalog for the on-demand `skill` tool.
    pub skill_catalog: crate::skill_tool::SkillCatalog,
    pub rules: Vec<pigs_config::RuleDoc>,
    pub memory: pigs_config::MemoryStore,
    pub snapshots: crate::snapshots::SnapshotStore,
    /// Preferred UI / default reply language.
    pub language: Language,
}

impl Agent {

    fn llm_provider_from_api(api: ApiFormat) -> LlmProvider {
        match api {
            ApiFormat::OpenAI => LlmProvider::OpenAI,
            ApiFormat::OpenAIChat => LlmProvider::OpenAIChat,
            ApiFormat::Anthropic => LlmProvider::Anthropic,
        }
    }

    fn create_api_client(resolved: &ResolvedModel) -> anyhow::Result<Arc<dyn ApiClient>> {
        create_client_for_endpoint(
            Self::llm_provider_from_api(resolved.api),
            &resolved.remote_model,
            &resolved.api_key,
            &resolved.base_url,
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Create a new agent from config and CLI args.
    pub fn new(mut config: AppConfig, args: CliArgs) -> anyhow::Result<Self> {
        // Apply CLI overrides
        if let Some(model) = &args.model {
            config.model = model.clone();
        }
        // Default to `-pig` (phased) unless the user explicitly chose a model
        // without the suffix via --model.
        if args.model.is_none() && !config.model.ends_with("-pig") {
            config.model = format!("{}-pig", config.model);
        }
        if let Some(mode) = &args.mode {
            config.permission_mode = mode.clone();
        }
        if let Some(lang) = &args.language {
            config.language = lang.clone();
        }
        if let Some(prompt) = &args.system_prompt {
            config.system_prompt = Some(prompt.clone());
        }

        let language = config
            .language_parsed()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        // Resolve model against configured providers/models catalog
        let resolved = config
            .resolve_model(&config.model)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let model = resolved.remote_model.clone();
        let api_client = Self::create_api_client(&resolved)?;

        // Create or resume session
        let sessions_dir = AppConfig::sessions_dir();
        let session = if let Some(resume_id) = &args.resume {
            Session::load(&sessions_dir, resume_id)
                .map_err(|e| anyhow::anyhow!("Failed to resume session: {e}"))?
        } else {
            Session::new(&model)
        };

        // Get workspace root
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        // Build permission policy
        let permission_mode = config.permission_mode_parsed().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let mut policy = PermissionPolicy::new(permission_mode);
        for (name, required) in pigs_tools::tool_permission_modes() {
            policy = policy.with_tool_requirement(name, required);
        }
        // CLI-local tools
        policy = policy.with_tool_requirement("skill", PermissionMode::ReadOnly);

        // Build system prompt (base + project context + rules + skills)
        let base_owned = Self::compose_base_prompt(&config, language);
        let mut system_prompt =
            pigs_config::build_system_prompt_from_dir(&base_owned, &workspace_root);
        let rules = pigs_config::load_rules(&workspace_root);
        system_prompt.push_str(&pigs_config::format_rules_for_prompt(&rules));
        let memory = pigs_config::load_memory(&workspace_root);
        system_prompt.push_str(&pigs_config::format_memory_for_prompt(&memory));
        let skills = pigs_config::load_skills(&workspace_root);
        // Catalog only (names + short descriptions); full bodies load via `skill` tool.
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&skills));
        let skill_catalog: crate::skill_tool::SkillCatalog =
            std::sync::Arc::new(std::sync::Mutex::new(skills.clone()));

        // Create tool registry (with shared todo list)
        let (mut tool_registry, todo_list): (pigs_core::ToolRegistry, TodoList) = if args.no_tools {
            (
                pigs_core::ToolRegistry::new(),
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            )
        } else {
            create_default_registry_with_todos()
        };

        // Register the sub-agent tool (needs the API client)
        tool_registry.register(Box::new(crate::agent_tool::AgentTool::new(Arc::clone(&api_client))));
        // On-demand skill loader (claw-code style)
        if !args.no_tools {
            tool_registry.register(Box::new(crate::skill_tool::SkillTool::new(std::sync::Arc::clone(
                &skill_catalog,
            ))));
        }

        // MCP client (stdio servers)
        let mcp_client = Arc::new(McpClient::new());

        Ok(Agent {
            config,
            session,
            api_client,
            tool_registry,
            permission_policy: policy,
            system_prompt,
            max_turns: args.max_turns,
            no_tools: args.no_tools,
            one_shot_prompt: args.prompt,
            output_format: args.output,
            sessions_dir,
            todo_list,
            mcp_client,
            skills,
            skill_catalog,
            rules,
            memory,
            snapshots: crate::snapshots::SnapshotStore::load_from_workspace(&workspace_root),
            workspace_root,
            language,
        })
    }

    /// Built-in or custom base prompt, with a language reminder when custom.
    fn compose_base_prompt(config: &AppConfig, language: Language) -> String {
        match config.system_prompt.as_deref() {
            Some(custom) if !custom.trim().is_empty() => {
                format!("{custom}{}", language.language_reminder())
            }
            _ => language.default_system_prompt().to_string(),
        }
    }

    /// Set UI / reply language and rebuild the system prompt.
    pub fn set_language(&mut self, language: Language) {
        self.language = language;
        self.config.language = language.as_str().to_string();
        self.rebuild_prompt_context();
    }

    /// Run a single turn of the agent loop.
    /// Returns the assistant's final text response.
    pub async fn run_turn(&mut self, user_input: &str) -> anyhow::Result<String> {
        // If the current model is a `-pig` variant, route through PhasedRuntime.
        let is_pig = self
            .resolved_model()
            .map(|r| r.is_pig)
            .unwrap_or(false);
        if is_pig {
            return self.run_turn_phased(user_input).await;
        }

        // Add user message
        self.session.add_message(Message::user(user_input));

        let mut prompter = CliPermissionPrompter::new();

        let mut iteration: u32 = 0;
        let mut total_usage = TokenUsage::default();
        let mut final_text = String::new();

        loop {
            iteration += 1;
            if iteration > self.max_turns {
                warn!(iteration, "Max turns reached, stopping");
                final_text = format!("(Agent reached max turn limit of {})", self.max_turns);
                break;
            }

            // Check for auto-compaction
            let compact_config = self.compact_config(false);
            compact_session(&mut self.session, &compact_config);

            // Build API request
            let tool_defs = if self.no_tools {
                Vec::new()
            } else {
                self.tool_registry.definitions()
            };

            let request = ApiRequest::new(self.api_client.model(), self.session.messages.clone())
                .with_system_prompt(&self.system_prompt)
                .with_tools(tool_defs)
                .with_max_tokens(
                    self.resolved_model()
                        .ok()
                        .and_then(|m| m.max_tokens)
                        .unwrap_or(self.config.max_tokens),
                )
                .with_temperature(
                    self.resolved_model()
                        .ok()
                        .and_then(|m| m.temperature)
                        .unwrap_or(self.config.temperature),
                );

            // Call the LLM with streaming
            debug!(iteration, "Sending request to LLM");

            // Print a marker for the start of the response
            let is_first_iteration = iteration == 1;
            if is_first_iteration {
                println!();
            }

            let stream_callback = StreamPrinter::new();
            let response = match self
                .api_client
                .send_message_streaming(request, &stream_callback)
                .await
            {
                Ok(r) => r,
                Err(ApiError::ContextWindowExceeded(msg)) => {
                    warn!("Context window exceeded, compacting and retrying");
                    compact_session(
                        &mut self.session,
                        &CompactConfig {
                            token_threshold: 1000,
                            keep_recent: 2,
                            summary_message_chars: 300,
                            force: true,
                        },
                    );
                    if self.session.messages.len() <= 3 {
                        return Err(anyhow::anyhow!(
                            "Context window exceeded even after compaction: {msg}"
                        ));
                    }
                    continue;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("LLM API error: {e}"));
                }
            };

            // Track usage
            if let Some(usage) = &response.usage {
                total_usage.add(usage);
                self.session.add_usage(usage);
            }

            // Add assistant message
            let assistant_message = Message::assistant(response.content.clone());
            self.session.add_message(assistant_message);

            // Extract text content for display (streaming callback already printed it)
            let text = response.text_content();
            if !text.is_empty() {
                if !final_text.is_empty() {
                    final_text.push_str("\n\n");
                }
                final_text.push_str(&text);
                // Add a newline after streamed text
                println!();
            }

            // Extract tool uses
            let tool_uses: Vec<(String, String, serde_json::Value)> = response
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            // If no tool uses, the turn is complete
            if tool_uses.is_empty() {
                info!(iteration, total_usage = %total_usage, "Turn complete");
                break;
            }

            // Execute tools: authorize sequentially, then run allowed tools
            // in parallel when there are multiple.
            self.execute_tool_batch(&tool_uses, &mut prompter).await;
        }

        // Save session
        if let Err(e) = self.session.save(&self.sessions_dir) {
            warn!("Failed to save session: {e}");
        }

        Ok(final_text)
    }

    /// Authorize and execute a batch of tool calls.
    /// Permissions/prompter run sequentially; authorized tools may run concurrently.
    async fn execute_tool_batch(
        &mut self,
        tool_uses: &[(String, String, serde_json::Value)],
        prompter: &mut dyn PermissionPrompter,
    ) {
        // Phase 1: authorize each tool (may prompt the user)
        let mut authorized: Vec<(String, String, serde_json::Value)> = Vec::new();
        for (tool_id, tool_name, tool_input) in tool_uses {
            let input_preview = format_tool_input(tool_name, tool_input);
            println!("\n▸ {tool_name}{input_preview}");

            match self.authorize_tool(tool_name, tool_input, prompter) {
                Ok(()) => authorized.push((tool_id.clone(), tool_name.clone(), tool_input.clone())),
                Err(e) => {
                    let output = format!("Tool error: {e}");
                    eprintln!("  ✗ {output}");
                    self.session
                        .add_message(Message::tool_result(tool_id, &output, true));
                }
            }
        }

        if authorized.is_empty() {
            return;
        }

        // Capture write snapshots before mutation.
        let mut pre_snapshots: Vec<Option<crate::snapshots::SnapshotBatch>> = authorized
            .iter()
            .map(|(_, tool_name, tool_input)| self.capture_snapshot_for_tool(tool_name, tool_input))
            .collect();

        // Phase 2: execute authorized tools (parallel when >1).
        let results = {
            let agent: &Agent = self;
            if authorized.len() == 1 {
                let (tool_id, tool_name, tool_input) = &authorized[0];
                let result = agent.execute_authorized_tool(tool_name, tool_input).await;
                vec![(tool_id.clone(), result)]
            } else {
                info!(count = authorized.len(), "Executing tools in parallel");
                let futs: Vec<_> = authorized
                    .iter()
                    .map(|(tool_id, tool_name, tool_input)| {
                        let tool_id = tool_id.clone();
                        async {
                            let result = agent.execute_authorized_tool(tool_name, tool_input).await;
                            (tool_id, result)
                        }
                    })
                    .collect();
                futures_util::future::join_all(futs).await
            }
        };

        // Phase 3: record results and commit snapshots for successful writes.
        for (idx, (tool_id, result)) in results.into_iter().enumerate() {
            let (output, is_error) = match result {
                Ok(r) => (r.output, r.is_error),
                Err(e) => (format!("Tool error: {e}"), true),
            };

            if !output.is_empty() {
                let preview: String = output.chars().take(300).collect();
                let ellipsis = if output.len() > 300 { "..." } else { "" };
                if is_error {
                    eprintln!("  ✗ {preview}{ellipsis}");
                } else {
                    for line in preview.lines().take(5) {
                        println!("  {line}");
                    }
                    if preview.lines().count() > 5 || output.len() > 300 {
                        println!("  {ellipsis}");
                    }
                }
            }

            if !is_error {
                if let Some(batch) = pre_snapshots.get_mut(idx).and_then(|b| b.take()) {
                    if let Ok(path) = crate::snapshots::persist_batch(&self.workspace_root, &batch) {
                        debug!(path = %path.display(), "Write snapshot saved");
                    }
                    self.snapshots.push(batch);
                }
            }

            self.session
                .add_message(Message::tool_result(&tool_id, &output, is_error));
        }
    }

    fn is_write_tool(tool_name: &str) -> bool {
        matches!(tool_name, "write_file" | "edit_file" | "apply_patch")
    }

    fn capture_snapshot_for_tool(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Option<crate::snapshots::SnapshotBatch> {
        if !Self::is_write_tool(tool_name) {
            return None;
        }
        let mut files = Vec::new();
        match tool_name {
            "write_file" | "edit_file" => {
                if let Some(path) = tool_input.get("path").and_then(|v| v.as_str()) {
                    files.push(crate::snapshots::capture_file_snapshot(std::path::Path::new(path)));
                }
            }
            "apply_patch" => {
                if let Some(patch) = tool_input.get("patch").and_then(|v| v.as_str()) {
                    for line in patch.lines() {
                        if let Some(rest) = line.strip_prefix("+++ ") {
                            let path = rest.split("	").next().unwrap_or(rest).trim();
                            let path = path.strip_prefix("b/").unwrap_or(path);
                            if path != "/dev/null" && !path.is_empty() {
                                files.push(crate::snapshots::capture_file_snapshot(std::path::Path::new(path)));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if files.is_empty() {
            None
        } else {
            Some(crate::snapshots::SnapshotBatch {
                id: crate::snapshots::new_batch_id(),
                tool_name: tool_name.to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                files,
            })
        }
    }

    fn authorize_tool(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        prompter: &mut dyn PermissionPrompter,
    ) -> Result<(), pigs_core::ToolError> {
        let outcome = self
            .permission_policy
            .check(tool_name, tool_input, Some(prompter));

        match outcome {
            PermissionOutcome::Allow => {
                debug!(tool = tool_name, "Tool allowed");
                Ok(())
            }
            PermissionOutcome::Deny { reason } => {
                warn!(tool = tool_name, reason = %reason, "Tool denied");
                Err(pigs_core::ToolError::PermissionDenied(reason))
            }
            PermissionOutcome::Ask => Err(pigs_core::ToolError::PermissionDenied(
                "Permission required but no decision was made".to_string(),
            )),
        }
    }

    /// Execute an already-authorized tool with lifecycle hooks.
    async fn execute_authorized_tool(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Result<pigs_core::ToolResult, pigs_core::ToolError> {
        // Pre-tool hooks
        match crate::hooks::run_pre_tool_hooks(&self.config.hooks, tool_name, tool_input).await {
            crate::hooks::HookDecision::Allow => {}
            crate::hooks::HookDecision::Deny { reason } => {
                return Err(pigs_core::ToolError::PermissionDenied(reason));
            }
        }

        // Execute the tool
        let result = self
            .tool_registry
            .execute(tool_name, tool_input.clone())
            .await;

        // Post-tool hooks (best-effort)
        match &result {
            Ok(r) => {
                crate::hooks::run_post_tool_hooks(
                    &self.config.hooks,
                    tool_name,
                    tool_input,
                    &r.output,
                    r.is_error,
                )
                .await;
            }
            Err(e) => {
                crate::hooks::run_post_tool_hooks(
                    &self.config.hooks,
                    tool_name,
                    tool_input,
                    &e.to_string(),
                    true,
                )
                .await;
            }
        }

        result
    }

    /// Get the current session ID.
    pub fn session_id(&self) -> &str {
        &self.session.session_id
    }

    /// Set the permission mode.
    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_policy.set_mode(mode);
    }
    /// Set the model (requires creating a new API client).
    pub fn set_model(&mut self, model: &str) -> anyhow::Result<()> {
        let resolved = self
            .config
            .resolve_model(model)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let api_client = Self::create_api_client(&resolved)?;

        self.api_client = api_client;
        self.session.model = resolved.remote_model.clone();
        // Keep the user-facing selection name (catalog alias) in config.model.
        self.config.model = resolved.name;

        Ok(())
    }

    /// Resolve the currently selected model (catalog + provider credentials).
    pub fn resolved_model(&self) -> anyhow::Result<ResolvedModel> {
        self.config
            .resolve_model(&self.config.model)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Reload configuration from disk (and env overrides).
    /// Updates model, permission mode, max turns, and system prompt.
    pub fn reload_config(&mut self) -> anyhow::Result<()> {
        let mut config = AppConfig::load_layered(&self.workspace_root)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .with_env_overrides();

        // Preserve CLI-level one-shot settings that shouldn't be wiped
        // by reloading from disk (model/mode may still be overridden via /model, /mode).
        let resolved = config
            .resolve_model(&config.model)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let model = resolved.remote_model.clone();
        let api_client = Self::create_api_client(&resolved)?;

        let permission_mode = config
            .permission_mode_parsed()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let mut policy = PermissionPolicy::new(permission_mode);
        for (name, required) in pigs_tools::tool_permission_modes() {
            policy = policy.with_tool_requirement(name, required);
        }
        policy = policy.with_tool_requirement("skill", PermissionMode::ReadOnly);

        let language = config
            .language_parsed()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let base_owned = Self::compose_base_prompt(&config, language);
        let mut system_prompt =
            pigs_config::build_system_prompt_from_dir(&base_owned, &self.workspace_root);
        let rules = pigs_config::load_rules(&self.workspace_root);
        system_prompt.push_str(&pigs_config::format_rules_for_prompt(&rules));
        let memory = pigs_config::load_memory(&self.workspace_root);
        system_prompt.push_str(&pigs_config::format_memory_for_prompt(&memory));
        let skills = pigs_config::load_skills(&self.workspace_root);
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&skills));

        self.api_client = api_client;
        self.session.model = model.clone();
        self.max_turns = config.max_turns;
        self.permission_policy = policy;
        self.system_prompt = system_prompt;
        self.skills = skills.clone();
        if let Ok(mut guard) = self.skill_catalog.lock() {
            *guard = skills;
        }
        self.rules = rules;
        self.memory = memory;
        self.language = language;
        config.model = model;
        self.config = config;

        Ok(())
    }

    /// Reload skills from disk and rebuild the system prompt skills section.
    pub fn reload_skills(&mut self) {
        self.rebuild_prompt_context();
    }

    /// Reload project rules from `.pigs/rules`.
    pub fn reload_rules(&mut self) {
        self.rebuild_prompt_context();
    }

    /// Reload memory notes.
    pub fn reload_memory(&mut self) {
        self.rebuild_prompt_context();
    }

    fn compact_config(&self, force: bool) -> CompactConfig {
        let threshold = self
            .resolved_model()
            .map(|m| m.compact_threshold(self.config.compact_token_threshold))
            .unwrap_or(self.config.compact_token_threshold);
        CompactConfig {
            token_threshold: threshold,
            keep_recent: self.config.compact_keep_recent.max(1),
            summary_message_chars: 400,
            force,
        }
    }

    fn rebuild_prompt_context(&mut self) {
        self.skills = pigs_config::load_skills(&self.workspace_root);
        if let Ok(mut guard) = self.skill_catalog.lock() {
            *guard = self.skills.clone();
        }
        self.rules = pigs_config::load_rules(&self.workspace_root);
        self.memory = pigs_config::load_memory(&self.workspace_root);
        let base_owned = Self::compose_base_prompt(&self.config, self.language);
        let mut system_prompt =
            pigs_config::build_system_prompt_from_dir(&base_owned, &self.workspace_root);
        system_prompt.push_str(&pigs_config::format_rules_for_prompt(&self.rules));
        system_prompt.push_str(&pigs_config::format_memory_for_prompt(&self.memory));
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&self.skills));
        self.system_prompt = system_prompt;
    }

    pub fn clear_session(&mut self) {
        self.session.clear();
    }

    /// Run a turn through the phased runtime (Pre → Executor → Post).
    ///
    /// Used when the current model is a `-pig` variant. The agent's local
    /// tool registry is injected into the PhasedRuntime so that bash,
    /// read_file, MCP tools, etc. are available during execution.
    async fn run_turn_phased(&mut self, user_input: &str) -> anyhow::Result<String> {
        let resolved = self.resolved_model()?;
        let limits = crate::phased_runtime::RuntimeLimits {
            max_tokens: resolved.max_tokens.unwrap_or(self.config.max_tokens),
            temperature: resolved.temperature.unwrap_or(self.config.temperature),
            ..Default::default()
        };

        let mut runtime = crate::phased_runtime::PhasedRuntime::from_resolved(
            resolved,
            limits,
            self.language,
        )?;

        // Inject the agent's full tool registry (bash, read_file, MCP, etc.)
        let agent_tools = std::mem::replace(&mut self.tool_registry, pigs_core::ToolRegistry::new());
        runtime.tools = agent_tools;

        // Build the complete message array as an API caller would:
        // [system, ...prior_turns, current_user]
        let mut messages: Vec<Message> = Vec::with_capacity(self.session.messages.len() + 2);
        if !self.system_prompt.is_empty() {
            messages.push(Message::system(&self.system_prompt));
        }
        messages.extend(self.session.messages.iter().cloned());
        messages.push(Message::user(user_input));

        // Progress sink: print text deltas and tool events to the console in real time.
        let text_printed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let text_printed_clone = text_printed.clone();
        println!();
        let sink: crate::phased_runtime::ProgressSink = std::sync::Arc::new(move |p| {
            use crate::phased_runtime::TurnProgress;
            match p {
                TurnProgress::PhaseStart { phase } => {
                    eprintln!("[pigs] phase: {phase}");
                }
                TurnProgress::TextDelta { text, .. } => {
                    if !text.is_empty() {
                        text_printed_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                        print!("{text}");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                }
                TurnProgress::ToolStart { phase, name } => {
                    eprintln!("[pigs] tool: {name} ({phase})");
                }
                TurnProgress::ToolEnd {
                    phase,
                    name,
                    is_error,
                } => {
                    if is_error {
                        eprintln!("[pigs] tool: {name} ({phase}) failed");
                    }
                }
                _ => {}
            }
        });

        let result = runtime
            .run_turn_with_progress(&messages, Some(sink))
            .await;

        // Restore agent's tools.
        self.tool_registry = runtime.tools;

        let result = result?;

        // If no text was streamed (e.g. PRE short-circuit without stream_visible),
        // print the final text explicitly.
        if !text_printed.load(std::sync::atomic::Ordering::Relaxed) && !result.final_text.is_empty() {
            println!("{}", result.final_text);
        }
        println!(); // newline after streamed text

        // Update session: add user + assistant for multi-turn continuity.
        self.session.add_message(Message::user(user_input));
        self.session.add_message(Message::assistant(vec![pigs_core::ContentBlock::Text {
            text: result.final_text.clone(),
        }]));

        eprintln!(
            "[pigs] ended_with={} phases={}",
            result.ended_with,
            result
                .events
                .iter()
                .filter(|e| e.kind == "phase_start")
                .filter_map(|e| e.phase.as_deref())
                .collect::<Vec<_>>()
                .join(",")
        );

        Ok(result.final_text)
    }

    pub fn undo_last_write(&mut self) -> anyhow::Result<Vec<String>> {
        let batch = self
            .snapshots
            .pop()
            .ok_or_else(|| anyhow::anyhow!("No write snapshots to undo"))?;
        let report = crate::snapshots::restore_batch(&batch).map_err(|e| anyhow::anyhow!(e))?;
        Ok(report)
    }

    pub fn list_write_snapshots(&self) -> Vec<&crate::snapshots::SnapshotBatch> {
        self.snapshots.list()
    }

    /// List saved sessions.
    pub fn list_sessions() -> anyhow::Result<Vec<pigs_session::SessionMetadata>> {
        let sessions_dir = AppConfig::sessions_dir();
        Session::list(&sessions_dir).map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Delete a saved session by id or unique prefix.
    pub fn delete_session(session_id_or_prefix: &str) -> anyhow::Result<std::path::PathBuf> {
        let sessions_dir = AppConfig::sessions_dir();
        Session::delete(&sessions_dir, session_id_or_prefix).map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Switch current in-memory session to a saved one.
    pub fn switch_session(&mut self, session_id_or_prefix: &str) -> anyhow::Result<()> {
        let sessions_dir = AppConfig::sessions_dir();
        let session = Session::load(&sessions_dir, session_id_or_prefix)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        self.session = session;
        Ok(())
    }


    /// Connect all enabled MCP servers from config and register their tools.
    pub async fn connect_configured_mcp(&mut self) -> anyhow::Result<()> {
        if self.no_tools {
            return Ok(());
        }

        let servers: Vec<_> = self
            .config
            .mcp_servers
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();

        for entry in servers {
            match self
                .connect_mcp_server(
                    &entry.name,
                    &entry.command,
                    entry.args.clone(),
                    entry.env.clone(),
                )
                .await
            {
                Ok(count) => {
                    info!(
                        server = %entry.name,
                        tools = count,
                        "Connected MCP server from config"
                    );
                    println!(
                        "Connected MCP server '{}' ({} tools)",
                        entry.name, count
                    );
                }
                Err(e) => {
                    warn!(server = %entry.name, error = %e, "Failed to connect MCP server");
                    eprintln!(
                        "Warning: failed to connect MCP server '{}': {e}",
                        entry.name
                    );
                }
            }
        }
        Ok(())
    }

    /// Connect a single MCP server and register its tools into the registry.
    pub async fn connect_mcp_server(
        &mut self,
        name: &str,
        command: &str,
        args: Vec<String>,
        env: std::collections::HashMap<String, String>,
    ) -> anyhow::Result<usize> {
        if self.no_tools {
            return Err(anyhow::anyhow!("Tools are disabled (--no-tools)"));
        }

        let config = McpServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            args,
            env,
        };

        let tools = self
            .mcp_client
            .connect(config)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let count = tools.len();
        for tool_info in tools {
            let handler = McpToolHandler::new(Arc::clone(&self.mcp_client), tool_info);
            let exposed = handler.exposed_name().to_string();
            // MCP tools require at least workspace write? Keep ReadOnly by default —
            // individual servers may perform side effects, so use WorkspaceWrite for safety.
            self.permission_policy = self
                .permission_policy
                .clone()
                .with_tool_requirement(&exposed, PermissionMode::WorkspaceWrite);
            self.tool_registry.register(Box::new(handler));
        }

        Ok(count)
    }

    /// Disconnect an MCP server by name.
    pub async fn disconnect_mcp_server(&mut self, name: &str) -> anyhow::Result<()> {
        self.mcp_client
            .disconnect(name)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        // Note: tools remain in registry until restart; document this limitation.
        Ok(())
    }

    /// List connected MCP server names.
    pub async fn list_mcp_servers(&self) -> Vec<String> {
        self.mcp_client.list_servers().await
    }

    /// List tools from connected MCP servers.
    pub async fn list_mcp_tools(&self) -> Vec<pigs_mcp::McpToolInfo> {
        self.mcp_client.list_tools().await
    }
}

/// Format tool input for display, showing the most relevant field.
fn format_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    let preview = match tool_name {
        "bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 80)),
        "read_file" | "write_file" | "edit_file" => input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "grep_search" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|s| format!("pattern: {}", truncate_str(s, 60))),
        "glob_search" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "list_files" => input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "web_fetch" => input
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 80)),
        "http_request" => {
            let method = input
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_uppercase();
            let url = input
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| truncate_str(s, 70))
                .unwrap_or_else(|| "<missing url>".into());
            Some(format!("{method} {url}"))
        }
        "web_search" => input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 80)),
        "todo_write" => {
            let count = input.get("todos").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            Some(format!("{count} items"))
        }
        "ask_user" => input
            .get("question")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 60)),
        "sleep" => input
            .get("seconds")
            .and_then(|v| v.as_f64())
            .map(|n| format!("{n:.1}s")),
        "git_diff" => {
            let staged = input.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            Some(format!("{}:{path}", if staged { "staged" } else { "unstaged" }))
        }
        "apply_patch" => {
            let dry = input.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(if dry { "dry-run".into() } else { "apply".into() })
        }
        "skill" => input
            .get("name")
            .or_else(|| input.get("skill"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    match preview {
        Some(p) if !p.is_empty() => format!("({p})"),
        _ => String::new(),
    }
}

/// Truncate a string to max length with ellipsis.
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}

/// A stream callback that prints text deltas to stdout in real time.
struct StreamPrinter;

impl StreamPrinter {
    fn new() -> Self {
        StreamPrinter
    }
}

impl StreamCallback for StreamPrinter {
    fn on_event(&self, event: &StreamEvent) {
        match event {
            StreamEvent::TextDelta(text) => {
                use std::io::Write;
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            StreamEvent::Done { stop_reason: Some(reason) }
                if reason != "stop" && reason != "end_turn" =>
            {
                debug!(stop_reason = %reason, "Stream done");
            }
            _ => {}
        }
    }
}
