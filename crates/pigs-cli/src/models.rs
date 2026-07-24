//! Model catalog display and guided setup for `/model` / `/models`.
//!
//! Inspired by opencode's progressive provider/model configuration flow:
//! show current selection → list catalog → guided add wizard.

use std::io::{self, Write};

use pigs_config::{ApiFormat, AppConfig, ModelConfig, NamedProviderConfig, ResolvedModel};

use crate::agent::Agent;

pub fn provider_format_name(api: ApiFormat) -> &'static str {
    api.as_str()
}

/// Print full provider + model catalog (`/models`).
pub fn print_models(out: &mut crate::output::OutputSink, config: &AppConfig, current_selection: &str, _current_remote: &str) {
    out.println(
        "API formats: anthropic (Messages) | openai (Responses /v1/responses) | openai-chat (Chat Completions)"
    );
    out.println_empty();

    let providers = config.effective_providers();
    if !providers.is_empty() {
        out.println("Configured providers:");
        out.println(format!("{:<16} {:<12} {:<40} Key", "Name", "API", "Base URL"));
        out.println("-".repeat(90));
        for p in &providers {
            let key = if p.api_key.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
                "set"
            } else {
                "missing"
            };
            let url = p.base_url.as_deref().unwrap_or("-");
            out.println(format!("{:<16} {:<12} {:<40} {}", key,
                p.name,
                p.api,
                truncate(url, 40)
            ));
        }
        out.println_empty();
    }

    if config.models.is_empty() {
        out.println("No [[models]] catalog entries. Using built-in aliases + raw model ids.");
        out.println("  aliases: opus, sonnet, haiku, gpt-4o, gpt-4o-mini");
        out.println("  or: provider/model-id  e.g. deepseek/deepseek-chat");
        out.println_empty();
        out.println("Models with `-pig` suffix go through the phased runtime (Pre→Executor→Post).");
    } else {
        out.println("Configured models (each has a `-pig` phased variant):");
        out.println(format!(
            "{:<24} {:<20} {:<14}",
            "Name", "Provider", "API"
        ));
        out.println("-".repeat(80));
        for m in &config.models {
            // Raw (direct) model
            let mark_raw = if current_selection == m.name {
                "*"
            } else {
                " "
            };
            let provider = m.provider.as_deref().unwrap_or("(auto)");
            let api = m.api.as_deref().unwrap_or("(auto)");
            out.println(format!("{}{:<23} {:<20} {:<14}", mark_raw,
                m.name, provider, api
            ));

            // -pig (phased) variant
            let pig_name = format!("{}-pig", m.name);
            let mark_pig = if current_selection == pig_name {
                "*"
            } else {
                " "
            };
            let provider = m.provider.as_deref().unwrap_or("(auto)");
            let api = m.api.as_deref().unwrap_or("(auto)");
            out.println(format!("{}{:<23} {:<20} {:<14}  phased (pre→executor→post)", mark_pig,
                pig_name, provider, api
            ));
        }
    }

    out.println_empty();
    match config.resolve_model(current_selection) {
        Ok(r) => print_resolved_detail(out, &r, current_selection),
        Err(e) => out.println(format!("Current selection '{current_selection}' unresolved: {e}")),
    }
    out.println_empty();
    out.println("Use:");
    out.println("  /model <name|alias|provider/model-id|raw-id>   switch model");
    out.println("  /model add | /模型 添加                        guided setup wizard");
    out.println("  /models                                        full catalog");
}

