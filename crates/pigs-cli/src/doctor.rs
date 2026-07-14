//! Doctor — environment and configuration health checks.

use std::path::PathBuf;
use std::process::Command;

use pigs_config::AppConfig;

use crate::agent::Agent;

#[derive(Debug)]
pub struct CheckItem {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub fn run_doctor(agent: &Agent) -> Vec<CheckItem> {
    let mut items = Vec::new();

    // Config files
    let global = AppConfig::config_path();
    items.push(CheckItem {
        name: "Global config".into(),
        ok: global.exists(),
        detail: if global.exists() {
            format!("found {}", global.display())
        } else {
            format!("missing {} (run /init)", global.display())
        },
    });

    let project = agent.workspace_root.join(".pigs").join("config.toml");
    items.push(CheckItem {
        name: "Project config".into(),
        ok: true, // optional
        detail: if project.exists() {
            format!("found {}", project.display())
        } else {
            format!("optional not present ({})", project.display())
        },
    });

    // Dirs
    for (label, path) in [
        ("Sessions dir", AppConfig::sessions_dir()),
        ("Logs dir", AppConfig::logs_dir()),
    ] {
        items.push(CheckItem {
            name: label.into(),
            ok: path.exists(),
            detail: path.display().to_string(),
        });
    }

    // Resolved model + provider credentials
    match agent.resolved_model() {
        Ok(resolved) => {
            items.push(CheckItem {
                name: format!(
                    "Model {} via {}",
                    resolved.remote_model, resolved.provider_name
                ),
                ok: true,
                detail: format!(
                    "api={}, context_window={}, base_url={}",
                    resolved.api.as_str(),
                    resolved.context_window,
                    resolved.base_url
                ),
            });
            items.push(CheckItem {
                name: format!("Provider credentials ({})", resolved.provider_name),
                ok: !resolved.api_key.is_empty(),
                detail: if resolved.api_key.is_empty() {
                    "missing api_key".into()
                } else {
                    "api_key set".into()
                },
            });
        }
        Err(e) => {
            items.push(CheckItem {
                name: "Model resolution".into(),
                ok: false,
                detail: e.to_string(),
            });
        }
    }

    // Tools
    items.push(CheckItem {
        name: "Tool registry".into(),
        ok: agent.no_tools || !agent.tool_registry.is_empty(),
        detail: if agent.no_tools {
            "tools disabled (--no-tools)".into()
        } else {
            format!("{} tools registered", agent.tool_registry.len())
        },
    });

    // Git availability
    let git_ok = Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    items.push(CheckItem {
        name: "git binary".into(),
        ok: git_ok,
        detail: if git_ok {
            "available".into()
        } else {
            "not found in PATH (git_diff tool limited)".into()
        },
    });

    // Workspace write probe
    let probe = agent.workspace_root.join(".pigs");
    let writable = if probe.exists() {
        true
    } else {
        std::fs::create_dir_all(&probe).is_ok()
    };
    items.push(CheckItem {
        name: "Workspace writable".into(),
        ok: writable,
        detail: agent.workspace_root.display().to_string(),
    });

    // pigsignore
    let ignore = agent.workspace_root.join(".pigsignore");
    items.push(CheckItem {
        name: ".pigsignore".into(),
        ok: true,
        detail: if ignore.exists() {
            format!("found {}", ignore.display())
        } else {
            "not present (optional)".into()
        },
    });

    // Skills
    items.push(CheckItem {
        name: "Skills".into(),
        ok: true,
        detail: format!("{} loaded", agent.skills.len()),
    });

    items
}

pub fn print_doctor_report(items: &[CheckItem]) {
    let mut ok_n = 0usize;
    let mut warn_n = 0usize;
    println!("Pigs doctor report:");
    println!("{}", "-".repeat(60));
    for item in items {
        let mark = if item.ok { "OK " } else { "ERR" };
        if item.ok {
            ok_n += 1;
        } else {
            warn_n += 1;
        }
        println!("[{mark}] {:<28} {}", item.name, item.detail);
    }
    println!("{}", "-".repeat(60));
    println!("Summary: {ok_n} ok, {warn_n} need attention");
}

#[allow(dead_code)]
pub fn config_paths() -> (PathBuf, PathBuf) {
    (AppConfig::config_path(), PathBuf::from(".pigs/config.toml"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::agent::Agent;
    use crate::cli::CliArgs;

    fn openai_compatible_smoke_args() -> CliArgs {
        CliArgs {
            prompt: None,
            model: Some("gpt-4o-mini".into()),
            mode: Some("readonly".into()),
            language: Some("en".into()),
            system_prompt: None,
            resume: None,
            no_tools: false,
            max_turns: 5,
            list_sessions: false,
            output: "text".into(),
        }
    }

    #[test]
    fn doctor_runs_with_dummy_openai_key() {
        // No real network call — only construct Agent + doctor checks.
        let mut config = AppConfig::default();
        config.openai.api_key = Some("sk-test-dummy".into());
        config.openai.base_url = Some("http://127.0.0.1:9/v1".into());
        let agent = Agent::new(config, openai_compatible_smoke_args())
            .expect("agent should start with openai-format model + dummy key");
        let items = run_doctor(&agent);
        assert!(
            !items.is_empty(),
            "doctor should report at least one check item"
        );

        let tool_check = items
            .iter()
            .find(|i| i.name == "Tool registry")
            .expect("tool registry check present");
        assert!(
            tool_check.ok,
            "tools should be registered: {}",
            tool_check.detail
        );
        assert!(
            tool_check.detail.contains("tools registered"),
            "detail={}",
            tool_check.detail
        );

        // Report formatting must not panic.
        print_doctor_report(&items);
    }

    #[test]
    fn list_sessions_api_without_agent() {
        // Same path used by --list-sessions (no Agent / no API key).
        let result = Agent::list_sessions();
        assert!(result.is_ok(), "list_sessions error: {result:?}");
    }
}
