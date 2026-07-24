//! Slash command handling for the REPL.
//!
//! English command names remain primary. Chinese characters and pinyin aliases
//! are always accepted via [`crate::command_aliases`], independent of UI language.

use std::str::FromStr;

use pigs_config::Language;
use pigs_permissions::PermissionMode;

use crate::agent::Agent;
use crate::command_aliases::{
    canonicalize_command, canonicalize_mcp_sub, canonicalize_sessions_sub,
};
use crate::i18n;

/// Result of handling a slash command.
pub enum CommandResult {
    /// Command handled successfully, continue the REPL.
    Continue,
    /// User requested to quit.
    Quit,
}

/// Handle a slash command.
pub async fn handle_command(agent: &mut Agent, line: &str) -> anyhow::Result<CommandResult> {
    let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
    let raw_cmd = parts[0].trim_start_matches('/');
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");
    let cmd = canonicalize_command(raw_cmd);

    // Bare `/中文` (or pinyin `/zhongwen`) with no args → switch language to zh.
    let bare_chinese_switch =
        matches!(raw_cmd, "中文" | "zhongwen" | "Zhongwen" | "ZHONGWEN") && arg.is_empty();

    match cmd {
        "help" => {
            print_help(&mut agent.output, agent.language);
            Ok(CommandResult::Continue)
        }

        "quit" => {
            agent.output.println(i18n::t(agent.language, "goodbye"));
            Ok(CommandResult::Quit)
        }

        "lang" => {
            if bare_chinese_switch {
                agent.set_language(Language::Zh);
                agent.output.println(i18n::t(agent.language, "lang_set_zh"));
                return Ok(CommandResult::Continue);
            }
            if arg.is_empty() {
                agent.output.println(format!(
                    "{}: {} ({})",
                    i18n::t(agent.language, "language"),
                    agent.language.as_str(),
                    agent.language.display_name()
                ));
                agent.output.println(i18n::t(agent.language, "lang_usage"));
            } else {
                match arg.parse::<Language>() {
                    Ok(lang) => {
                        agent.set_language(lang);
                        match lang {
                            Language::En => agent.output.println(i18n::t(agent.language, "lang_set_en")),
                            Language::Zh => agent.output.println(i18n::t(agent.language, "lang_set_zh")),
                        }
                    }
                    Err(e) => agent.output.eprintln(e),
                }
            }
            Ok(CommandResult::Continue)
        }

        "model" => {
            crate::models::handle_model_command(agent, arg)?;
            Ok(CommandResult::Continue)
        }

        "mode" => {
            if arg.is_empty() {
                agent.output.println(format!(
                    "Current permission mode: {}",
                    agent.permission_policy.active_mode
                ));
                agent.output.println("Usage: /mode <readonly|workspace_write|danger|ask|allow>");
                agent.output.println("       (alias: /模式 /权限 /moshi /quanxian)");
            } else {
                match arg {
                    "ask" => {
                        agent.permission_policy = agent.permission_policy.clone().always_ask();
                        agent.output.println("Permission mode: ask (prompt for every tool)");
                    }
                    "allow" => {
                        agent.permission_policy = agent.permission_policy.clone().allow_all();
                        agent.output.println("Permission mode: allow (all tools allowed without prompting)");
                    }
                    _ => match PermissionMode::from_str(arg) {
                        Ok(mode) => {
                            agent.set_permission_mode(mode.clone());
                            agent.output.println(format!("Permission mode: {mode}"));
                        }
                        Err(e) => agent.output.eprintln(e),
                    },
                }
            }
            Ok(CommandResult::Continue)
        }

        "clear" => {
            agent.clear_session();
            agent.output.println(i18n::t(agent.language, "session_cleared"));
            Ok(CommandResult::Continue)
        }

        "save" => {
            agent.output.println("Sessions are auto-saved after each turn. No manual save needed.");
            Ok(CommandResult::Continue)
        }

        "ses" => {
            let parts: Vec<&str> = arg.splitn(2, " ").collect();
            let sub_raw = parts.first().copied().unwrap_or("");
            let sub = if sub_raw.is_empty() {
                "list"
            } else {
                canonicalize_sessions_sub(sub_raw)
            };
            match sub {
                "list" => {
                    // List all agents: Active (in memory) + History (on disk)
                    let mgr = agent.sub_agent_manager.lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let current_focus = mgr.current_focus().to_string();
                    let in_memory_ids: std::collections::HashSet<String> = mgr.agents.keys()
                        .cloned()
                        .collect();
                    let main_id = agent.session.session_id.clone();

                    // Active section: main agent + in-memory sub-agents
                    agent.output.println("Active (in memory):");
                    // Main agent
                    let main_marker = if current_focus == main_id { " ← viewing" } else { "" };
                    let main_title = agent.session.display_title();
                    agent.output.println(format!("  {main_id:<8} [main]    active     {main_title}{main_marker}"));
                    // Sub-agents in memory
                    for (id, task, status, mode) in mgr.list() {
                        let marker = if current_focus == id { " ← viewing" } else { "" };
                        let task_preview: String = task.chars().take(40).collect();
                        agent.output.println(format!("  {id:<8} [sub]     {mode_str:<10} {status_str:<20} {}{}", task_preview, marker,
                            mode_str = mode.as_str(),
                            status_str = status.display(),
                        ));
                    }
                    drop(mgr);

                    // History section: on disk but not in memory
                    match Agent::list_sessions() {
                        Ok(sessions) => {
                            let history: Vec<_> = sessions.iter()
                                .filter(|s| s.session_id != main_id && !in_memory_ids.contains(s.session_id.as_str()))
                                .collect();
                            if !history.is_empty() {
                                agent.output.println_empty();
                                agent.output.println("History (on disk):");
                                for s in history.iter().take(20) {
                                    let updated = s.updated_at.format("%Y-%m-%d %H:%M");
                                    let title = s.title.clone().unwrap_or_else(|| "(untitled)".to_string());
                                    let title = if title.chars().count() > 30 {
                                        let t: String = title.chars().take(27).collect();
                                        format!("{t}...")
                                    } else {
                                        title
                                    };
                                    let agent_type = if s.agent_type.is_empty() { "?" } else { &s.agent_type };
                                    agent.output.println(format!("  {:<8} [{:<4}]    {}  {}", s.session_id, agent_type, title, updated));
                                }
                            }
                        }
                        Err(e) => agent.output.eprintln(format!("Failed to list history: {e}")),
                    }
                }
                "rm" => {
                    let id = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    if id.is_empty() {
                        agent.output.println("Usage: /ses rm <id-or-prefix>");
                    } else {
                        match Agent::delete_session(id) {
                            Ok(path) => {
                                agent.output.println(format!("Deleted session file: {}", path.display()));
                                let cur = agent.session.short_id();
                                if agent.session.session_id.starts_with(id) || id.starts_with(cur) {
                                    agent.output.println("Note: current in-memory session was not cleared.");
                                }
                            }
                            Err(e) => agent.output.eprintln(format!("Failed to delete session: {e}")),
                        }
                    }
                }
                "search" => {
                    let q = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    if q.is_empty() {
                        agent.output.println("Usage: /ses search <text>");
                    } else {
                        match Agent::list_sessions() {
                            Ok(sessions) => {
                                let q_lower = q.to_lowercase();
                                let filtered: Vec<_> = sessions
                                    .into_iter()
                                    .filter(|s| {
                                        s.session_id.to_lowercase().contains(&q_lower)
                                            || s.model.to_lowercase().contains(&q_lower)
                                            || s.title
                                                .as_ref()
                                                .map(|t| t.to_lowercase().contains(&q_lower))
                                                .unwrap_or(false)
                                    })
                                    .collect();
                                if filtered.is_empty() {
                                    agent.output.println(format!("No sessions matched `{q}`."));
                                } else {
                                    agent.output.println(format!("Matches for `{}` ({}):", q, filtered.len()));
                                    for s in filtered.iter().take(50) {
                                        let title = s.title.clone().unwrap_or_else(|| "(untitled)".to_string());
                                        agent.output.println(format!("  {}  {}  {} msgs", s.session_id, title, s.message_count));
                                    }
                                }
                            }
                            Err(e) => agent.output.eprintln(format!("Failed to search: {e}")),
                        }
                    }
                }
                _ => {
                    // /ses <id> — smart navigate
                    let id = sub_raw.trim();
                    // Check if it's a subcommand keyword
                    if id == "list" || id == "current" || id == "open" {
                        // Legacy subcommands — treat "open <id>" as "/ses <id>"
                        if id == "open" {
                            let target = parts.get(1).map(|s| s.trim()).unwrap_or("");
                            if !target.is_empty() {
                                match agent.switch_session(target) {
                                    Ok(()) => {
                                        agent.output.println(format!("Switched to session {} ({})",
                                            agent.session.session_id,
                                            agent.session.display_title()));
                                    }
                                    Err(e) => agent.output.eprintln(format!("Failed to switch: {e}")),
                                }
                            }
                        }
                    } else if !id.is_empty() {
                        // Smart: check if in memory
                        let in_memory = agent.sub_agent_manager.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .is_in_memory(id);
                        if in_memory {
                            // Just switch focus (navigation), no reload
                            agent.sub_agent_manager.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .switch_to(id);
                            agent.output.println(format!("Switched focus to: {id}"));
                        } else {
                            // Load from disk (includes sub-agent restoration)
                            match agent.switch_session(id) {
                                Ok(()) => {
                                    agent.output.println(format!("Loaded session {} ({})",
                                        agent.session.session_id,
                                        agent.session.display_title()));
                                }
                                Err(e) => agent.output.eprintln(format!("Failed to load: {e}")),
                            }
                        }
                    }
                }
            }
            Ok(CommandResult::Continue)
        }

        "tools" => {
            if agent.no_tools {
                agent.output.println("Tools are disabled (--no-tools flag).");
            } else {
                let names = agent.tool_registry.names();
                agent.output.println(format!("Available tools ({}):", names.len()));
                for name in &names {
                    agent.output.println(format!("  - {name}"));
                }
            }
            Ok(CommandResult::Continue)
        }

        "todo" => {
            let todos = agent.todo_list.lock();
            match todos {
                Ok(todos) => {
                    if todos.is_empty() {
                        agent.output.println("Todo list is empty.");
                    } else {
                        let total = todos.len();
                        let completed = todos
                            .iter()
                            .filter(|t| {
                                matches!(t.status, pigs_tools::todo_write::TodoStatus::Completed)
                            })
                            .count();
                        agent.output.println(format!("Todo list ({completed}/{total} completed):"));
                        agent.output.println("-".repeat(60));
                        for (i, item) in todos.iter().enumerate() {
                            let status_icon = match item.status {
                                pigs_tools::todo_write::TodoStatus::Completed => "[x]",
                                pigs_tools::todo_write::TodoStatus::InProgress => "[~]",
                                pigs_tools::todo_write::TodoStatus::Pending => "[ ]",
                            };
                            let priority_tag = match item.priority {
                                pigs_tools::todo_write::TodoPriority::High => "HIGH",
                                pigs_tools::todo_write::TodoPriority::Medium => "MED ",
                                pigs_tools::todo_write::TodoPriority::Low => "LOW ",
                            };
                            agent.output.println(format!("  {} {}  {}. {}", priority_tag,
                                status_icon,
                                i + 1,
                                item.content
                            ));
                        }
                    }
                }
                Err(e) => agent.output.eprintln(format!("Failed to read todo list: {e}")),
            }
            Ok(CommandResult::Continue)
        }

        "status" => {
            agent.output.println("Pigs status");
            agent.output.println("-".repeat(60));
            agent.output.println(format!(
                "Session:    {} ({})",
                agent.session.display_title(),
                agent.session.short_id()
            ));
            agent.output.println(format!("Model:      {}", agent.api_client.model()));
            agent.output.println(format!(
                "{}:   {} ({})",
                i18n::t(agent.language, "language"),
                agent.language.as_str(),
                agent.language.display_name()
            ));
            agent.output.println(format!("Permission: {}", agent.permission_policy.active_mode));
            agent.output.println(format!("Messages:   {}", agent.session.message_count()));
            agent.output.println(format!("Est tokens: {}", agent.session.estimated_tokens()));
            agent.output.println(format!("Usage:      {}", agent.session.total_usage));
            if let Some(cost) = agent
                .session
                .total_usage
                .estimate_cost_for_model(agent.api_client.model())
            {
                agent.output.println("Est cost:   ${cost:.4}");
            }
            agent.output.println(format!(
                "Tools:      {}",
                if agent.no_tools {
                    0
                } else {
                    agent.tool_registry.len()
                }
            ));
            agent.output.println(format!("Skills:     {}", agent.skills.len()));
            agent.output.println(format!("Rules:      {}", agent.rules.len()));
            agent.output.println(format!("Memory:     {} note(s)", agent.memory.len()));
            let servers = agent.list_mcp_servers().await;
            agent.output.println(format!("MCP:        {} server(s)", servers.len()));
            if !servers.is_empty() {
                agent.output.println(format!("            {}", servers.join(", ")));
            }
            agent.output.println(format!("Workspace:  {}", agent.workspace_root.display()));
            agent.output.println(format!(
                "Logs:       {}",
                pigs_config::AppConfig::logs_dir().display()
            ));
            Ok(CommandResult::Continue)
        }

        "info" => {
            agent.output.println(format!("Session ID:   {}", agent.session_id()));
            agent.output.println(format!("Model:        {}", agent.api_client.model()));
            agent.output.println(format!("Permission:   {}", agent.permission_policy.active_mode));
            agent.output.println(format!("Messages:     {}", agent.session.message_count()));
            agent.output.println(format!("Est. tokens:  {}", agent.session.estimated_tokens()));
            agent.output.println(format!("Usage:        {}", agent.session.total_usage));
            agent.output.println(format!(
                "Tools:        {}",
                if agent.no_tools {
                    "disabled"
                } else {
                    "enabled"
                }
            ));
            agent.output.println(format!("Max turns:    {}", agent.max_turns));
            agent.output.println(format!("Skills:       {}", agent.skills.len()));
            Ok(CommandResult::Continue)
        }

        "title" => {
            if arg.is_empty() {
                agent.output.println(format!("Current title: {}", agent.session.display_title()));
                agent.output.println("Usage: /title <new title>");
            } else {
                agent.session.set_title(arg);
                if let Err(e) = agent.session.save(&agent.sessions_dir) {
                    agent.output.eprintln(format!("Failed to save session title: {e}"));
                } else {
                    agent.output.println(format!("Title set to: {}", agent.session.display_title()));
                }
            }
            Ok(CommandResult::Continue)
        }

        "cost" => {
            let usage = &agent.session.total_usage;
            let model = agent.api_client.model();
            agent.output.println("Token usage for this session:");
            agent.output.println(format!("  Model:         {model}"));
            agent.output.println(format!("  Input tokens:  {}", usage.input_tokens));
            agent.output.println(format!("  Output tokens: {}", usage.output_tokens));
            if let Some(cached) = usage.cache_read_tokens {
                agent.output.println(format!("  Cached tokens: {cached}"));
            }
            agent.output.println(format!("  Total tokens:  {}", usage.total_tokens()));
            match usage.estimate_cost_for_model(model) {
                Some(cost) => {
                    agent.output.println("  Est. cost:     ${cost:.4} USD (approximate list pricing)");
                }
                None => {
                    agent.output.println("  Est. cost:     (unknown model pricing)");
                }
            }
            Ok(CommandResult::Continue)
        }

        "init" => {
            // Create default config file
            match agent.config.clone().save() {
                Ok(()) => {
                    let config_path = pigs_config::AppConfig::config_path();
                    agent.output.println(format!("Config file created at: {}", config_path.display()));
                    agent.output.println("Edit it to set your API keys and preferences.");
                }
                Err(e) => agent.output.eprintln(format!("Failed to create config: {e}")),
            }
            Ok(CommandResult::Continue)
        }

        "reload" => {
            match agent.reload_config() {
                Ok(()) => {
                    agent.output.println(i18n::t(agent.language, "config_reloaded"));
                    agent.output.println(format!("  Model:      {}", agent.api_client.model()));
                    agent.output.println(format!("  Permission: {}", agent.permission_policy.active_mode));
                    agent.output.println(format!(
                        "  {}:     {} ({})",
                        i18n::t(agent.language, "language"),
                        agent.language.as_str(),
                        agent.language.display_name()
                    ));
                    agent.output.println(format!("  Max turns:  {}", agent.max_turns));
                }
                Err(e) => agent.output.eprintln(format!("Failed to reload config: {e}")),
            }
            Ok(CommandResult::Continue)
        }

        "mcp" => {
            handle_mcp_command(agent, arg).await?;
            Ok(CommandResult::Continue)
        }

        "memory" => {
            let parts: Vec<&str> = arg.splitn(2, ' ').collect();
            let sub = parts.first().copied().unwrap_or("");
            match sub {
                "" | "list" | "ls" => {
                    if agent.memory.is_empty() {
                        agent.output.println("Memory is empty.");
                        agent.output.println("Use: /memory add [--global] <note>");
                    } else {
                        agent.output.println(format!("Memory notes ({}):", agent.memory.len()));
                        if !agent.memory.global.is_empty() {
                            agent.output.println("[global]");
                            for (i, n) in agent.memory.global.iter().enumerate() {
                                agent.output.println(format!("  {}. {}", n, i + 1));
                            }
                        }
                        if !agent.memory.project.is_empty() {
                            agent.output.println("[project]");
                            for (i, n) in agent.memory.project.iter().enumerate() {
                                agent.output.println(format!("  {}. {}", n, i + 1));
                            }
                        }
                    }
                }
                "add" => {
                    let rest = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    let (source, note) = if let Some(n) = rest.strip_prefix("--global ") {
                        (pigs_config::MemorySource::Global, n.trim())
                    } else if rest == "--global" {
                        agent.output.println("Usage: /memory add [--global] <note>");
                        return Ok(CommandResult::Continue);
                    } else {
                        (pigs_config::MemorySource::Project, rest)
                    };
                    if note.is_empty() {
                        agent.output.println("Usage: /memory add [--global] <note>");
                    } else {
                        match pigs_config::add_memory_note(&agent.workspace_root, source, note) {
                            Ok(path) => {
                                agent.reload_memory();
                                agent.output.println(format!(
                                    "Added {} memory note -> {}",
                                    source.as_str(),
                                    path.display()
                                ));
                            }
                            Err(e) => agent.output.eprintln(format!("Failed to add memory: {e}")),
                        }
                    }
                }
                "rm" | "remove" => {
                    let rest = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    let (source, needle) = if let Some(n) = rest.strip_prefix("--global ") {
                        (pigs_config::MemorySource::Global, n.trim())
                    } else {
                        (pigs_config::MemorySource::Project, rest)
                    };
                    if needle.is_empty() {
                        agent.output.println("Usage: /memory rm [--global] <substring>");
                    } else {
                        match pigs_config::remove_memory_notes(
                            &agent.workspace_root,
                            source,
                            needle,
                        ) {
                            Ok((n, path)) => {
                                agent.reload_memory();
                                agent.output.println(format!("Removed {} {} note(s) from {}", n,
                                    source.as_str(),
                                    path.display()
                                ));
                            }
                            Err(e) => agent.output.eprintln(format!("Failed to remove memory: {e}")),
                        }
                    }
                }
                "reload" => {
                    agent.reload_memory();
                    agent.output.println(format!("Memory reloaded ({} notes).", agent.memory.len()));
                }
                _ => {
                    agent.output.println("Memory commands:");
                    agent.output.println("  /memory list");
                    agent.output.println("  /memory add [--global] <note>");
                    agent.output.println("  /memory rm  [--global] <substring>");
                    agent.output.println("  /memory reload");
                }
            }
            Ok(CommandResult::Continue)
        }

        "rules" => {
            if arg == "reload" {
                agent.reload_rules();
                agent.output.println(format!("Rules reloaded ({}).", agent.rules.len()));
            }
            if agent.rules.is_empty() {
                agent.output.println("No project rules loaded.");
                agent.output.println("Add markdown files under .pig/rules/");
            } else {
                agent.output.println(format!("Project rules ({}):", agent.rules.len()));
                for rule in &agent.rules {
                    agent.output.println(format!("  - {}  ({})", rule.name, rule.path.display()));
                }
            }
            Ok(CommandResult::Continue)
        }

        "skills" => {
            if arg == "reload" {
                agent.reload_skills();
                agent.output.println(format!("Skills reloaded ({} skill(s)).", agent.skills.len()));
            }
            if agent.skills.is_empty() {
                agent.output.println("No skills loaded.");
                agent.output.println("Place SKILL.md or *.md files in (first match of a name wins):");
                agent.output.println("  ~/.pig/skills/          # pigs user");
                agent.output.println("  ~/.agents/skills/        # common user agent skills");
                agent.output.println("  .pig/skills/            # pigs project");
                agent.output.println("  .agents/skills/          # common project skills (e.g. ARIS)");
                agent.output.println("  skills/                  # workspace root");
            } else {
                agent.output.println(format!(
                    "Skill catalog ({}): names+descriptions in system prompt; full body via tool `skill`",
                    agent.skills.len()
                ));
                for skill in &agent.skills {
                    let desc = if skill.description.is_empty() {
                        "(no description)"
                    } else {
                        skill.description.as_str()
                    };
                    agent.output.println(format!("  - {}  {}", skill.name, desc));
                    agent.output.println(format!("    {}", skill.path.display()));
                }
            }
            Ok(CommandResult::Continue)
        }

        "undo" => {
            match arg {
                "list" | "ls" => {
                    // Clone to release the immutable borrow on `agent` so we
                    // can mutably borrow `agent.output` inside the loop below.
                    let items: Vec<_> = agent.list_write_snapshots().into_iter().cloned().collect();
                    if items.is_empty() {
                        agent.output.println("No undo snapshots.");
                    } else {
                        agent.output.println("Undoable write batches (most recent first):");
                        for (i, b) in items.iter().enumerate() {
                            agent.output.println(format!(
                                "  {}. {}  tool={}  files={}  at={}",
                                i + 1,
                                b.id,
                                b.tool_name,
                                b.files.len(),
                                b.created_at
                            ));
                            for f in b.files.iter().take(5) {
                                agent.output.println(format!("      - {}", f.path.display()));
                            }
                        }
                    }
                }
                "" => match agent.undo_last_write() {
                    Ok(report) => {
                        agent.output.println("Undo complete:");
                        for line in report {
                            agent.output.println(format!("  {line}"));
                        }
                    }
                    Err(e) => agent.output.eprintln(format!("Undo failed: {e}")),
                },
                _ => {
                    agent.output.println("Usage:");
                    agent.output.println("  /undo        Undo last write tool batch");
                    agent.output.println("  /undo list   List undoable batches");
                }
            }
            Ok(CommandResult::Continue)
        }

        "export" => {
            let path = if arg.is_empty() {
                format!("session-{}.md", agent.session.short_id())
            } else {
                arg.to_string()
            };
            match export_session_markdown(agent, &path) {
                Ok(()) => agent.output.println(format!("Session exported to: {path}")),
                Err(e) => agent.output.eprintln(format!("Failed to export session: {e}")),
            }
            Ok(CommandResult::Continue)
        }

        "doctor" => {
            let items = crate::doctor::run_doctor(agent);
            crate::doctor::print_doctor_report(&mut agent.output, &items);
            Ok(CommandResult::Continue)
        }

        "models" => {
            crate::models::print_models(
                &mut agent.output,
                &agent.config,
                &agent.config.model,
                agent.api_client.model(),
            );
            Ok(CommandResult::Continue)
        }

        "hooks" => {
            agent.output.println(crate::hooks::summarize_hooks(&agent.config.hooks));
            agent.output.println_empty();
            agent.output.println("Configure hooks in ~/.pigs/pig.toml:");
            agent.output.println("  [[hooks.pre_tool_use]]");
            agent.output.println("  matcher = \"bash\"");
            agent.output.println("  command = \"echo checking $PIG_TOOL_NAME\"");
            agent.output.println("  timeout = 10");
            agent.output.println("  enabled = true");
            Ok(CommandResult::Continue)
        }

        "history" => {
            let messages = &agent.session.messages;
            if messages.is_empty() {
                agent.output.println("No messages in this session.");
            } else {
                agent.output.println(format!(
                    "Session history ({} messages, est. {} tokens):",
                    messages.len(),
                    agent.session.estimated_tokens()
                ));
                agent.output.println("-".repeat(60));
                for (i, msg) in messages.iter().enumerate() {
                    let role = match msg.role {
                        pigs_core::MessageRole::System => "system",
                        pigs_core::MessageRole::User => "user",
                        pigs_core::MessageRole::Assistant => "assistant",
                        pigs_core::MessageRole::Tool => "tool",
                    };
                    let text = msg.text_content();
                    let preview = if text.chars().count() > 80 {
                        let truncated: String = text.chars().take(80).collect();
                        format!("{truncated}...")
                    } else if text.is_empty() {
                        let tool_uses = msg.tool_uses();
                        if !tool_uses.is_empty() {
                            let names: Vec<&str> = tool_uses.iter().map(|(_, n, _)| *n).collect();
                            format!("[tools: {}]", names.join(", "))
                        } else {
                            "[empty]".to_string()
                        }
                    } else {
                        text
                    };
                    agent.output.println(format!("{:>3}. [{role:<9}] {}", preview, i + 1));
                }
            }
            Ok(CommandResult::Continue)
        }

        "compact" => {
            // 手动压缩命令。默认使用 LLM 摘要式压缩。
            // /compact          — LLM 摘要式压缩（默认）
            // /compact truncate — 截断式压缩（不调 LLM，每条旧消息截断到 400 字符）
            //
            // 自动压缩在 API 层（pigs-proxy）实现，agent 层不需要自动压缩。
            let keep_recent: usize = 4;
            let msg_count = agent.session.messages.len();

            if msg_count <= keep_recent {
                agent.output.println(format!("Only {msg_count} messages (keep_recent={keep_recent}), nothing to compact."));
                return Ok(CommandResult::Continue);
            }

            // /compact truncate — 截断式压缩（显式请求）
            if arg.trim() == "truncate" || arg.trim() == "截断" {
                use pigs_session::{compact_session, CompactConfig};
                let old_count = agent.session.messages.len();
                let config = CompactConfig {
                    token_threshold: 0,
                    keep_recent,
                    summary_message_chars: 400,
                    force: true,
                };
                let compacted = compact_session(&mut agent.session, &config);
                if compacted {
                    let _ = agent.session.save(&agent.sessions_dir);
                    agent.output.println(format!(
                        "Truncation compaction done: {} messages → summary + {} recent (est. tokens: {})",
                        old_count - keep_recent,
                        keep_recent,
                        agent.session.estimated_tokens()
                    ));
                } else {
                    agent.output.println(format!("No compaction needed (est. tokens: {})", agent.session.estimated_tokens()));
                }
                return Ok(CommandResult::Continue);
            }

            // 默认：LLM 摘要式压缩
            use pigs_core::{ApiRequest, Message};

            let messages = agent.session.messages.clone();
            let split_point = messages.len() - keep_recent;
            let old_messages = &messages[..split_point];

            // Serialize old messages into text for the summarization prompt
            let mut conversation_text = String::new();
            for (i, msg) in old_messages.iter().enumerate() {
                let role = match msg.role {
                    pigs_core::MessageRole::System => "system",
                    pigs_core::MessageRole::User => "user",
                    pigs_core::MessageRole::Assistant => "assistant",
                    pigs_core::MessageRole::Tool => "tool",
                };
                conversation_text.push_str(&format!("--- Message {i} [{role}] ---\\\\n"));
                for block in &msg.content {
                    match block {
                        pigs_core::ContentBlock::Text { text } => {
                            let truncated = if text.len() > 2000 {
                                format!("{}...(truncated)", &text[..2000])
                            } else {
                                text.clone()
                            };
                            conversation_text.push_str(&truncated);
                            conversation_text.push('\n');
                        }
                        pigs_core::ContentBlock::ToolUse { name, input, .. } => {
                            conversation_text.push_str(&format!("[Tool Call: {name}]\\\\n"));
                            let input_str = serde_json::to_string_pretty(input).unwrap_or_default();
                            let truncated = if input_str.len() > 1000 {
                                format!("{}...(truncated)", &input_str[..1000])
                            } else {
                                input_str
                            };
                            conversation_text.push_str(&truncated);
                            conversation_text.push('\n');
                        }
                        pigs_core::ContentBlock::ToolResult { output, .. } => {
                            conversation_text.push_str("[Tool Result]\n");
                            let truncated = if output.len() > 1000 {
                                format!("{}...(truncated)", &output[..1000])
                            } else {
                                output.clone()
                            };
                            conversation_text.push_str(&truncated);
                            conversation_text.push('\n');
                        }
                    }
                }
                conversation_text.push('\n');
            }

            let summary_prompt = r#"You are a conversation summarizer. Summarize the following conversation context into a structured summary with these sections:

## Objective
What the user is trying to accomplish.

## Important Details
Key technical decisions, constraints, and context.

## Work State
- Completed: What has been done.
- Active: What is being worked on.
- Blocked: Any blockers.

## Relevant Files
Files that have been read, modified, or discussed.

## Key Code & Commands
Important code snippets, commands, or configurations.

## Next Step
What should happen next.

Rules:
- Preserve exact file paths, symbol names, and commands.
- Be concise but complete.
- Never mention the compaction process."#;

            agent.output.println(format!("Compacting {split_point} messages via LLM summarization..."));

            let request = ApiRequest::new(
                &agent.config.model,
                vec![Message::user(format!("Summarize the following conversation:\\\\n\\\\n{conversation_text}"))],
            )
            .with_system_prompt(summary_prompt)
            .with_max_tokens(4096);

            match agent.api_client.send_message(request).await {
                Ok(response) => {
                    let summary_text = response.text_content();
                    if summary_text.trim().is_empty() {
                        agent.output.eprintln("Compaction failed: LLM returned empty summary.");
                        return Ok(CommandResult::Continue);
                    }

                    // Build the summary system message
                    let full_summary = format!("--- Conversation Summary (compacted) ---\\\\n{summary_text}\\\\n--- End Summary ---");

                    // Replace old messages with the summary + keep recent
                    let recent: Vec<Message> = messages[split_point..].to_vec();
                    agent.session.messages.clear();
                    agent.session.add_message(Message::system(full_summary));
                    for msg in recent {
                        agent.session.messages.push(msg);
                    }
                    agent.session.dirty = true;

                    // Auto-save after compaction
                    let _ = agent.session.save(&agent.sessions_dir);

                    agent.output.println(format!(
                        "Session compacted: {} messages → 1 summary + {} recent (est. tokens: {})",
                        split_point,
                        keep_recent,
                        agent.session.estimated_tokens()
                    ));
                }
                Err(e) => {
                    agent.output.eprintln(format!("Compaction failed: LLM request error: {e}"));
                    agent.output.eprintln("Use '/compact truncate' for truncation-based compaction (no LLM call).");
                }
            }
            Ok(CommandResult::Continue)
        }

        // PI-aligned commands

        "copy" => {
            let last_assistant = agent
                .session
                .messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, pigs_core::MessageRole::Assistant))
                .map(|m| m.text_content());
            if let Some(text) = last_assistant {
                match clipboard_copy(&text) {
                    Ok(()) => agent.output.println(format!("Copied {} chars to clipboard.", text.len())),
                    Err(e) => agent.output.eprintln(format!("Failed to copy: {e}")),
                }
            } else {
                agent.output.eprintln("No assistant message to copy.");
            }
            Ok(CommandResult::Continue)
        }

        "name" => {
            if arg.is_empty() {
                agent.output.println(format!(
                    "Session name: {}",
                    agent.session.title.as_deref().unwrap_or("(unnamed)")
                ));
            } else {
                agent.session.title = Some(arg.to_string());
                agent.output.println(format!("Session name set to: {arg}"));
            }
            Ok(CommandResult::Continue)
        }

        "new" => {
            let _ = agent.session.save(&agent.sessions_dir);
            agent.session = pigs_session::Session::new(&agent.config.model, &agent.sessions_dir);
            agent.output.println(format!("New session started: {}", agent.session.session_id));
            Ok(CommandResult::Continue)
        }

        "resume" => {
            if arg.is_empty() {
                agent.output.eprintln("Usage: /resume <agent-code> (or use /ses <id>)");
                return Ok(CommandResult::Continue);
            }
            match agent.switch_session(arg) {
                Ok(()) => agent.output.println(format!("Resumed session {}.", agent.session.session_id)),
                Err(e) => agent.output.eprintln(format!("Failed to resume: {e}")),
            }
            Ok(CommandResult::Continue)
        }

        "hotkeys" => {
            agent.output.println("Keyboard shortcuts:");
            agent.output.println("  Enter          Submit message");
            agent.output.println("  Ctrl+C         Interrupt / clear input");
            agent.output.println("  Ctrl+D         Exit pig");
            agent.output.println("  /<tab>         Autocomplete slash commands");
            agent.output.println("  !<command>     Run bash directly");
            agent.output.println("  Up/Down        Navigate input history");
            Ok(CommandResult::Continue)
        }

        "settings" => {
            agent.output.println("Current settings:");
            agent.output.println(format!("  language:     {}", agent.language));
            agent.output.println(format!("  model:        {}", agent.config.model));
            agent.output.println(format!(
                "  permission:   {}",
                agent.permission_policy.active_mode
            ));
            agent.output.println(format!("  max_turns:    {:?}", agent.max_turns));
            agent.output.println(format!("  log_to_file:  {}", agent.config.log_to_file));
            agent.output.println(
                "  config path:  ~/.pigs/pig.toml"
            );
            agent.output.println("\nEdit the config file to change settings, then /reload.");
            Ok(CommandResult::Continue)
        }

        "changelog" => {
            agent.output.println("pig changelog:");
            agent.output.println("  0.1.3  Aligned CLI with PI: tool names, prompts, paths");
            agent.output.println("  0.1.2  Phase orchestration runtime + HTTP loopback");
            agent.output.println("  0.1.1  Three-protocol API support");
            agent.output.println("  0.1.0  Initial release");
            Ok(CommandResult::Continue)
        }

        "login" => {
            if arg.is_empty() {
                agent.output.eprintln("Usage: /login <provider>");
                agent.output.eprintln("Providers: anthropic, openai");
                return Ok(CommandResult::Continue);
            }
            agent.output.println(format!("Configure {arg} authentication by setting the API key:"));
            match arg.to_lowercase().as_str() {
                "anthropic" => agent.output.println("  export ANTHROPIC_API_KEY=sk-ant-..."),
                "openai" => agent.output.println("  export OPENAI_API_KEY=sk-..."),
                _ => agent.output.println(format!("  Unknown provider: {arg}")),
            }
            Ok(CommandResult::Continue)
        }

        "logout" => {
            if arg.is_empty() {
                agent.output.eprintln("Usage: /logout <provider>");
                return Ok(CommandResult::Continue);
            }
            agent.output.println(format!("To log out from {arg}, unset the corresponding environment variable."));
            Ok(CommandResult::Continue)
        }

        "fork" => {
            // Fork: save current session, create a fork with parent_id set
            let old_id = agent.session_id().to_string();
            let _ = agent.session.save(&agent.sessions_dir);
            let model = agent.config.model.clone();
            let new_session = agent.session.fork_from(&model, &agent.sessions_dir);
            let new_id = new_session.session_id.clone();
            agent.session = new_session;
            agent.output.println(format!("Forked session {old_id} -> new session {new_id} (parent: {old_id})"));
            Ok(CommandResult::Continue)
        }

        "clone" => {
            // Clone: duplicate current session at current position
            let _ = agent.session.save(&agent.sessions_dir);
            let model = agent.config.model.clone();
            let title = agent.session.title.clone();
            let new_session = agent.session.fork_from(&model, &agent.sessions_dir);
            let mut new_session = new_session;
            new_session.title = title;
            let new_id = new_session.session_id.clone();
            agent.session = new_session;
            agent.output.println(format!("Cloned to new session {new_id} (parent: previous)"));
            Ok(CommandResult::Continue)
        }

        "tree" => {
            // Tree: show session history as a simple list (no branching support yet)
            // Show session tree with parent/branch info
            agent.output.println(format!("Session: {} ({})", agent.session_id(), agent.session.title.as_deref().unwrap_or("(unnamed)")));
            if let Some(parent) = agent.session.parent_id() {
                agent.output.println(format!("  Parent: {parent}"));
            }
            agent.output.println(format!("  Messages: {}", agent.session.messages.len()));
            agent.output.println_empty();
            agent.output.println("Message tree:");
            for (i, msg) in agent.session.messages.iter().enumerate() {
                let role = match msg.role {
                    pigs_core::MessageRole::System => "SYS",
                    pigs_core::MessageRole::User => "USR",
                    pigs_core::MessageRole::Assistant => "AST",
                    pigs_core::MessageRole::Tool => "TOL",
                };
                let preview: String = msg.text_content().chars().take(60).collect();
                agent.output.println(format!("  {i:>3} [{role}] {preview}"));
            }
            if agent.session.is_fork() {
                agent.output.println("\nThis session is a fork. Use /sessions to see all sessions.");
            }
            Ok(CommandResult::Continue)
        }

        "import" => {
            // Import: load a session from a JSONL file
            if arg.is_empty() {
                agent.output.eprintln("Usage: /import <path-to-jsonl>");
                return Ok(CommandResult::Continue);
            }
            let path = std::path::PathBuf::from(arg);
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            match pigs_session::Session::load(parent, filename) {
                Ok(session) => {
                    agent.output.println(format!("Imported session {} ({} messages)", session.session_id, session.messages.len()));
                    agent.session = session;
                }
                Err(e) => {
                    agent.output.eprintln(format!("Failed to import: {e}"));
                }
            }
            Ok(CommandResult::Continue)
        }

        "scoped-models" => {
            // List models available for cycling (Ctrl+P in TUI)
            let models: Vec<String> = agent.config.models.iter()
                .map(|m| m.name.clone())
                .collect();
            if models.is_empty() {
                agent.output.println("No models configured. Use /model add to add a provider.");
            } else {
                agent.output.println("Scoped models (for Ctrl+P cycling):");
                for (i, m) in models.iter().enumerate() {
                    let marker = if m == &agent.config.model { " ← current" } else { "" };
                    agent.output.println(format!("  {}. {}{}", i + 1, m, marker));
                }
            }
            Ok(CommandResult::Continue)
        }

        "share" => {
            // Share: export session to a file (simplified — no GitHub gist integration)
            let default_path = format!("pig-session-{}.md", &agent.session_id()[..8.min(agent.session_id().len())]);
            let path = if arg.is_empty() { &default_path } else { arg };
            match export_session_markdown(agent, path) {
                Ok(()) => {
                    agent.output.println(format!("Session exported to: {path}"));
                    agent.output.println("Share this file with others to share the conversation.");
                }
                Err(e) => agent.output.eprintln(format!("Failed to export: {e}")),
            }
            Ok(CommandResult::Continue)
        }

        "theme" => {
            // Switch color theme (for TUI mode)
            let themes = ["dark", "light", "high-contrast"];
            if arg.is_empty() {
                agent.output.println(format!("Available themes: {}", themes.join(", ")));
                agent.output.println("Usage: /theme <name>");
                agent.output.println("Or press Ctrl+T in TUI to cycle themes.");
            } else if themes.contains(&arg) {
                agent.output.println(format!("Theme set to: {arg} (restart or /reload to apply in TUI)"));
            } else {
                agent.output.eprintln(format!("Unknown theme: {}. Available: {}", arg, themes.join(", ")));
            }
            Ok(CommandResult::Continue)
        }

        "sub" => {
            // /sub is deprecated — merged into /ses and /back
            // Keep as compatibility alias for /ses
            agent.output.println("Note: /sub is now /ses. Use /ses to list and navigate agents.");
            // Delegate to /ses behavior
            if arg.is_empty() || arg == "list" {
                // Re-run /ses list by falling through
                // Show sub-agents in memory
                let mgr = agent.sub_agent_manager.lock()
                    .unwrap_or_else(|e| e.into_inner());
                if mgr.is_empty() {
                    agent.output.println("No sub-agents in memory.");
                    agent.output.println("Use the 'spawn' tool to create sub-agents.");
                } else {
                    let focus = mgr.current_focus().to_string();
                    for (id, task, status, mode) in mgr.list() {
                        let marker = if focus == id { " ← viewing" } else { "" };
                        let task_preview: String = task.chars().take(40).collect();
                        agent.output.println(format!("  {id:<8} [{}] {}  {}{}",
                            mode.as_str(), status.display(), task_preview, marker));
                    }
                }
            } else if arg == "back" {
                let prev = agent.sub_agent_manager.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .switch_back();
                if let Some(id) = prev {
                    // Check if target is in memory or needs disk load
                    let in_mem = agent.sub_agent_manager.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_in_memory(&id);
                    if !in_mem && id != agent.session.session_id {
                        let _ = agent.switch_session(&id);
                    }
                    agent.output.println(format!("Navigated back to: {id}"));
                } else {
                    agent.output.println("Already at the beginning of navigation history.");
                }
            } else {
                // /sub <id> → treat as /ses <id>
                let id = arg.trim();
                let in_memory = agent.sub_agent_manager.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_in_memory(id);
                if in_memory {
                    agent.sub_agent_manager.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .switch_to(id);
                    agent.output.println(format!("Switched focus to: {id}"));
                } else {
                    match agent.switch_session(id) {
                        Ok(()) => agent.output.println(format!("Loaded session {} ({})",
                            agent.session.session_id,
                            agent.session.display_title())),
                        Err(e) => agent.output.eprintln(format!("Failed to load: {e}")),
                    }
                }
            }
            Ok(CommandResult::Continue)
        }

        "back" => {
            // Navigate back in history (pointer -1, no reload, doesn't affect running agents)
            let prev = agent.sub_agent_manager.lock()
                .unwrap_or_else(|e| e.into_inner())
                .switch_back();
            match prev {
                Some(id) => {
                    // If target not in memory, load from disk
                    let in_mem = agent.sub_agent_manager.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_in_memory(&id);
                    if !in_mem && id != agent.session.session_id {
                        let _ = agent.switch_session(&id);
                    }
                    agent.output.println(format!("← {id}"));
                }
                None => agent.output.println("Already at the beginning of navigation history."),
            }
            Ok(CommandResult::Continue)
        }

        "next" => {
            // Navigate forward in history (pointer +1, /back's reverse)
            let fwd = agent.sub_agent_manager.lock()
                .unwrap_or_else(|e| e.into_inner())
                .switch_forward();
            match fwd {
                Some(id) => {
                    let in_mem = agent.sub_agent_manager.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_in_memory(&id);
                    if !in_mem && id != agent.session.session_id {
                        let _ = agent.switch_session(&id);
                    }
                    agent.output.println(format!("→ {id}"));
                }
                None => agent.output.println("Already at the end of navigation history."),
            }
            Ok(CommandResult::Continue)
        }

        _ => {
            agent.output.eprintln(format!("Unknown command: /{}. {}", raw_cmd,
                i18n::t(agent.language, "unknown_command_hint")
            ));
            Ok(CommandResult::Continue)
        }
    }
}

