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
    let bare_chinese_switch = matches!(
        raw_cmd,
        "中文" | "zhongwen" | "Zhongwen" | "ZHONGWEN"
    ) && arg.is_empty();

    match cmd {
        "help" => {
            print_help(agent.language);
            Ok(CommandResult::Continue)
        }

        "quit" => {
            println!("{}", i18n::t(agent.language, "goodbye"));
            Ok(CommandResult::Quit)
        }

        "lang" => {
            if bare_chinese_switch {
                agent.set_language(Language::Zh);
                println!("{}", i18n::t(agent.language, "lang_set_zh"));
                return Ok(CommandResult::Continue);
            }
            if arg.is_empty() {
                println!(
                    "{}: {} ({})",
                    i18n::t(agent.language, "language"),
                    agent.language.as_str(),
                    agent.language.display_name()
                );
                println!("{}", i18n::t(agent.language, "lang_usage"));
            } else {
                match arg.parse::<Language>() {
                    Ok(lang) => {
                        agent.set_language(lang);
                        match lang {
                            Language::En => println!("{}", i18n::t(agent.language, "lang_set_en")),
                            Language::Zh => println!("{}", i18n::t(agent.language, "lang_set_zh")),
                        }
                    }
                    Err(e) => eprintln!("{e}"),
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
                println!(
                    "Current permission mode: {}",
                    agent.permission_policy.active_mode
                );
                println!("Usage: /mode <readonly|workspace_write|danger|ask|allow>");
                println!("       (alias: /模式 /权限 /moshi /quanxian)");
            } else {
                match arg {
                    "ask" => {
                        agent.permission_policy = agent.permission_policy.clone().always_ask();
                        println!("Permission mode: ask (prompt for every tool)");
                    }
                    "allow" => {
                        agent.permission_policy = agent.permission_policy.clone().allow_all();
                        println!("Permission mode: allow (all tools allowed without prompting)");
                    }
                    _ => match PermissionMode::from_str(arg) {
                        Ok(mode) => {
                            agent.set_permission_mode(mode.clone());
                            println!("Permission mode: {mode}");
                        }
                        Err(e) => eprintln!("{e}"),
                    },
                }
            }
            Ok(CommandResult::Continue)
        }

        "clear" => {
            agent.clear_session();
            println!("{}", i18n::t(agent.language, "session_cleared"));
            Ok(CommandResult::Continue)
        }

        "save" => {
            match agent.session.save(&agent.sessions_dir) {
                Ok(()) => println!("Session saved: {}", agent.session_id()),
                Err(e) => eprintln!("Failed to save session: {e}"),
            }
            Ok(CommandResult::Continue)
        }

        "sessions" => {
            let parts: Vec<&str> = arg.splitn(2, " ").collect();
            let sub_raw = parts.first().copied().unwrap_or("");
            let sub = if sub_raw.is_empty() {
                "list"
            } else {
                canonicalize_sessions_sub(sub_raw)
            };
            match sub {
                "list" => {
                    match Agent::list_sessions() {
                        Ok(sessions) => {
                            if sessions.is_empty() {
                                println!("No saved sessions.");
                            } else {
                                println!(
                                    "{:<10} {:<24} {:<18} {:<6} {:<16}",
                                    "ID", "Title", "Model", "Msgs", "Updated"
                                );
                                println!("{}", "-".repeat(80));
                                for s in sessions.iter().take(20) {
                                    let short_id = &s.session_id[..8.min(s.session_id.len())];
                                    let updated = s.updated_at.format("%Y-%m-%d %H:%M");
                                    let title = s
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| "(untitled)".to_string());
                                    let title = if title.chars().count() > 22 {
                                        let t: String = title.chars().take(19).collect();
                                        format!("{t}...")
                                    } else {
                                        title
                                    };
                                    println!(
                                        "{short_id:<10} {title:<24} {:<18} {:<6} {updated}",
                                        s.model, s.message_count
                                    );
                                }
                            }
                        }
                        Err(e) => eprintln!("Failed to list sessions: {e}"),
                    }
                }
                "rm" => {
                    let id = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    if id.is_empty() {
                        println!("Usage: /sessions rm <id-or-prefix>");
                    } else {
                        match Agent::delete_session(id) {
                            Ok(path) => {
                                println!("Deleted session file: {}", path.display());
                                let cur = agent.session.short_id();
                                if agent.session.session_id.starts_with(id) || id.starts_with(cur) {
                                    println!("Note: current in-memory session was not cleared. Use /clear if needed.");
                                }
                            }
                            Err(e) => eprintln!("Failed to delete session: {e}"),
                        }
                    }
                }
                "open" => {
                    let id = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    if id.is_empty() {
                        println!("Usage: /sessions open <id-or-prefix>");
                    } else {
                        match agent.switch_session(id) {
                            Ok(()) => {
                                println!(
                                    "Switched to session {} ({})",
                                    agent.session.short_id(),
                                    agent.session.display_title()
                                );
                            }
                            Err(e) => eprintln!("Failed to open session: {e}"),
                        }
                    }
                }
                "search" => {
                    let q = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    if q.is_empty() {
                        println!("Usage: /sessions search <text>");
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
                                    println!("No sessions matched `{q}`.");
                                } else {
                                    println!("Matches for `{q}` ({}):", filtered.len());
                                    println!(
                                        "{:<10} {:<24} {:<18} {:<6} {:<16}",
                                        "ID", "Title", "Model", "Msgs", "Updated"
                                    );
                                    println!("{}", "-".repeat(80));
                                    for s in filtered.iter().take(50) {
                                        let short_id = &s.session_id[..8.min(s.session_id.len())];
                                        let updated = s.updated_at.format("%Y-%m-%d %H:%M");
                                        let title = s
                                            .title
                                            .clone()
                                            .unwrap_or_else(|| "(untitled)".to_string());
                                        let title = if title.chars().count() > 22 {
                                            let t: String = title.chars().take(19).collect();
                                            format!("{t}...")
                                        } else {
                                            title
                                        };
                                        println!(
                                            "{short_id:<10} {title:<24} {:<18} {:<6} {updated}",
                                            s.model, s.message_count
                                        );
                                    }
                                }
                            }
                            Err(e) => eprintln!("Failed to search sessions: {e}"),
                        }
                    }
                }
                "current" => {
                    println!(
                        "Current: {}  title={}  model={}  msgs={}",
                        agent.session.short_id(),
                        agent.session.display_title(),
                        agent.session.model,
                        agent.session.message_count()
                    );
                }
                _ => {
                    println!("Session commands:");
                    println!("  /sessions                List saved sessions");
                    println!("  /sessions current        Show current session");
                    println!("  /sessions open <id>      Switch to a saved session");
                    println!("  /sessions rm <id>        Delete a saved session file");
                    println!("  /sessions search <text>  Filter by id/title/model");
                }
            }
            Ok(CommandResult::Continue)
        }

        "tools" => {
            if agent.no_tools {
                println!("Tools are disabled (--no-tools flag).");
            } else {
                let names = agent.tool_registry.names();
                println!("Available tools ({}):", names.len());
                for name in &names {
                    println!("  - {name}");
                }
            }
            Ok(CommandResult::Continue)
        }

        "todo" => {
            let todos = agent.todo_list.lock();
            match todos {
                Ok(todos) => {
                    if todos.is_empty() {
                        println!("Todo list is empty.");
                    } else {
                        let total = todos.len();
                        let completed = todos
                            .iter()
                            .filter(|t| matches!(t.status, pigs_tools::todo_write::TodoStatus::Completed))
                            .count();
                        println!("Todo list ({completed}/{total} completed):");
                        println!("{}", "-".repeat(60));
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
                            println!("  {} {priority_tag}  {}. {}", status_icon, i + 1, item.content);
                        }
                    }
                }
                Err(e) => eprintln!("Failed to read todo list: {e}"),
            }
            Ok(CommandResult::Continue)
        }

        "status" => {
            println!("Pigs status");
            println!("{}", "-".repeat(60));
            println!("Session:    {} ({})", agent.session.display_title(), agent.session.short_id());
            println!("Model:      {}", agent.api_client.model());
            println!(
                "{}:   {} ({})",
                i18n::t(agent.language, "language"),
                agent.language.as_str(),
                agent.language.display_name()
            );
            println!("Permission: {}", agent.permission_policy.active_mode);
            println!("Messages:   {}", agent.session.message_count());
            println!("Est tokens: {}", agent.session.estimated_tokens());
            println!("Usage:      {}", agent.session.total_usage);
            if let Some(cost) = agent
                .session
                .total_usage
                .estimate_cost_for_model(agent.api_client.model())
            {
                println!("Est cost:   ${cost:.4}");
            }
            println!(
                "Tools:      {}",
                if agent.no_tools {
                    0
                } else {
                    agent.tool_registry.len()
                }
            );
            println!("Skills:     {}", agent.skills.len());
            println!("Rules:      {}", agent.rules.len());
            println!("Memory:     {} note(s)", agent.memory.len());
            let servers = agent.list_mcp_servers().await;
            println!("MCP:        {} server(s)", servers.len());
            if !servers.is_empty() {
                println!("            {}", servers.join(", "));
            }
            println!("Workspace:  {}", agent.workspace_root.display());
            println!("Logs:       {}", pigs_config::AppConfig::logs_dir().display());
            Ok(CommandResult::Continue)
        }

        "info" => {
            println!("Session ID:   {}", agent.session_id());
            println!("Model:        {}", agent.api_client.model());
            println!("Permission:   {}", agent.permission_policy.active_mode);
            println!("Messages:     {}", agent.session.message_count());
            println!("Est. tokens:  {}", agent.session.estimated_tokens());
            println!("Usage:        {}", agent.session.total_usage);
            println!("Tools:        {}", if agent.no_tools { "disabled" } else { "enabled" });
            println!("Max turns:    {}", agent.max_turns);
            println!("Skills:       {}", agent.skills.len());
            Ok(CommandResult::Continue)
        }

        "title" => {
            if arg.is_empty() {
                println!("Current title: {}", agent.session.display_title());
                println!("Usage: /title <new title>");
            } else {
                agent.session.set_title(arg);
                if let Err(e) = agent.session.save(&agent.sessions_dir) {
                    eprintln!("Failed to save session title: {e}");
                } else {
                    println!("Title set to: {}", agent.session.display_title());
                }
            }
            Ok(CommandResult::Continue)
        }

        "cost" => {
            let usage = &agent.session.total_usage;
            let model = agent.api_client.model();
            println!("Token usage for this session:");
            println!("  Model:         {model}");
            println!("  Input tokens:  {}", usage.input_tokens);
            println!("  Output tokens: {}", usage.output_tokens);
            if let Some(cached) = usage.cache_read_tokens {
                println!("  Cached tokens: {cached}");
            }
            println!("  Total tokens:  {}", usage.total_tokens());
            match usage.estimate_cost_for_model(model) {
                Some(cost) => {
                    println!("  Est. cost:     ${cost:.4} USD (approximate list pricing)");
                }
                None => {
                    println!("  Est. cost:     (unknown model pricing)");
                }
            }
            Ok(CommandResult::Continue)
        }

        "init" => {
            // Create default config file
            match agent.config.clone().save() {
                Ok(()) => {
                    let config_path = pigs_config::AppConfig::config_path();
                    println!("Config file created at: {}", config_path.display());
                    println!("Edit it to set your API keys and preferences.");
                }
                Err(e) => eprintln!("Failed to create config: {e}"),
            }
            Ok(CommandResult::Continue)
        }

        "reload" => {
            match agent.reload_config() {
                Ok(()) => {
                    println!("{}", i18n::t(agent.language, "config_reloaded"));
                    println!("  Model:      {}", agent.api_client.model());
                    println!("  Permission: {}", agent.permission_policy.active_mode);
                    println!(
                        "  {}:     {} ({})",
                        i18n::t(agent.language, "language"),
                        agent.language.as_str(),
                        agent.language.display_name()
                    );
                    println!("  Max turns:  {}", agent.max_turns);
                }
                Err(e) => eprintln!("Failed to reload config: {e}"),
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
                        println!("Memory is empty.");
                        println!("Use: /memory add [--global] <note>");
                    } else {
                        println!("Memory notes ({}):", agent.memory.len());
                        if !agent.memory.global.is_empty() {
                            println!("[global]");
                            for (i, n) in agent.memory.global.iter().enumerate() {
                                println!("  {}. {n}", i + 1);
                            }
                        }
                        if !agent.memory.project.is_empty() {
                            println!("[project]");
                            for (i, n) in agent.memory.project.iter().enumerate() {
                                println!("  {}. {n}", i + 1);
                            }
                        }
                    }
                }
                "add" => {
                    let rest = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    let (source, note) = if let Some(n) = rest.strip_prefix("--global ") {
                        (pigs_config::MemorySource::Global, n.trim())
                    } else if rest == "--global" {
                        println!("Usage: /memory add [--global] <note>");
                        return Ok(CommandResult::Continue);
                    } else {
                        (pigs_config::MemorySource::Project, rest)
                    };
                    if note.is_empty() {
                        println!("Usage: /memory add [--global] <note>");
                    } else {
                        match pigs_config::add_memory_note(&agent.workspace_root, source, note) {
                            Ok(path) => {
                                agent.reload_memory();
                                println!(
                                    "Added {} memory note -> {}",
                                    source.as_str(),
                                    path.display()
                                );
                            }
                            Err(e) => eprintln!("Failed to add memory: {e}"),
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
                        println!("Usage: /memory rm [--global] <substring>");
                    } else {
                        match pigs_config::remove_memory_notes(
                            &agent.workspace_root,
                            source,
                            needle,
                        ) {
                            Ok((n, path)) => {
                                agent.reload_memory();
                                println!(
                                    "Removed {n} {} note(s) from {}",
                                    source.as_str(),
                                    path.display()
                                );
                            }
                            Err(e) => eprintln!("Failed to remove memory: {e}"),
                        }
                    }
                }
                "reload" => {
                    agent.reload_memory();
                    println!("Memory reloaded ({} notes).", agent.memory.len());
                }
                _ => {
                    println!("Memory commands:");
                    println!("  /memory list");
                    println!("  /memory add [--global] <note>");
                    println!("  /memory rm  [--global] <substring>");
                    println!("  /memory reload");
                }
            }
            Ok(CommandResult::Continue)
        }

        "rules" => {
            if arg == "reload" {
                agent.reload_rules();
                println!("Rules reloaded ({}).", agent.rules.len());
            }
            if agent.rules.is_empty() {
                println!("No project rules loaded.");
                println!("Add markdown files under .pigs/rules/");
            } else {
                println!("Project rules ({}):", agent.rules.len());
                for rule in &agent.rules {
                    println!("  - {}  ({})", rule.name, rule.path.display());
                }
            }
            Ok(CommandResult::Continue)
        }

        "skills" => {
            if arg == "reload" {
                agent.reload_skills();
                println!("Skills reloaded ({} skill(s)).", agent.skills.len());
            }
            if agent.skills.is_empty() {
                println!("No skills loaded.");
                println!("Place SKILL.md or *.md files in (first match of a name wins):");
                println!("  ~/.pigs/skills/          # pigs user");
                println!("  ~/.agents/skills/        # common user agent skills");
                println!("  .pigs/skills/            # pigs project");
                println!("  .agents/skills/          # common project skills (e.g. ARIS)");
                println!("  skills/                  # workspace root");
            } else {
                println!(
                    "Skill catalog ({}): names+descriptions in system prompt; full body via tool `skill`",
                    agent.skills.len()
                );
                for skill in &agent.skills {
                    let desc = if skill.description.is_empty() {
                        "(no description)"
                    } else {
                        skill.description.as_str()
                    };
                    println!("  - {}  {}", skill.name, desc);
                    println!("    {}", skill.path.display());
                }
            }
            Ok(CommandResult::Continue)
        }

        "undo" => {
            match arg {
                "list" | "ls" => {
                    let items = agent.list_write_snapshots();
                    if items.is_empty() {
                        println!("No undo snapshots.");
                    } else {
                        println!("Undoable write batches (most recent first):");
                        for (i, b) in items.iter().enumerate() {
                            println!(
                                "  {}. {}  tool={}  files={}  at={}",
                                i + 1,
                                b.id,
                                b.tool_name,
                                b.files.len(),
                                b.created_at
                            );
                            for f in b.files.iter().take(5) {
                                println!("      - {}", f.path.display());
                            }
                        }
                    }
                }
                "" => match agent.undo_last_write() {
                    Ok(report) => {
                        println!("Undo complete:");
                        for line in report {
                            println!("  {line}");
                        }
                    }
                    Err(e) => eprintln!("Undo failed: {e}"),
                },
                _ => {
                    println!("Usage:");
                    println!("  /undo        Undo last write tool batch");
                    println!("  /undo list   List undoable batches");
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
                Ok(()) => println!("Session exported to: {path}"),
                Err(e) => eprintln!("Failed to export session: {e}"),
            }
            Ok(CommandResult::Continue)
        }

        "doctor" => {
            let items = crate::doctor::run_doctor(agent);
            crate::doctor::print_doctor_report(&items);
            Ok(CommandResult::Continue)
        }

        "models" => {
            crate::models::print_models(&agent.config, &agent.config.model, agent.api_client.model());
            Ok(CommandResult::Continue)
        }

        "hooks" => {
            println!("{}", crate::hooks::summarize_hooks(&agent.config.hooks));
            println!();
            println!("Configure hooks in ~/.pigs/config.toml:");
            println!("  [[hooks.pre_tool_use]]");
            println!("  matcher = \"bash\"");
            println!("  command = \"echo checking $PIGS_TOOL_NAME\"");
            println!("  timeout = 10");
            println!("  enabled = true");
            Ok(CommandResult::Continue)
        }

        "history" => {
            let messages = &agent.session.messages;
            if messages.is_empty() {
                println!("No messages in this session.");
            } else {
                println!(
                    "Session history ({} messages, est. {} tokens):",
                    messages.len(),
                    agent.session.estimated_tokens()
                );
                println!("{}", "-".repeat(60));
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
                    println!("{:>3}. [{role:<9}] {preview}", i + 1);
                }
            }
            Ok(CommandResult::Continue)
        }

        "compact" => {
            use pigs_session::{compact_session, CompactConfig};
            let config = CompactConfig {
                token_threshold: agent.config.compact_token_threshold,
                keep_recent: agent.config.compact_keep_recent.max(1),
                summary_message_chars: 400,
                force: true,
            };
            let compacted = compact_session(&mut agent.session, &config);
            if compacted {
                println!(
                    "Session compacted (est. tokens: {})",
                    agent.session.estimated_tokens()
                );
            } else {
                println!(
                    "No compaction needed (est. tokens: {})",
                    agent.session.estimated_tokens()
                );
            }
            Ok(CommandResult::Continue)
        }

        _ => {
            eprintln!(
                "Unknown command: /{raw_cmd}. {}",
                i18n::t(agent.language, "unknown_command_hint")
            );
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

fn print_mcp_help() {
    println!("MCP commands:");
    println!("  /mcp list                     List connected servers and tools");
    println!("  /mcp tools                    List MCP tools with descriptions");
    println!("  /mcp connect <name> <cmd> ... Connect a stdio MCP server");
    println!("  /mcp disconnect <name>        Disconnect a server");
    println!();
    println!("Config example (~/.pigs/config.toml):");
    println!("  [[mcp_servers]]");
    println!("  name = \"filesystem\"");
    println!("  command = \"npx\"");
    println!("  args = [\"-y\", \"@modelcontextprotocol/server-filesystem\", \".\"]");
    println!("  enabled = true");
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
                println!("No MCP servers connected.");
                println!("Use: /mcp connect <name> <command> [args...]");
                println!("Or configure [[mcp_servers]] in ~/.pigs/config.toml");
            } else {
                println!("Connected MCP servers:");
                for name in &servers {
                    println!("  - {name}");
                }
                let tools = agent.list_mcp_tools().await;
                if !tools.is_empty() {
                    println!("\nMCP tools ({}):", tools.len());
                    for t in tools {
                        println!(
                            "  - mcp_{}_{}  ({})",
                            t.server_name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-', "_"),
                            t.name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-', "_"),
                            t.server_name
                        );
                    }
                }
            }
        }
        "tools" => {
            let tools = agent.list_mcp_tools().await;
            if tools.is_empty() {
                println!("No MCP tools available. Connect a server first.");
            } else {
                println!("MCP tools ({}):", tools.len());
                for t in &tools {
                    let desc = if t.description.is_empty() {
                        "(no description)"
                    } else {
                        t.description.as_str()
                    };
                    println!("  [{server}] {name}", server = t.server_name, name = t.name);
                    println!("    {desc}");
                }
            }
        }
        "connect" => {
            if parts.len() < 3 {
                println!("Usage: /mcp connect <name> <command> [args...]");
                println!("Example: /mcp connect filesystem npx -y @modelcontextprotocol/server-filesystem .");
                return Ok(());
            }
            let name = parts[1];
            let command = parts[2];
            let args: Vec<String> = parts[3..].iter().map(|s| s.to_string()).collect();
            match agent
                .connect_mcp_server(name, command, args, Default::default())
                .await
            {
                Ok(count) => println!("Connected MCP server '{name}' with {count} tool(s)."),
                Err(e) => eprintln!("Failed to connect MCP server '{name}': {e}"),
            }
        }
        "disconnect" => {
            if parts.len() < 2 {
                println!("Usage: /mcp disconnect <name>");
                return Ok(());
            }
            let name = parts[1];
            match agent.disconnect_mcp_server(name).await {
                Ok(()) => println!(
                    "Disconnected MCP server '{name}'. (Tools remain until restart.)"
                ),
                Err(e) => eprintln!("Failed to disconnect '{name}': {e}"),
            }
        }
        "help" => {
            print_mcp_help();
        }
        other => {
            eprintln!("Unknown MCP subcommand: {other}");
            print_mcp_help();
        }
    }
    Ok(())
}