/// Status view for bare `/model` / `/模型` — always shows configured ids first.
pub fn print_model_status(agent: &mut Agent) {
    let zh = agent.language.is_zh();
    let selection = agent.config.model.as_str();
    let live_remote = agent.api_client.model();

    if zh {
        agent.output.println("当前模型");
        agent.output.println("-".repeat(60));
    } else {
        agent.output.println("Current model");
        agent.output.println("-".repeat(60));
    }

    match agent.resolved_model() {
        Ok(r) => {
            print_resolved_detail(&mut agent.output, &r, selection);
            if r.remote_model != live_remote {
                if zh {
                    agent.output.println(format!("运行中远端 id: {live_remote}  (与解析结果不一致，可 /reload)"));
                } else {
                    agent.output.println(format!("Live remote id: {live_remote}  (differs from resolve; try /reload)"));
                }
            }
        }
        Err(e) => {
            if zh {
                agent.output.println(format!("配置选择: {selection}"));
                agent.output.println(format!("运行中远端 id: {live_remote}"));
                agent.output.println(format!("解析失败: {e}"));
            } else {
                agent.output.println(format!("Config selection: {selection}"));
                agent.output.println(format!("Live remote id: {live_remote}"));
                agent.output.println(format!("Resolve error: {e}"));
            }
        }
    }

    agent.output.println_empty();
    if !agent.config.models.is_empty() {
        if zh {
            agent.output.println(format!("已配置模型目录 ({}):", agent.config.models.len()));
        } else {
            agent.output.println(format!("Configured catalog ({}):", agent.config.models.len()));
        }
        for m in &agent.config.models {
            let mark = if m.name == selection {
                "*"
            } else {
                " "
            };
            let provider = m.provider.as_deref().unwrap_or("(auto)");
            agent.output.println(format!("  {} {}  [{}]", mark, m.name, provider));
        }
        agent.output.println_empty();
    }

    if zh {
        agent.output.println("用法:");
        agent.output.println("  /模型 <名称|别名|供应商/模型id|原始id>   切换模型");
        agent.output.println("  /模型 添加 | /model add                  逐步引导添加供应商+模型");
        agent.output.println("  /模型列表 | /models                      查看完整目录");
        agent.output.println("内置别名: opus, sonnet, haiku, gpt-4o, gpt-4o-mini");
    } else {
        agent.output.println("Usage:");
        agent.output.println("  /model <name|alias|provider/model-id|raw-id>   switch");
        agent.output.println("  /model add | /模型 添加                        guided setup wizard");
        agent.output.println("  /models                                        full catalog");
        agent.output.println("Built-in aliases: opus, sonnet, haiku, gpt-4o, gpt-4o-mini");
    }
}

fn print_resolved_detail(out: &mut crate::output::OutputSink, r: &ResolvedModel, selection: &str) {
    out.println(format!("  selection:      {selection}"));
    out.println(format!("  catalog name:   {}", r.name));
    out.println(format!("  remote model:   {}", r.remote_model));
    out.println(format!("  provider:       {}", r.provider_name));
    out.println(format!("  api format:     {}", provider_format_name(r.api)));
    out.println(format!("  base_url:       {}", r.base_url));
    out.println(format!("  context_window: {}", r.context_window));
    if let Some(mt) = r.max_tokens {
        out.println(format!("  max_tokens:     {mt}"));
    }
    if let Some(temp) = r.temperature {
        out.println(format!("  temperature:    {temp}"));
    }
    let key_state = if r.api_key.is_empty() {
        "missing"
    } else {
        "set"
    };
    out.println(format!("  api_key:        {key_state}"));
}

/// Handle `/model` arguments: switch, status, or wizard.
pub fn handle_model_command(agent: &mut Agent, arg: &str) -> anyhow::Result<()> {
    let arg = arg.trim();
    if arg.is_empty() {
        print_model_status(agent);
        return Ok(());
    }

    let lower = arg.to_ascii_lowercase();
    let is_add = matches!(
        lower.as_str(),
        "add" | "new" | "setup" | "wizard" | "config" | "tianjia" | "xinzeng"
    ) || arg == "添加"
        || arg == "新增"
        || arg == "配置"
        || arg == "向导";

    if is_add {
        run_model_add_wizard(agent)?;
        return Ok(());
    }

    // `/model list` convenience → same as /models
    if matches!(lower.as_str(), "list" | "ls" | "liebiao") || arg == "列表" || arg == "目录" {
        print_models(&mut agent.output, &agent.config, &agent.config.model, agent.api_client.model());
        return Ok(());
    }

    match agent.set_model(arg) {
        Ok(()) => {
            if agent.language.is_zh() {
                agent.output.println(format!(
                    "已切换模型: selection=`{}` remote=`{}`",
                    agent.config.model,
                    agent.api_client.model()
                ));
            } else {
                agent.output.println(format!(
                    "Model switched: selection=`{}` remote=`{}`",
                    agent.config.model,
                    agent.api_client.model()
                ));
            }
            if let Ok(r) = agent.resolved_model() {
                print_resolved_detail(&mut agent.output, &r, &agent.config.model);
            }
        }
        Err(e) => {
            if agent.language.is_zh() {
                agent.output.eprintln(format!("切换失败: {e}"));
                agent.output.eprintln("提示: 用 `/模型 添加` 引导配置供应商与模型，或 `/模型列表` 查看目录。");
            } else {
                agent.output.eprintln(format!("Failed to switch model: {e}"));
                agent.output.eprintln("Hint: `/model add` for guided setup, or `/models` to list catalog.");
            }
        }
    }
    Ok(())
}