fn export_session_markdown(agent: &Agent, path: &str) -> anyhow::Result<()> {
    use std::fmt::Write as _;

    let mut out = String::new();
    writeln!(out, "# Pigs Session Export")?;
    writeln!(out)?;
    writeln!(out, "- **Session ID**: `{}`", agent.session.session_id)?;
    writeln!(out, "- **Model**: `{}`", agent.session.model)?;
    writeln!(out, "- **Messages**: {}", agent.session.message_count())?;
    writeln!(out, "- **Usage**: {}", agent.session.total_usage)?;
    writeln!(out, "- **Exported**: {}", chrono::Utc::now().to_rfc3339())?;
    writeln!(out)?;
    writeln!(out, "---")?;
    writeln!(out)?;

    for (i, msg) in agent.session.messages.iter().enumerate() {
        let role = match msg.role {
            pigs_core::MessageRole::System => "System",
            pigs_core::MessageRole::User => "User",
            pigs_core::MessageRole::Assistant => "Assistant",
            pigs_core::MessageRole::Tool => "Tool",
        };
        writeln!(out, "## {}. {role}", i + 1)?;
        writeln!(out)?;

        let text = msg.text_content();
        if !text.is_empty() {
            writeln!(out, "{text}")?;
            writeln!(out)?;
        }

        for (id, name, input) in msg.tool_uses() {
            writeln!(out, "### Tool call: `{name}` (`{id}`)")?;
            writeln!(out)?;
            writeln!(out, "```json")?;
            writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
            )?;
            writeln!(out, "```")?;
            writeln!(out)?;
        }

        // Tool results
        for block in &msg.content {
            if let pigs_core::ContentBlock::ToolResult {
                tool_use_id,
                output,
                is_error,
            } = block
            {
                let tag = if *is_error { "error" } else { "result" };
                writeln!(out, "### Tool {tag}: `{tool_use_id}`")?;
                writeln!(out)?;
                writeln!(out, "```")?;
                writeln!(out, "{output}")?;
                writeln!(out, "```")?;
                writeln!(out)?;
            }
        }
    }

    std::fs::write(path, out)?;
    Ok(())
}