fn print_help(lang: Language) {
    if lang.is_zh() {
        println!("Pigs Agent — 可用命令（英文 / 中文 / 拼音）：");
        println!();
        println!("  /help, /h, /? , /帮助, /bangzhu              显示帮助");
        println!("  /lang, /language, /语言, /中文, /yuyan, /zhongwen");
        println!("                                              查看或设置语言 (en|zh)");
        println!("  /model, /模型, /moxing [add|添加|<id>]      查看/切换/引导添加模型");
        println!("  /mode, /模式, /权限, /moshi, /quanxian      设置权限模式");
        println!("  /tools, /工具, /gongju                      列出工具");
        println!("  /todo, /todos, /待办, /任务, /daiban, /renwu 显示待办");
        println!("  /status, /状态, /仪表盘, /zhuangtai, /yibiaopan  状态仪表盘");
        println!("  /info, /信息, /xinxi                        当前会话信息");
        println!("  /title, /标题, /biaoti                      设置会话标题");
        println!("  /cost, /费用, /开销, /成本, /feiyong, /kaixiao, /chengben  费用");
        println!("  /history, /历史, /lishi                     会话历史摘要");
        println!("  /mcp                                        管理 MCP 服务器");
        println!("      子命令: list/列表/liebiao, tools/工具/gongju,");
        println!("              connect/连接/lianjie, disconnect/断开/duankai");
        println!("  /skills, /技能, /jineng [reload]            技能目录（全文用 skill 工具）");
        println!("  /rules, /规则, /guize [reload]              项目规则");
        println!("  /memory, /记忆, /jiyi ...                   跨会话记忆");
        println!("  /export, /导出, /daochu [path]              导出会话为 markdown");
        println!("  /hooks, /钩子, /gouzi                       查看 hooks");
        println!("  /doctor, /诊断, /体检, /zhenduan, /tijian   环境健康检查");
        println!("  /models, /模型列表, /moxingliebiao          供应商/模型目录");
        println!("  /init, /初始化, /chushihua                  创建默认配置");
        println!("  /reload, /重载, /zhongzai, /chongzai        热重载配置");
        println!("  /compact, /压缩, /精简, /yasuo, /jingjian   手动压缩上下文");
        println!("  /clear, /清空, /清除, /qingkong, /qingchu   清空当前会话");
        println!("  /save, /保存, /baocun                       保存当前会话");
        println!("  /sessions, /会话, /huihua ...               管理会话");
        println!("      子命令: list/列表, open/打开/dakai, rm/删除/shanchu,");
        println!("              search/搜索/sousuo, current/当前/dangqian");
        println!("  /undo, /撤销, /回退, /chexiao, /huitui [list] 撤销最近写操作");
        println!("  /quit, /q, /exit, /退出, /tuichu, /likai    退出");
        println!();
        println!("中文与拼音别名在任何语言设置下都可用；默认语言为中文 (zh)。");
        println!("输入其它文字将作为提示发送给 Agent。");
    } else {
        println!("Pigs Agent — Available Commands (English / 中文 / pinyin):");
        println!();
        println!("  /help, /h, /?, /帮助, /bangzhu              Show this help");
        println!("  /lang, /language, /语言, /中文, /yuyan, /zhongwen");
        println!("                                              Show or set language (en|zh)");
        println!("  /model, /模型, /moxing [add|添加|<id>]      status / switch / guided add");
        println!("  /mode, /模式, /权限, /moshi, /quanxian      Permission mode");
        println!("  /tools, /工具, /gongju                      List tools");
        println!("  /todo, /todos, /待办, /daiban               Todo list");
        println!("  /status, /状态, /zhuangtai                  Dashboard");
        println!("  /info, /信息, /xinxi                        Session info");
        println!("  /title, /标题, /biaoti                      Set session title");
        println!("  /cost, /费用, /feiyong                      Token usage / cost");
        println!("  /history, /历史, /lishi                     History summary");
        println!("  /mcp                                        MCP servers");
        println!("      subs: list/列表/liebiao, tools/工具/gongju,");
        println!("            connect/连接/lianjie, disconnect/断开/duankai");
        println!("  /skills, /技能, /jineng [reload]            Skill catalog");
        println!("  /rules, /规则, /guize [reload]              Project rules");
        println!("  /memory, /记忆, /jiyi ...                   Memory notes");
        println!("  /export, /导出, /daochu [path]              Export session");
        println!("  /hooks, /钩子, /gouzi                       Tool hooks");
        println!("  /doctor, /诊断, /zhenduan, /tijian          Health checks");
        println!("  /models, /模型列表, /moxingliebiao          Model catalog");
        println!("  /init, /初始化, /chushihua                  Create config");
        println!("  /reload, /重载, /zhongzai, /chongzai        Reload config");
        println!("  /compact, /压缩, /yasuo, /jingjian          Compact context");
        println!("  /clear, /清空, /qingkong                    Clear session");
        println!("  /save, /保存, /baocun                       Save session");
        println!("  /sessions, /会话, /huihua ...               Manage sessions");
        println!("      subs: list/列表, open/打开/dakai, rm/删除/shanchu,");
        println!("            search/搜索/sousuo, current/当前/dangqian");
        println!("  /undo, /撤销, /chexiao, /huitui [list]      Undo last write");
        println!("  /quit, /q, /exit, /退出, /tuichu            Exit");
        println!();
        println!("Chinese / pinyin aliases work regardless of language setting.");
        println!("Default language is Chinese (zh); use /lang en to switch.");
        println!("Type any other text to send it to the agent as a prompt.");
    }
}