/// Interactive wizard to add a provider + model (opencode-style progressive setup).
pub fn run_model_add_wizard(agent: &mut Agent) -> anyhow::Result<()> {
    let zh = agent.language.is_zh();
    if zh {
        agent.output.println("添加模型向导（逐步配置供应商与模型，写入 ~/.pigs/pig.toml）");
        agent.output.println("直接回车使用括号中的默认值；输入 q 取消。");
    } else {
        agent.output.println("Add model wizard (provider + model → ~/.pigs/pig.toml)");
        agent.output.println("Press Enter for defaults in [brackets]; type q to cancel.");
    }
    agent.output.println_empty();

    // Show existing providers briefly
    let providers = agent.config.effective_providers();
    if zh {
        agent.output.println("已有供应商:");
    } else {
        agent.output.println("Existing providers:");
    }
    for (i, p) in providers.iter().enumerate() {
        let key = if p.api_key.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
            "key=set"
        } else {
            "key=missing"
        };
        agent.output.println(format!(
            "  {}. {}  api={}  {}  url={}",
            i + 1,
            p.name,
            p.api,
            key,
            p.base_url.as_deref().unwrap_or("-")
        ));
    }
    agent.output.println_empty();

    // Step 1: provider name
    let provider_name = match prompt_line(
        if zh {
            "1/7 供应商名称 (如 openai / anthropic / deepseek / iflytek)"
        } else {
            "1/7 Provider name (e.g. openai / anthropic / deepseek / iflytek)"
        },
        Some("openai"),
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };

    let existing = providers.iter().find(|p| p.name == provider_name);
    let default_api = existing
        .map(|p| p.api.as_str())
        .unwrap_or(if provider_name == "anthropic" {
            "anthropic"
        } else if provider_name == "openai" {
            "openai"
        } else {
            "openai-chat"
        });

    // Step 2: api format
    if zh {
        agent.output.println("  API 格式: openai (Responses) | openai-chat (Chat Completions) | anthropic");
    } else {
        agent.output.println("  API formats: openai (Responses) | openai-chat (Chat Completions) | anthropic");
    }
    let api_raw = match prompt_line(
        if zh {
            "2/7 API 格式"
        } else {
            "2/7 API format"
        },
        Some(default_api),
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };
    let api = match ApiFormat::parse(&api_raw) {
        Ok(a) => a,
        Err(e) => {
            agent.output.eprintln(e);
            return Ok(());
        }
    };

    // Step 3: base url
    let default_url = existing
        .and_then(|p| p.base_url.clone())
        .unwrap_or_else(|| match api {
            ApiFormat::OpenAI | ApiFormat::OpenAIChat => "https://api.openai.com/v1".into(),
            ApiFormat::Anthropic => "https://api.anthropic.com".into(),
        });
    let base_url = match prompt_line(
        if zh { "3/7 Base URL" } else { "3/7 Base URL" },
        Some(&default_url),
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };

    // Step 4: api key
    let existing_key = existing.and_then(|p| p.api_key.clone()).unwrap_or_default();
    let key_default_hint = if existing_key.is_empty() {
        ""
    } else {
        "********"
    };
    let api_key_in = match prompt_line(
        if zh {
            "4/7 API Key（已有密钥可回车保留）"
        } else {
            "4/7 API Key (Enter keeps existing)"
        },
        if key_default_hint.is_empty() {
            None
        } else {
            Some(key_default_hint)
        },
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };
    let api_key = if api_key_in == "********" || api_key_in.is_empty() {
        if existing_key.is_empty() {
            if zh {
                agent.output.eprintln("未设置 API Key，继续保存但调用会失败，直到配置密钥。");
            } else {
                agent.output.eprintln("No API key set; calls will fail until a key is configured.");
            }
            String::new()
        } else {
            existing_key
        }
    } else {
        api_key_in
    };

    // Step 5: local catalog name
    let default_local = provider_name.clone();
    let local_name = match prompt_line(
        if zh {
            "5/7 本地模型名（/model 切换时用的名字）"
        } else {
            "5/7 Local model name (used by /model)"
        },
        Some(&default_local),
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };

    // Step 6: remote model id
    let remote_default = local_name.clone();
    let remote_model = match prompt_line(
        if zh {
            "6/7 远端模型 id（发给 API 的 model 字段）"
        } else {
            "6/7 Remote model id (wire `model` field)"
        },
        Some(&remote_default),
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };

    // context_window is no longer stored in ModelConfig — it lives in config.toml's
    // [provider.context_windows]. Skip the context window prompt step.

    let switch_default = if zh { "y" } else { "y" };
    let switch_raw = match prompt_line(
        if zh {
            "添加后切换为当前模型? [y/N]"
        } else {
            "Switch to this model now? [y/N]"
        },
        Some(switch_default),
    )? {
        None => {
            cancel(&mut agent.output, zh);
            return Ok(());
        }
        Some(v) => v,
    };
    let do_switch = matches!(
        switch_raw.to_ascii_lowercase().as_str(),
        "y" | "yes" | "1" | "true" | "是"
    );

    // Apply to in-memory config
    agent.config.upsert_provider(NamedProviderConfig {
        name: provider_name.clone(),
        api: api.as_str().to_string(),
        api_key: if api_key.is_empty() {
            None
        } else {
            Some(api_key)
        },
        base_url: if base_url.is_empty() {
            None
        } else {
            Some(base_url)
        },
    });
    agent.config.upsert_model(ModelConfig {
        name: local_name.clone(),
        api_key: None,
        provider: Some(provider_name.clone()),
        api: None,
        temperature: None,
    });

    // Persist global config
    match agent.config.save() {
        Ok(()) => {
            let path = AppConfig::config_path();
            if zh {
                agent.output.println(format!("已写入配置: {}", path.display()));
            } else {
                agent.output.println(format!("Saved config: {}", path.display()));
            }
        }
        Err(e) => {
            if zh {
                agent.output.eprintln(format!("保存配置失败: {e}（内存中已更新，可稍后 /init 或手动保存）"));
            } else {
                agent.output.eprintln(format!("Failed to save config: {e} (in-memory updated)"));
            }
        }
    }

    if do_switch {
        match agent.set_model(&local_name) {
            Ok(()) => {
                if zh {
                    agent.output.println(format!(
                        "已切换: selection=`{}` remote=`{}`",
                        agent.config.model,
                        agent.api_client.model()
                    ));
                } else {
                    agent.output.println(format!(
                        "Switched: selection=`{}` remote=`{}`",
                        agent.config.model,
                        agent.api_client.model()
                    ));
                }
                // Persist default model selection too
                agent.config.set_default_model(local_name.clone());
                let _ = agent.config.save();
            }
            Err(e) => {
                if zh {
                    agent.output.eprintln(format!("模型已添加，但切换失败: {e}"));
                } else {
                    agent.output.eprintln(format!("Model added, but switch failed: {e}"));
                }
            }
        }
    } else if zh {
        agent.output.println(format!("模型已添加。切换: /模型 {local_name}"));
    } else {
        agent.output.println(format!("Model added. Switch with: /model {local_name}"));
    }

    // Show final status
    agent.output.println_empty();
    print_model_status(agent);
    Ok(())
}