fn print_mcp_help(out: &mut crate::output::OutputSink) {
    out.println("MCP commands:");
    out.println("  /mcp list                     List connected servers and tools");
    out.println("  /mcp tools                    List MCP tools with descriptions");
    out.println("  /mcp connect <name> <cmd> ... Connect a stdio MCP server");
    out.println("  /mcp disconnect <name>        Disconnect a server");
    out.println_empty();
    out.println("Config example (~/.pigs/pig.toml):");
    out.println("  [[mcp_servers]]");
    out.println("  name = \"filesystem\"");
    out.println("  command = \"npx\"");
    out.println("  args = [\"-y\", \"@modelcontextprotocol/server-filesystem\", \".\"]");
    out.println("  enabled = true");
}

async fn handle_mcp_command(agent: &mut Agent, arg: &str) -> anyhow::Result<()> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let sub_raw = parts.first().copied().unwrap_or("");
    let sub = if sub_raw.is_empty() {
        "list"
    } else {
        canonicalize_mcp_sub(sub_raw)
    };

    match sub {
        "list" => {
            let servers = agent.list_mcp_servers().await;
            if servers.is_empty() {
                agent.output.println("No MCP servers connected.");
                agent.output.println("Use: /mcp connect <name> <command> [args...]");
                agent.output.println("Or configure [[mcp_servers]] in ~/.pigs/pig.toml");
            } else {
                agent.output.println("Connected MCP servers:");
                for name in &servers {
                    agent.output.println(format!("  - {name}"));
                }
                let tools = agent.list_mcp_tools().await;
                if !tools.is_empty() {
                    agent.output.println(format!("\nMCP tools ({}):", tools.len()));
                    for t in tools {
                        agent.output.println(format!(
                            "  - mcp_{}_{}  ({})",
                            t.server_name.replace(
                                |c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-',
                                "_"
                            ),
                            t.name.replace(
                                |c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-',
                                "_"
                            ),
                            t.server_name
                        ));
                    }
                }
            }
        }
        "tools" => {
            let tools = agent.list_mcp_tools().await;
            if tools.is_empty() {
                agent.output.println("No MCP tools available. Connect a server first.");
            } else {
                agent.output.println(format!("MCP tools ({}):", tools.len()));
                for t in &tools {
                    let desc = if t.description.is_empty() {
                        "(no description)"
                    } else {
                        t.description.as_str()
                    };
                    agent.output.println(format!("  [{}] {}", t.server_name, t.name));
                    agent.output.println(format!("    {desc}"));
                }
            }
        }
        "connect" => {
            if parts.len() < 3 {
                agent.output.println("Usage: /mcp connect <name> <command> [args...]");
                agent.output.println("Example: /mcp connect filesystem npx -y @modelcontextprotocol/server-filesystem .");
                return Ok(());
            }
            let name = parts[1];
            let command = parts[2];
            let args: Vec<String> = parts[3..].iter().map(|s| s.to_string()).collect();
            match agent
                .connect_mcp_server(name, command, args, Default::default())
                .await
            {
                Ok(count) => agent.output.println(format!("Connected MCP server '{name}' with {count} tool(s).")),
                Err(e) => agent.output.eprintln(format!("Failed to connect MCP server '{name}': {e}")),
            }
        }
        "disconnect" => {
            if parts.len() < 2 {
                agent.output.println("Usage: /mcp disconnect <name>");
                return Ok(());
            }
            let name = parts[1];
            match agent.disconnect_mcp_server(name).await {
                Ok(()) => {
                    agent.output.println(format!("Disconnected MCP server '{name}'. (Tools remain until restart.)"))
                }
                Err(e) => agent.output.eprintln(format!("Failed to disconnect '{name}': {e}")),
            }
        }
        "help" => {
            print_mcp_help(&mut agent.output);
        }
        other => {
            agent.output.eprintln(format!("Unknown MCP subcommand: {other}"));
            print_mcp_help(&mut agent.output);
        }
    }
    Ok(())
}

