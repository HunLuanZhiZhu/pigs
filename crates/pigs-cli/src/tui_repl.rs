//! TUI-based REPL — full-screen ratatui terminal UI aligned with PI.
//!
//! Architecture:
//! - App runs its own draw loop (terminal input + redraw)
//! - User submissions trigger agent.run_turn_with_callback()
//! - Streaming text deltas and tool events flow to App via EventBroker
//! - Slash commands and bash mode handled inline

use tokio::sync::mpsc;

use pigs_tui::event::{AppEvent, EventBroker};
use pigs_tui::App;

use crate::agent::Agent;
use crate::commands::{handle_command, CommandResult};

/// Run the TUI-based interactive REPL.
pub async fn run_tui_repl(
    agent: &mut Agent,
    log_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
) -> anyhow::Result<()> {
    let cwd = agent.workspace_root.display().to_string();
    let model = agent.config.model.clone();
    let language = agent.language.as_str().to_string();

    let mut app = App::init(&model, &language, &cwd)?;

    // Populate model list for the model selector overlay (Ctrl+L)
    let models: Vec<String> = agent.config.models.iter().map(|m| m.name.clone()).collect();
    if !models.is_empty() {
        app.set_models(models);
    }

    let mut broker = EventBroker::new();

    // Proxy log receiver (None in --api-only mode or fallback REPL)
    let mut log_rx = log_rx;

    let mut terminal_event_interval = tokio::time::interval(std::time::Duration::from_millis(50));
    terminal_event_interval.tick().await;

    loop {
        tokio::select! {
            // Drain proxy log lines into the App buffer
            Some(line) = async {
                match &mut log_rx {
                    Some(rx) => rx.recv().await,
                    None => None,
                }
            } => {
                app.push_proxy_log(line);
            }
            _ = terminal_event_interval.tick() => {
                while pigs_tui::crossterm::event::poll(std::time::Duration::from_millis(0))? {
                    let event = pigs_tui::crossterm::event::read()?;
                    app.handle_terminal_event_public(event);

                    if let Some(text) = app.take_pending_input() {
                        if text.trim().is_empty() {
                            continue;
                        }

                        if text.starts_with('/') {
                            // Check for navigation commands to sync TUI focus
                            let is_nav_cmd = {
                                let lower = text.to_ascii_lowercase();
                                let cmd_part = lower.split_whitespace().next().unwrap_or("");
                                matches!(cmd_part,
                                    "/sub" | "/ses" | "/sessions" | "/back" | "/next"
                                    | "/ziagent" | "/zizhuti" | "/huihua"
                                    | "/会话" | "/会话列表" | "/返回" | "/后退" | "/前进"
                                    | "/fanhui" | "/houtui" | "/qianjin"
                                )
                            };

                            // Switch to buffer mode so command output is captured
                            // instead of being written raw to stdout (which would
                            // corrupt the ratatui alternate-screen differential renderer).
                            agent.set_output_buffer_mode();
                            let result = handle_command(agent, &text).await;
                            let captured = agent.take_output_buffer();

                            // Push captured output to TUI chat as a System entry
                            // (only if non-empty). This routes the output through
                            // ratatui's normal rendering path instead of raw stdout.
                            let trimmed = captured.trim_end();
                            if !trimmed.is_empty() {
                                app.push_to_main_chat(
                                    pigs_tui::chat::ChatEntry::System(trimmed.to_string()),
                                );
                            }

                            match result {
                                Ok(CommandResult::Continue) => {
                                    // After navigation command, sync TUI focus
                                    if is_nav_cmd {
                                        // Check if navigating to "api" (proxy log view)
                                        let lower = text.to_ascii_lowercase();
                                        if lower.contains("api") || lower.contains("api") {
                                            // /ses api → view proxy log
                                            // Check if the command actually targets "api"
                                            let parts: Vec<&str> = text.split_whitespace().collect();
                                            if parts.len() >= 2 && (parts[1] == "api" || parts[1].ends_with("api")) {
                                                app.view_proxy();
                                            } else {
                                                app.view_chat();
                                                let focus = agent.sub_agent_manager
                                                    .lock().unwrap_or_else(|e| e.into_inner())
                                                    .current_focus().to_string();
                                                app.switch_chat_focus(&focus);
                                            }
                                        } else {
                                            app.view_chat();
                                            let focus = agent.sub_agent_manager
                                                .lock().unwrap_or_else(|e| e.into_inner())
                                                .current_focus().to_string();
                                            app.switch_chat_focus(&focus);
                                        }
                                    }
                                }
                                Ok(CommandResult::Quit) => {
                                    app.cleanup_public()?;
                                    let _ = agent.session.save(&agent.sessions_dir);
                                    agent.sub_agent_manager
                                        .lock().unwrap_or_else(|e| e.into_inner())
                                        .save_all(&agent.sessions_dir);
                                    return Ok(());
                                }
                                Err(e) => {
                                    let _ = broker.sender().send(AppEvent::AgentError(e.to_string()));
                                }
                            }
                        } else if text.starts_with('!') {
                            let cmd = text.trim_start_matches('!');
                            run_bash_and_inject(cmd, &broker.sender());
                        } else {
                            // Agent turn with streaming callback.
                            //
                            // The `run_turn_with_callback` future borrows
                            // `&mut agent` and drives the LLM stream; its
                            // callback pushes AppEvent::StreamText into the
                            // broker channel. Previously this future was
                            // awaited inline, which starved the outer
                            // `tokio::select!` loop — the broker branch could
                            // not fire while the turn was in flight, so all
                            // streaming deltas piled up in the channel and
                            // rendered as one bulk update when the turn
                            // finished. That is why the TUI output appeared
                            // non-streaming.
                            //
                            // Fix: pin the turn future locally and poll it
                            // inside a `select!` that also drains broker
                            // events. `agent` is borrowed by the pinned
                            // future for the duration of the turn; we touch
                            // `agent` again only after the future resolves,
                            // which is sound.
                            app.set_working(true);
                            let _ = app.draw_public();

                            let tx = broker.sender();
                            // Box::pin (instead of tokio::pin!) so we can
                            // explicitly `drop` the future before touching
                            // `agent` again — the borrow checker can't see
                            // through tokio::pin!'s drop order.
                            let mut turn = Box::pin(agent.run_turn_with_callback(&text, move |progress| {
                                use crate::phased_runtime::TurnProgress;
                                match progress {
                                    TurnProgress::TextDelta { text, .. } => {
                                        let _ = tx.send(AppEvent::StreamText(text));
                                    }
                                    TurnProgress::ToolStart { name, .. } => {
                                        let _ = tx.send(AppEvent::ToolStart { name, args: String::new() });
                                    }
                                    TurnProgress::ToolEnd { name, is_error, .. } => {
                                        let _ = tx.send(AppEvent::ToolEnd {
                                            name,
                                            result: String::new(),
                                            is_error,
                                        });
                                    }
                                    _ => {}
                                }
                            }));

                            let result = loop {
                                tokio::select! {
                                    biased;
                                    Some(app_event) = broker.next() => {
                                        app.handle_app_event_public(app_event);
                                        let _ = app.draw_public();
                                        if app.should_quit_public() {
                                            app.cleanup_public()?;
                                            // Drop the future first so the
                                            // mutable borrow of `agent` ends.
                                            drop(turn);
                                            let _ = agent.session.save(&agent.sessions_dir);
                                            agent.sub_agent_manager
                                                .lock().unwrap_or_else(|e| e.into_inner())
                                                .save_all(&agent.sessions_dir);
                                            return Ok(());
                                        }
                                    }
                                    res = &mut turn.as_mut() => {
                                        // Turn future resolved; the borrow of
                                        // `agent` is released here so we can
                                        // safely use it again below.
                                        break res;
                                    }
                                }
                            };

                            // Drop the future so `agent` is borrowable.
                            // The future already resolved above, but the Box
                            // is still alive; release it now.
                            drop(turn);

                            // Drain any broker events that were sent before
                            // the turn resolved (e.g. final TextDelta frames).
                            while let Ok(Some(app_event)) = broker.try_next() {
                                app.handle_app_event_public(app_event);
                            }
                            let _ = app.draw_public();

                            match result {
                                Ok(_) => {
                                    let _ = broker.sender().send(AppEvent::TurnFinished);

                                    // Sync sub-agent status to TUI (Team Mode)
                                    let (total, done) = {
                                        let mgr = agent.sub_agent_manager.lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        let total = mgr.len();
                                        let done = mgr.agents.values()
                                            .filter(|s| s.status.is_terminal())
                                            .count();
                                        (total, done)
                                    };
                                    app.update_sub_agent_status(total, done);

                                    // Push sub-agent completion notifications to main chat
                                    let mgr = agent.sub_agent_manager.lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    for (id, sub) in &mgr.agents {
                                        if let crate::sub_agent::SubAgentStatus::Done = sub.status {
                                            if let Some(result) = &sub.result {
                                            app.push_to_main_chat(
                                                pigs_tui::chat::ChatEntry::SubAgentDone {
                                                    id: id.clone(),
                                                    success: true,
                                                    result: result.clone(),
                                                }
                                            );
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = broker.sender().send(AppEvent::AgentError(e.to_string()));
                                }
                            }
                            app.set_working(false);
                        }
                    }

                    // Check for model change from overlay
                    if let Some(new_model) = app.take_pending_model() {
                        agent.config.model = new_model;
                    }

                    if app.should_quit_public() {
                        app.cleanup_public()?;
                        let _ = agent.session.save(&agent.sessions_dir);
                        agent.sub_agent_manager
                            .lock().unwrap_or_else(|e| e.into_inner())
                            .save_all(&agent.sessions_dir);
                        return Ok(());
                    }
                    let _ = app.draw_public();
                }

                if app.is_working_public() {
                    app.tick_spinner();
                    let _ = app.draw_public();
                }
            }
            Some(app_event) = broker.next() => {
                app.handle_app_event_public(app_event);
                if app.should_quit_public() {
                    app.cleanup_public()?;
                    let _ = agent.session.save(&agent.sessions_dir);
                    agent.sub_agent_manager
                        .lock().unwrap_or_else(|e| e.into_inner())
                        .save_all(&agent.sessions_dir);
                    return Ok(());
                }
                let _ = app.draw_public();
            }
        }
    }
}

fn run_bash_and_inject(cmd: &str, tx: &mpsc::UnboundedSender<AppEvent>) {
    use std::process::Command;
    let shell = if cfg!(target_os = "windows") { "cmd" } else { "sh" };
    let flag = if cfg!(target_os = "windows") { "/C" } else { "-c" };

    let output = Command::new(shell).args([flag, cmd]).output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let combined = if stderr.is_empty() { stdout } else { format!("{stdout}\n{stderr}") };
            let code = out.status.code().unwrap_or(-1);
            let _ = tx.send(AppEvent::AgentError(format!("$ {cmd}\n{combined}\n[exit: {code}]")));
        }
        Err(e) => {
            let _ = tx.send(AppEvent::AgentError(format!("Failed to execute: {e}")));
        }
    }
}