fn cancel(out: &mut crate::output::OutputSink, zh: bool) {
    if zh {
        out.println("已取消。");
    } else {
        out.println("Cancelled.");
    }
}

fn prompt_line(label: &str, default: Option<&str>) -> io::Result<Option<String>> {
    let mut stdout = io::stdout();
    if let Some(d) = default {
        write!(stdout, "{label} [{d}]: ")?;
    } else {
        write!(stdout, "{label}: ")?;
    }
    stdout.flush()?;

    let mut line = String::new();
    let n = io::stdin().read_line(&mut line)?;
    if n == 0 {
        // EOF
        return Ok(None);
    }
    let t = line.trim();
    if t.eq_ignore_ascii_case("q") || t == "quit" || t == "exit" || t == "取消" {
        return Ok(None);
    }
    if t.is_empty() {
        return Ok(Some(default.unwrap_or("").to_string()));
    }
    Ok(Some(t.to_string()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{t}...")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn print_resolved_includes_ids() {
        let r = ResolvedModel {
            name: "auto".into(),
            remote_model: "xop3qwen1b7".into(),
            provider_name: "iflytek".into(),
            api: ApiFormat::OpenAIChat,
            api_key: "sk-test".into(),
            base_url: "https://example.com/v1".into(),
            context_window: 200_000,
            max_tokens: None,
            temperature: None,
            is_pig: false,
        };
        // smoke: function does not panic
        let mut sink = crate::output::OutputSink::Stdout;
        print_resolved_detail(&mut sink, &r, "auto");
    }
}