fn print_help(out: &mut crate::output::OutputSink, lang: Language) {
    if lang.is_zh() {
        out.println("Pigs Agent — 可用命令（英文 / 中文 / 拼音）：");
        out.println_empty();
        out.println("  /help, /h, /? , /帮助, /bangzhu              显示帮助");
        out.println("  /lang, /language, /语言, /中文, /yuyan, /zhongwen");
        out.println("                                              查看或设置语言 (en|zh)");
        out.println("  /model, /模型, /moxing [add|添加|<id>]      查看/切换/引导添加模型");
        out.println("  /mode, /模式, /权限, /moshi, /quanxian      设置权限模式");
        out.println("  /tools, /工具, /gongju                      列出工具");
        out.println("  /todo, /todos, /待办, /任务, /daiban, /renwu 显示待办");
        out.println("  /status, /状态, /仪表盘, /zhuangtai, /yibiaopan  状态仪表盘");
        out.println("  /info, /信息, /xinxi                        当前会话信息");
        out.println("  /title, /标题, /biaoti                      设置会话标题");
        out.println("  /cost, /费用, /开销, /成本, /feiyong, /kaixiao, /chengben  费用");
        out.println("  /history, /历史, /lishi                     会话历史摘要");
        out.println("  /mcp                                        管理 MCP 服务器");
        out.println("      子命令: list/列表/liebiao, tools/工具/gongju,");
        out.println("              connect/连接/lianjie, disconnect/断开/duankai");
        out.println("  /skills, /技能, /jineng [reload]            技能目录（全文用 skill 工具）");
        out.println("  /rules, /规则, /guize [reload]              项目规则");
        out.println("  /memory, /记忆, /jiyi ...                   跨会话记忆");
        out.println("  /export, /导出, /daochu [path]              导出会话为 markdown");
        out.println("  /hooks, /钩子, /gouzi                       查看 hooks");
        out.println("  /doctor, /诊断, /体检, /zhenduan, /tijian   环境健康检查");
        out.println("  /models, /模型列表, /moxingliebiao          供应商/模型目录");
        out.println("  /init, /初始化, /chushihua                  创建默认配置");
        out.println("  /reload, /重载, /zhongzai, /chongzai        热重载配置");
        out.println("  /compact, /压缩, /精简, /yasuo, /jingjian   手动压缩上下文（LLM 摘要式）");
        out.println("  /compact truncate, /压缩 截断               截断式压缩（不调 LLM）");
        out.println("  /clear, /清空, /清除, /qingkong, /qingchu   清空当前会话");
        out.println("  /save, /保存, /baocun                       保存当前会话");
        out.println("  /sessions, /会话, /huihua ...               管理会话");
        out.println("      子命令: list/列表, open/打开/dakai, rm/删除/shanchu,");
        out.println("              search/搜索/sousuo, current/当前/dangqian");
        out.println("  /undo, /撤销, /回退, /chexiao, /huitui [list] 撤销最近写操作");
        out.println("  /quit, /q, /exit, /退出, /tuichu, /likai    退出");
        out.println_empty();
        out.println("中文与拼音别名在任何语言设置下都可用；默认语言为中文 (zh)。");
        out.println("输入其它文字将作为提示发送给 Agent。");
    } else {
        out.println("Pigs Agent — Available Commands (English / 中文 / pinyin):");
        out.println_empty();
        out.println("  /help, /h, /?, /帮助, /bangzhu              Show this help");
        out.println("  /lang, /language, /语言, /中文, /yuyan, /zhongwen");
        out.println("                                              Show or set language (en|zh)");
        out.println("  /model, /模型, /moxing [add|添加|<id>]      status / switch / guided add");
        out.println("  /mode, /模式, /权限, /moshi, /quanxian      Permission mode");
        out.println("  /tools, /工具, /gongju                      List tools");
        out.println("  /todo, /todos, /待办, /daiban               Todo list");
        out.println("  /status, /状态, /zhuangtai                  Dashboard");
        out.println("  /info, /信息, /xinxi                        Session info");
        out.println("  /title, /标题, /biaoti                      Set session title");
        out.println("  /cost, /费用, /feiyong                      Token usage / cost");
        out.println("  /history, /历史, /lishi                     History summary");
        out.println("  /mcp                                        MCP servers");
        out.println("      subs: list/列表/liebiao, tools/工具/gongju,");
        out.println("            connect/连接/lianjie, disconnect/断开/duankai");
        out.println("  /skills, /技能, /jineng [reload]            Skill catalog");
        out.println("  /rules, /规则, /guize [reload]              Project rules");
        out.println("  /memory, /记忆, /jiyi ...                   Memory notes");
        out.println("  /export, /导出, /daochu [path]              Export session");
        out.println("  /hooks, /钩子, /gouzi                       Tool hooks");
        out.println("  /doctor, /诊断, /zhenduan, /tijian          Health checks");
        out.println("  /models, /模型列表, /moxingliebiao          Model catalog");
        out.println("  /init, /初始化, /chushihua                  Create config");
        out.println("  /reload, /重载, /zhongzai, /chongzai        Reload config");
        out.println("  /compact, /压缩, /yasuo, /jingjian          Compact context (LLM summary)");
        out.println("  /compact truncate                            Truncation compaction (no LLM)");
        out.println("  /clear, /清空, /qingkong                    Clear session");
        out.println("  /save, /保存, /baocun                       Save session");
        out.println("  /sessions, /会话, /huihua ...               Manage sessions");
        out.println("      subs: list/列表, open/打开/dakai, rm/删除/shanchu,");
        out.println("            search/搜索/sousuo, current/当前/dangqian");
        out.println("  /undo, /撤销, /chexiao, /huitui [list]      Undo last write");
        out.println("  /quit, /q, /exit, /退出, /tuichu            Exit");
        out.println_empty();
        out.println("Chinese / pinyin aliases work regardless of language setting.");
        out.println("Default language is Chinese (zh); use /lang en to switch.");
        out.println("Type any other text to send it to the agent as a prompt.");
    }
}

/// Copy text to the system clipboard.
fn clipboard_copy(text: &str) -> anyhow::Result<()> {
    // Try using the system clipboard via a shell command
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("Set-Clipboard -Value {}", shell_escape(text))])
            .output()?;
        if !output.status.success() {
            anyhow::bail!("powershell Set-Clipboard failed");
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(stdin) = &mut child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        // Try xclip first, then xsel
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = &mut child.stdin {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
            return Ok(());
        }
        if let Ok(mut child) = std::process::Command::new("xsel")
            .args(["--clipboard", "--input"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = &mut child.stdin {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
            return Ok(());
        }
        anyhow::bail!("No clipboard utility found (xclip/xsel)")
    }
}

/// Escape a string for PowerShell single-quoted argument.
#[cfg(target_os = "windows")]
fn shell_escape(s: &str) -> String {
    // PowerShell single-quote escaping: double the single quotes
    format!("'{}'", s.replace('\'', "''"))
}
