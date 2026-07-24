//! Interactive REPL using rustyline.
//!
//! Aligned with PI's interactive mode conventions:
//! - Prompt: `pig> `
//! - `!command` prefix: run bash directly (like PI's bash mode)
//! - `/command` prefix: slash commands
//! - Banner: version, model, keybindings hint

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::Agent;
use crate::commands::{handle_command, CommandResult};
use crate::i18n;

/// Run the interactive REPL loop.
pub async fn run_repl(agent: &mut Agent) -> anyhow::Result<()> {
    let mut rl =
        DefaultEditor::new().map_err(|e| anyhow::anyhow!("Failed to init readline: {e}"))?;

    let lang = agent.language;
    let tools_label = if agent.no_tools {
        i18n::t(lang, "disabled")
    } else {
        i18n::t(lang, "enabled")
    };

    // Banner — PI-style: version + config summary + keybinding hint
    let version = env!("CARGO_PKG_VERSION");
    println!("\n  pig v{version}\n");
    println!(
        "  {} {} | {} {} | {} {} | {}",
        i18n::t(lang, "model"),
        agent.config.model,
        i18n::t(lang, "permission"),
        agent.permission_policy.active_mode,
        i18n::t(lang, "tools"),
        tools_label,
        agent.language.display_name(),
    );
    println!("  {} {}", i18n::t(lang, "session"), agent.session_id());
    println!("\n  {} /help  !bash  Ctrl+D {}", 
        i18n::t(lang, "shortcuts_label").trim_end_matches(':'),
        i18n::t(lang, "goodbye").trim_start_matches('\n'));
    println!();

    loop {
        let prompt = "pig> ".to_string();
        let readline = rl.readline(&prompt);

        match readline {
            Ok(line) => {
                let trimmed = line.trim();

                // Skip empty lines
                if trimmed.is_empty() {
                    continue;
                }

                // Add to history
                let _ = rl.add_history_entry(trimmed);

                // Bash mode: !command runs directly (PI-style)
                if let Some(bash_cmd) = trimmed.strip_prefix('!') {
                    // Handle !! prefix (bash without context — same behavior for now)
                    let bash_cmd = bash_cmd.strip_prefix('!').unwrap_or(bash_cmd);
                    if !bash_cmd.trim().is_empty() {
                        run_bash_direct(bash_cmd);
                    }
                    continue;
                }

                // Handle slash commands
                if trimmed.starts_with('/') {
                    match handle_command(agent, trimmed).await {
                        Ok(CommandResult::Continue) => continue,
                        Ok(CommandResult::Quit) => break,
                        Err(e) => {
                            eprintln!("Command error: {e}");
                            continue;
                        }
                    }
                }

                // Run the agent turn
                match agent.run_turn(trimmed).await {
                    Ok(_) => {
                        println!();
                    }
                    Err(e) => {
                        eprintln!("Agent error: {e}");
                        // Don't exit on errors — let the user try again
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("\n{}", i18n::t(agent.language, "ctrl_c"));
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("\n{}", i18n::t(agent.language, "goodbye"));
                break;
            }
            Err(e) => {
                eprintln!("Readline error: {e}");
                break;
            }
        }
    }

    // Final session save
    if let Err(e) = agent.session.save(&agent.sessions_dir) {
        eprintln!("Warning: failed to save session: {e}");
    }

    Ok(())
}

/// Run a bash command directly and print its output.
/// This is the `!command` mode — the output is shown to the user
/// but not added to the LLM conversation context.
fn run_bash_direct(cmd: &str) {
    use std::process::Command;
    let shell = if cfg!(target_os = "windows") { "cmd" } else { "sh" };
    let flag = if cfg!(target_os = "windows") { "/C" } else { "-c" };

    println!("$ {cmd}");
    let output = Command::new(shell)
        .args([flag, cmd])
        .output();

    match output {
        Ok(out) => {
            if !out.stdout.is_empty() {
                print!("{}", String::from_utf8_lossy(&out.stdout));
            }
            if !out.stderr.is_empty() {
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
            }
            let code = out.status.code().unwrap_or(-1);
            if code != 0 {
                eprintln!("[exit code: {code}]");
            }
        }
        Err(e) => {
            eprintln!("Failed to execute: {e}");
        }
    }
}
