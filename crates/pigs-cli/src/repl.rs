//! Interactive REPL using rustyline.

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::Agent;
use crate::commands::{handle_command, CommandResult};
use crate::i18n;

/// Run the interactive REPL loop.
pub async fn run_repl(agent: &mut Agent) -> anyhow::Result<()> {
    let mut rl = DefaultEditor::new().map_err(|e| anyhow::anyhow!("Failed to init readline: {e}"))?;

    let lang = agent.language;
    let tools_label = if agent.no_tools {
        i18n::t(lang, "disabled")
    } else {
        i18n::t(lang, "enabled")
    };

    println!("Pigs Agent v{}", env!("CARGO_PKG_VERSION"));
    println!(
        "{}: {} ({}) | {}: {} | {}: {} | {}: {} ({})",
        i18n::t(lang, "model"),
        agent.config.model,
        agent.api_client.model(),
        i18n::t(lang, "permission"),
        agent.permission_policy.active_mode,
        i18n::t(lang, "tools"),
        tools_label,
        i18n::t(lang, "language"),
        agent.language.as_str(),
        agent.language.display_name(),
    );
    println!("{}: {}", i18n::t(lang, "session"), agent.session_id());
    println!();
    println!("{}", i18n::t(lang, "banner_help"));
    println!();

    loop {
        let prompt = "pigs> ".to_string();
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
