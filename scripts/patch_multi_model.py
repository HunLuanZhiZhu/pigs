#!/usr/bin/env python3
"""Patch pigs config/llm/cli for multi-provider multi-model + context_window."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def patch_config() -> None:
    path = ROOT / "crates/pigs-config/src/config.rs"
    text = path.read_text(encoding="utf-8")

    types = '''
/// Wire API format for a named provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiFormat {
    /// OpenAI Chat Completions (`/v1/chat/completions`).
    #[default]
    OpenAI,
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
}

impl ApiFormat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "chat" | "completions" => Ok(Self::OpenAI),
            "anthropic" | "claude" | "messages" => Ok(Self::Anthropic),
            other => Err(format!(
                "unknown api format '{other}' (expected openai or anthropic)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

/// A named provider endpoint (credentials + API format).
///
/// Multiple providers may share the same API format (e.g. openai.com, DeepSeek,
/// Ollama all use OpenAI Chat Completions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedProviderConfig {
    /// Local name used by model entries (e.g. "openai", "deepseek", "ollama").
    pub name: String,
    /// Wire format: "openai" or "anthropic".
    #[serde(default = "default_api_openai")]
    pub api: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

fn default_api_openai() -> String {
    "openai".to_string()
}

/// A selectable model entry in the local catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Local name / alias used by `/model` and `model = "..."`.
    pub name: String,
    /// Provider name from `[[providers]]` (or built-in "openai" / "anthropic").
    pub provider: String,
    /// Remote model id sent to the API. Defaults to `name` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Context window size in tokens (used for auto-compaction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Optional per-model max output tokens override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Optional per-model temperature override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Optional notes shown by `/models`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Fully resolved model + provider credentials for client creation.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// Local selection name (alias or raw id).
    pub name: String,
    /// Model id sent over the wire.
    pub remote_model: String,
    /// Provider catalog name.
    pub provider_name: String,
    pub api: ApiFormat,
    pub api_key: String,
    pub base_url: String,
    /// Effective context window in tokens.
    pub context_window: u64,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

impl ResolvedModel {
    /// Auto-compact threshold derived from context window and global config.
    ///
    /// Uses ~75% of the context window, capped by `compact_token_threshold`.
    pub fn compact_threshold(&self, compact_token_threshold: u64) -> u64 {
        let from_window = self.context_window.saturating_mul(3) / 4;
        from_window.min(compact_token_threshold).max(1_024)
    }
}

'''

    if "pub struct NamedProviderConfig" not in text:
        needle = "/// Configuration for an MCP server process.\n"
        if needle not in text:
            raise SystemExit("MCP comment not found")
        text = text.replace(needle, types + needle, 1)

    if "pub providers:" not in text:
        text = text.replace(
            """    /// Tool lifecycle hooks.
    #[serde(default)]
    pub hooks: HooksConfig,
}
""",
            """    /// Named provider endpoints (multi-vendor). Prefer this over legacy openai/anthropic blocks.
    #[serde(default)]
    pub providers: Vec<NamedProviderConfig>,
    /// Local model catalog with optional context_window and provider binding.
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// Tool lifecycle hooks.
    #[serde(default)]
    pub hooks: HooksConfig,
}
""",
        )

    if "providers: Vec::new()" not in text:
        text = text.replace(
            """            mcp_servers: Vec::new(),
            hooks: HooksConfig::default(),
""",
            """            mcp_servers: Vec::new(),
            providers: Vec::new(),
            models: Vec::new(),
            hooks: HooksConfig::default(),
""",
        )

    if "self.providers.extend" not in text and "for p in project.providers" not in text:
        text = text.replace(
            """        // Append project MCP servers and hooks
        self.mcp_servers.extend(project.mcp_servers);
        self.hooks
            .pre_tool_use
            .extend(project.hooks.pre_tool_use);
        self.hooks
            .post_tool_use
            .extend(project.hooks.post_tool_use);
    }
""",
            """        // Append project MCP servers, providers, models, hooks
        self.mcp_servers.extend(project.mcp_servers);
        for p in project.providers {
            if let Some(existing) = self.providers.iter_mut().find(|x| x.name == p.name) {
                if p.api_key.is_some() {
                    existing.api_key = p.api_key;
                }
                if p.base_url.is_some() {
                    existing.base_url = p.base_url;
                }
                if !p.api.is_empty() {
                    existing.api = p.api;
                }
            } else {
                self.providers.push(p);
            }
        }
        for m in project.models {
            if let Some(existing) = self.models.iter_mut().find(|x| x.name == m.name) {
                *existing = m;
            } else {
                self.models.push(m);
            }
        }
        self.hooks
            .pre_tool_use
            .extend(project.hooks.pre_tool_use);
        self.hooks
            .post_tool_use
            .extend(project.hooks.post_tool_use);
    }
""",
        )

    methods = r'''
    /// Built-in default context window guesses by model id pattern.
    pub fn default_context_window_for(model_id: &str) -> u64 {
        let m = model_id.to_ascii_lowercase();
        if m.starts_with("claude") {
            200_000
        } else if m.contains("gpt-4o") || m.starts_with("o1") || m.starts_with("o3") {
            128_000
        } else if m.contains("gpt-4") {
            128_000
        } else {
            128_000
        }
    }

    /// Effective providers list: configured `[[providers]]` plus legacy openai/anthropic.
    pub fn effective_providers(&self) -> Vec<NamedProviderConfig> {
        let mut out = self.providers.clone();

        if !out.iter().any(|p| p.name == "openai") {
            out.push(NamedProviderConfig {
                name: "openai".into(),
                api: "openai".into(),
                api_key: self.openai.api_key.clone(),
                base_url: self
                    .openai
                    .base_url
                    .clone()
                    .or_else(|| Some("https://api.openai.com/v1".into())),
            });
        } else if let Some(p) = out.iter_mut().find(|p| p.name == "openai") {
            if self.openai.api_key.is_some() {
                p.api_key = self.openai.api_key.clone();
            }
            if self.openai.base_url.is_some() {
                p.base_url = self.openai.base_url.clone();
            }
        }

        if !out.iter().any(|p| p.name == "anthropic") {
            out.push(NamedProviderConfig {
                name: "anthropic".into(),
                api: "anthropic".into(),
                api_key: self.anthropic.api_key.clone(),
                base_url: self
                    .anthropic
                    .base_url
                    .clone()
                    .or_else(|| Some("https://api.anthropic.com".into())),
            });
        } else if let Some(p) = out.iter_mut().find(|p| p.name == "anthropic") {
            if self.anthropic.api_key.is_some() {
                p.api_key = self.anthropic.api_key.clone();
            }
            if self.anthropic.base_url.is_some() {
                p.base_url = self.anthropic.base_url.clone();
            }
        }

        out
    }

    /// Resolve a model selection (alias, catalog name, provider/model, or raw id).
    pub fn resolve_model(&self, selection: &str) -> Result<ResolvedModel, ConfigError> {
        let selection = selection.trim();
        if selection.is_empty() {
            return Err(ConfigError::Invalid("model selection is empty".into()));
        }

        let providers = self.effective_providers();

        if let Some(entry) = self.models.iter().find(|m| m.name == selection) {
            return self.resolve_from_catalog_entry(entry, &providers);
        }

        if let Some((prov_name, remote)) = selection.split_once('/') {
            if let Some(provider) = providers.iter().find(|p| p.name == prov_name) {
                return self.resolve_with_provider(
                    selection,
                    remote,
                    provider,
                    None,
                    None,
                    None,
                );
            }
        }

        let aliased = match selection.to_ascii_lowercase().as_str() {
            "opus" => Some("claude-opus-4-20250514"),
            "sonnet" => Some("claude-sonnet-4-20250514"),
            "haiku" => Some("claude-3-5-haiku-20241022"),
            "gpt-4" | "gpt-4o" => Some("gpt-4o"),
            "gpt-4-mini" | "gpt-4o-mini" => Some("gpt-4o-mini"),
            _ => None,
        };
        if let Some(remote) = aliased {
            if let Some(entry) = self
                .models
                .iter()
                .find(|m| m.name == remote || m.model.as_deref() == Some(remote))
            {
                return self.resolve_from_catalog_entry(entry, &providers);
            }
            let provider_name = if remote.starts_with("claude") {
                "anthropic"
            } else {
                "openai"
            };
            let provider = providers
                .iter()
                .find(|p| p.name == provider_name)
                .ok_or_else(|| {
                    ConfigError::Invalid(format!("provider '{provider_name}' not configured"))
                })?;
            return self.resolve_with_provider(
                selection,
                remote,
                provider,
                Some(Self::default_context_window_for(remote)),
                None,
                None,
            );
        }

        let provider = if selection.to_ascii_lowercase().starts_with("claude") {
            providers
                .iter()
                .find(|p| p.name == "anthropic" || p.api.eq_ignore_ascii_case("anthropic"))
        } else {
            providers
                .iter()
                .find(|p| p.name == "openai")
                .or_else(|| {
                    providers
                        .iter()
                        .find(|p| p.api.eq_ignore_ascii_case("openai"))
                })
        }
        .ok_or_else(|| ConfigError::Invalid("no suitable provider configured".into()))?;

        self.resolve_with_provider(
            selection,
            selection,
            provider,
            Some(Self::default_context_window_for(selection)),
            None,
            None,
        )
    }

    fn resolve_from_catalog_entry(
        &self,
        entry: &ModelConfig,
        providers: &[NamedProviderConfig],
    ) -> Result<ResolvedModel, ConfigError> {
        let provider = providers
            .iter()
            .find(|p| p.name == entry.provider)
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "model '{}' references unknown provider '{}'",
                    entry.name, entry.provider
                ))
            })?;
        let remote = entry.model.clone().unwrap_or_else(|| entry.name.clone());
        self.resolve_with_provider(
            &entry.name,
            &remote,
            provider,
            entry
                .context_window
                .or(Some(Self::default_context_window_for(&remote))),
            entry.max_tokens,
            entry.temperature,
        )
    }

    fn resolve_with_provider(
        &self,
        name: &str,
        remote_model: &str,
        provider: &NamedProviderConfig,
        context_window: Option<u64>,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
    ) -> Result<ResolvedModel, ConfigError> {
        let api = ApiFormat::parse(&provider.api).map_err(ConfigError::Invalid)?;
        let api_key = provider
            .api_key
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "provider '{}' is missing api_key (set [[providers]] or env)",
                    provider.name
                ))
            })?;
        let default_url = match api {
            ApiFormat::OpenAI => "https://api.openai.com/v1",
            ApiFormat::Anthropic => "https://api.anthropic.com",
        };
        let base_url = provider
            .base_url
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_url.to_string());
        let context_window =
            context_window.unwrap_or_else(|| Self::default_context_window_for(remote_model));

        Ok(ResolvedModel {
            name: name.to_string(),
            remote_model: remote_model.to_string(),
            provider_name: provider.name.clone(),
            api,
            api_key,
            base_url,
            context_window,
            max_tokens,
            temperature,
        })
    }
'''

    if "fn resolve_model(" not in text:
        marker = "\n}\n\n#[cfg(test)]\n"
        idx = text.rfind(marker)
        if idx < 0:
            raise SystemExit("impl end marker not found")
        # insert before the impl-closing brace that precedes cfg(test)
        # find the last "\n}\n\n#[cfg(test)]" and insert methods before that closing brace
        text = text[:idx] + methods + text[idx:]

    extra_tests = '''
    #[test]
    fn test_resolve_model_catalog_with_context_window() {
        let config = AppConfig {
            providers: vec![NamedProviderConfig {
                name: "deepseek".into(),
                api: "openai".into(),
                api_key: Some("sk-ds".into()),
                base_url: Some("https://api.deepseek.com".into()),
            }],
            models: vec![ModelConfig {
                name: "ds-chat".into(),
                provider: "deepseek".into(),
                model: Some("deepseek-chat".into()),
                context_window: Some(64_000),
                max_tokens: Some(2048),
                temperature: None,
                notes: Some("DeepSeek chat".into()),
            }],
            openai: ProviderConfig {
                api_key: Some("sk-oai".into()),
                base_url: None,
            },
            anthropic: ProviderConfig {
                api_key: Some("sk-ant".into()),
                base_url: None,
            },
            ..AppConfig::default()
        };

        let resolved = config.resolve_model("ds-chat").unwrap();
        assert_eq!(resolved.remote_model, "deepseek-chat");
        assert_eq!(resolved.provider_name, "deepseek");
        assert_eq!(resolved.api, ApiFormat::OpenAI);
        assert_eq!(resolved.base_url, "https://api.deepseek.com");
        assert_eq!(resolved.context_window, 64_000);
        assert_eq!(resolved.max_tokens, Some(2048));
        assert_eq!(resolved.compact_threshold(100_000), 48_000);

        let slash = config.resolve_model("deepseek/deepseek-reasoner").unwrap();
        assert_eq!(slash.remote_model, "deepseek-reasoner");
        assert_eq!(slash.provider_name, "deepseek");
    }

    #[test]
    fn test_resolve_builtin_alias_uses_legacy_provider() {
        let config = AppConfig {
            anthropic: ProviderConfig {
                api_key: Some("sk-ant".into()),
                base_url: None,
            },
            openai: ProviderConfig {
                api_key: Some("sk-oai".into()),
                base_url: None,
            },
            ..AppConfig::default()
        };
        let r = config.resolve_model("sonnet").unwrap();
        assert_eq!(r.api, ApiFormat::Anthropic);
        assert!(r.remote_model.starts_with("claude"));
        assert_eq!(r.context_window, 200_000);
    }
'''

    if "test_resolve_model_catalog_with_context_window" not in text:
        # insert before final tests module close
        if text.rstrip().endswith("}"):
            # last } closes tests module
            text = text.rstrip()[:-1] + extra_tests + "}\n"

    path.write_text(text, encoding="utf-8")
    print("patched", path)


def patch_config_lib() -> None:
    path = ROOT / "crates/pigs-config/src/lib.rs"
    text = path.read_text(encoding="utf-8")
    text = text.replace(
        """pub use config::{
    AppConfig, HookEntry, HooksConfig, McpServerConfigEntry, ProviderConfig,
};
""",
        """pub use config::{
    ApiFormat, AppConfig, HookEntry, HooksConfig, McpServerConfigEntry, ModelConfig,
    NamedProviderConfig, ProviderConfig, ResolvedModel,
};
""",
    )
    path.write_text(text, encoding="utf-8")
    print("patched", path)


def write_provider_rs() -> None:
    path = ROOT / "crates/pigs-llm/src/provider.rs"
    path.write_text(
        '''//! Client factory for Anthropic Messages and OpenAI Chat Completions.
//!
//! Wire formats only. Multi-vendor endpoints are configured via named providers
//! in `pigs-config` (`[[providers]]` + `[[models]]`).

use std::sync::Arc;

use pigs_core::ApiClient;

use crate::anthropic::AnthropicClient;
use crate::openai::OpenAiClient;

/// Wire format used to talk to an LLM endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// OpenAI Chat Completions (`/v1/chat/completions`), including compatible proxies.
    OpenAI,
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Credentials and endpoint for creating a client.
#[derive(Debug, Clone, Default)]
pub struct ClientConfig<'a> {
    pub openai_api_key: Option<&'a str>,
    pub openai_base_url: Option<&'a str>,
    pub anthropic_api_key: Option<&'a str>,
    pub anthropic_base_url: Option<&'a str>,
}

/// Create a client from an explicit API format + credentials.
pub fn create_client_for_endpoint(
    api: Provider,
    model: &str,
    api_key: &str,
    base_url: &str,
) -> Result<Arc<dyn ApiClient>, pigs_core::ApiError> {
    if model.trim().is_empty() {
        return Err(pigs_core::ApiError::Config("model id is empty".into()));
    }
    if api_key.trim().is_empty() {
        return Err(pigs_core::ApiError::Config("api_key is empty".into()));
    }
    let base_url = if base_url.trim().is_empty() {
        match api {
            Provider::OpenAI => "https://api.openai.com/v1",
            Provider::Anthropic => "https://api.anthropic.com",
        }
    } else {
        base_url
    };

    match api {
        Provider::Anthropic => Ok(Arc::new(AnthropicClient::new(api_key, model, base_url))),
        Provider::OpenAI => Ok(Arc::new(OpenAiClient::new(api_key, model, base_url))),
    }
}

/// Legacy helper: infer format from model name (`claude*` => Anthropic).
pub fn detect_provider(model: &str) -> Provider {
    if model.to_lowercase().starts_with("claude") {
        Provider::Anthropic
    } else {
        Provider::OpenAI
    }
}

/// Built-in short aliases (not a vendor catalog).
pub fn resolve_model_alias(alias: &str) -> String {
    match alias.to_lowercase().as_str() {
        "opus" => "claude-opus-4-20250514".to_string(),
        "sonnet" => "claude-sonnet-4-20250514".to_string(),
        "haiku" => "claude-3-5-haiku-20241022".to_string(),
        "gpt-4" | "gpt-4o" => "gpt-4o".to_string(),
        "gpt-4-mini" | "gpt-4o-mini" => "gpt-4o-mini".to_string(),
        _ => alias.to_string(),
    }
}

/// Create an API client using legacy dual-slot credentials and model-name routing.
pub fn create_client(
    model: &str,
    openai_api_key: Option<&str>,
    openai_base_url: Option<&str>,
    anthropic_api_key: Option<&str>,
    anthropic_base_url: Option<&str>,
) -> Result<Arc<dyn ApiClient>, pigs_core::ApiError> {
    create_client_with_config(
        model,
        ClientConfig {
            openai_api_key,
            openai_base_url,
            anthropic_api_key,
            anthropic_base_url,
        },
    )
}

/// Create an API client with full legacy configuration.
pub fn create_client_with_config(
    model: &str,
    config: ClientConfig<'_>,
) -> Result<Arc<dyn ApiClient>, pigs_core::ApiError> {
    let model = resolve_model_alias(model);
    match detect_provider(&model) {
        Provider::Anthropic => {
            let api_key = config.anthropic_api_key.ok_or_else(|| {
                pigs_core::ApiError::Config(
                    "ANTHROPIC_API_KEY is required for Claude models.".into(),
                )
            })?;
            let base_url = config
                .anthropic_base_url
                .unwrap_or("https://api.anthropic.com");
            create_client_for_endpoint(Provider::Anthropic, &model, api_key, base_url)
        }
        Provider::OpenAI => {
            let api_key = config.openai_api_key.ok_or_else(|| {
                pigs_core::ApiError::Config(
                    "OPENAI_API_KEY is required for OpenAI Chat Completions models.".into(),
                )
            })?;
            let base_url = config
                .openai_base_url
                .unwrap_or("https://api.openai.com/v1");
            create_client_for_endpoint(Provider::OpenAI, &model, api_key, base_url)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_detect_provider() {
        assert_eq!(detect_provider("claude-sonnet-4-20250514"), Provider::Anthropic);
        assert_eq!(detect_provider("gpt-4o"), Provider::OpenAI);
        assert_eq!(detect_provider("deepseek-chat"), Provider::OpenAI);
    }

    #[test]
    fn test_create_for_endpoint() {
        let client = create_client_for_endpoint(
            Provider::OpenAI,
            "my-model",
            "dummy",
            "http://127.0.0.1:8080/v1",
        )
        .unwrap();
        assert_eq!(client.model(), "my-model");
    }

    #[test]
    fn test_create_client_missing_key() {
        assert!(create_client("claude-sonnet-4-20250514", None, None, None, None).is_err());
        assert!(create_client("gpt-4o", None, None, None, None).is_err());
    }
}
''',
        encoding="utf-8",
    )
    print("wrote", path)

    lib = ROOT / "crates/pigs-llm/src/lib.rs"
    lib.write_text(
        """//! LLM provider clients — OpenAI Chat Completions and Anthropic Messages APIs.

pub mod anthropic;
pub mod openai;
pub mod provider;

pub use anthropic::AnthropicClient;
pub use openai::OpenAiClient;
pub use provider::{
    create_client, create_client_for_endpoint, create_client_with_config, detect_provider,
    resolve_model_alias, ClientConfig, Provider,
};
""",
        encoding="utf-8",
    )
    print("wrote", lib)


def patch_agent() -> None:
    path = ROOT / "crates/pigs-cli/src/agent.rs"
    text = path.read_text(encoding="utf-8")

    text = text.replace(
        "use pigs_llm::{create_client_with_config, resolve_model_alias, ClientConfig};\n",
        "use pigs_config::{ApiFormat, ResolvedModel};\nuse pigs_llm::{create_client_for_endpoint, Provider as LlmProvider};\n",
    )

    helper = '''
    fn llm_provider_from_api(api: ApiFormat) -> LlmProvider {
        match api {
            ApiFormat::OpenAI => LlmProvider::OpenAI,
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

'''

    if "fn create_api_client(" not in text:
        # insert after impl Agent {
        text = text.replace("impl Agent {\n", "impl Agent {\n" + helper, 1)

    # Replace Agent::new client creation block
    old_new = '''        // Resolve model alias
        let model = resolve_model_alias(&config.model);

        // Create API client
        let api_client = create_client_with_config(
            &model,
            ClientConfig {
                openai_api_key: config.openai.api_key.as_deref(),
                openai_base_url: config.openai.base_url.as_deref(),
                anthropic_api_key: config.anthropic.api_key.as_deref(),
                anthropic_base_url: config.anthropic.base_url.as_deref(),
            },
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        // Create or resume session
        let sessions_dir = AppConfig::sessions_dir();
        let session = if let Some(resume_id) = &args.resume {
            Session::load(&sessions_dir, resume_id)
                .map_err(|e| anyhow::anyhow!("Failed to resume session: {e}"))?
        } else {
            Session::new(&model)
        };
'''
    new_new = '''        // Resolve model against configured providers/models catalog
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
'''
    if old_new not in text:
        raise SystemExit("Agent::new model block not found")
    text = text.replace(old_new, new_new)

    # set_model
    old_set = '''    pub fn set_model(&mut self, model: &str) -> anyhow::Result<()> {
        let model = resolve_model_alias(model);
        let api_client = create_client_with_config(
            &model,
            ClientConfig {
                openai_api_key: self.config.openai.api_key.as_deref(),
                openai_base_url: self.config.openai.base_url.as_deref(),
                anthropic_api_key: self.config.anthropic.api_key.as_deref(),
                anthropic_base_url: self.config.anthropic.base_url.as_deref(),
            },
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        self.api_client = api_client;
        self.session.model = model.clone();
        self.config.model = model;

        Ok(())
    }
'''
    new_set = '''    pub fn set_model(&mut self, model: &str) -> anyhow::Result<()> {
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
'''
    if old_set not in text:
        raise SystemExit("set_model block not found")
    text = text.replace(old_set, new_set)

    # reload_config client creation
    old_reload = '''        let model = resolve_model_alias(&config.model);
        let api_client = create_client_with_config(
            &model,
            ClientConfig {
                openai_api_key: config.openai.api_key.as_deref(),
                openai_base_url: config.openai.base_url.as_deref(),
                anthropic_api_key: config.anthropic.api_key.as_deref(),
                anthropic_base_url: config.anthropic.base_url.as_deref(),
            },
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
'''
    new_reload = '''        let resolved = config
            .resolve_model(&config.model)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let model = resolved.remote_model.clone();
        let api_client = Self::create_api_client(&resolved)?;
'''
    if old_reload not in text:
        raise SystemExit("reload_config block not found")
    text = text.replace(old_reload, new_reload)

    # compact_config uses model context window
    old_compact = '''    fn compact_config(&self, force: bool) -> CompactConfig {
        CompactConfig {
            token_threshold: self.config.compact_token_threshold,
            keep_recent: self.config.compact_keep_recent.max(1),
            summary_message_chars: 400,
            force,
        }
    }
'''
    new_compact = '''    fn compact_config(&self, force: bool) -> CompactConfig {
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
'''
    if old_compact not in text:
        raise SystemExit("compact_config not found")
    text = text.replace(old_compact, new_compact)

    # per-model max_tokens/temperature in run_turn request building if present
    text = text.replace(
        """.with_max_tokens(self.config.max_tokens)
                .with_temperature(self.config.temperature);""",
        """.with_max_tokens(
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
                );""",
    )

    path.write_text(text, encoding="utf-8")
    print("patched", path)


def write_models_rs() -> None:
    path = ROOT / "crates/pigs-cli/src/models.rs"
    path.write_text(
        '''//! Model catalog display for /models.

use pigs_config::{ApiFormat, AppConfig, ResolvedModel};

pub fn provider_format_name(api: ApiFormat) -> &'static str {
    api.as_str()
}

pub fn print_models(config: &AppConfig, current_selection: &str, current_remote: &str) {
    println!("API formats: anthropic (Messages) | openai (Chat Completions)");
    println!();

    let providers = config.effective_providers();
    if !providers.is_empty() {
        println!("Configured providers:");
        println!("{:<16} {:<12} {:<40} Key", "Name", "API", "Base URL");
        println!("{}", "-".repeat(90));
        for p in &providers {
            let key = if p.api_key.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
                "set"
            } else {
                "missing"
            };
            let url = p.base_url.as_deref().unwrap_or("-");
            println!("{:<16} {:<12} {:<40} {key}", p.name, p.api, truncate(url, 40));
        }
        println!();
    }

    if config.models.is_empty() {
        println!("No [[models]] catalog entries. Using built-in aliases + raw model ids.");
        println!("  aliases: opus, sonnet, haiku, gpt-4o, gpt-4o-mini");
        println!("  or: provider/model-id  e.g. deepseek/deepseek-chat");
    } else {
        println!("Configured models:");
        println!(
            "{:<14} {:<22} {:<12} {:>10} Notes",
            "Name", "Remote", "Provider", "Ctx"
        );
        println!("{}", "-".repeat(90));
        for m in &config.models {
            let remote = m.model.as_deref().unwrap_or(&m.name);
            let ctx = m
                .context_window
                .unwrap_or_else(|| AppConfig::default_context_window_for(remote));
            let mark = if current_selection == m.name
                || current_remote == remote
                || current_selection == remote
            {
                "*"
            } else {
                " "
            };
            let notes = m.notes.as_deref().unwrap_or("");
            println!(
                "{mark}{:<13} {:<22} {:<12} {:>10} {notes}",
                m.name, remote, m.provider, ctx
            );
        }
    }

    println!();
    match config.resolve_model(current_selection) {
        Ok(r) => print_resolved(&r),
        Err(e) => println!("Current selection '{current_selection}' unresolved: {e}"),
    }
    println!("Use: /model <name|alias|provider/model-id|raw-id>");
}

fn print_resolved(r: &ResolvedModel) {
    println!(
        "Current: {} -> {} via {} ({})  context_window={}",
        r.name,
        r.remote_model,
        r.provider_name,
        provider_format_name(r.api),
        r.context_window
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{t}...")
    }
}
''',
        encoding="utf-8",
    )
    print("wrote", path)


def patch_commands_and_doctor() -> None:
    cmd = ROOT / "crates/pigs-cli/src/commands.rs"
    ct = cmd.read_text(encoding="utf-8")
    ct = ct.replace(
        "crate::models::print_models(agent.api_client.model());",
        "crate::models::print_models(&agent.config, &agent.config.model, agent.api_client.model());",
    )
    ct = ct.replace(
        'println!("  /models           List known model aliases");',
        'println!("  /models           List providers/models (with context_window)");',
    )
    ct = ct.replace(
        'println!("  /model <name>     Switch model (opus/sonnet/haiku/gpt-4o or full id)");',
        'println!("  /model <name>     Switch model (catalog name, alias, provider/id, or raw id)");',
    )
    cmd.write_text(ct, encoding="utf-8")
    print("patched commands")

    doc = ROOT / "crates/pigs-cli/src/doctor.rs"
    dt = doc.read_text(encoding="utf-8")
    # simplify credential check to resolved model
    old = """    // API keys / providers for current model
    let model = agent.api_client.model().to_string();
    let provider = detect_provider(&model);
    let (need, present, hint) = match provider {
        pigs_llm::Provider::Anthropic => (
            "ANTHROPIC_API_KEY / config.anthropic.api_key".to_string(),
            agent
                .config
                .anthropic
                .api_key
                .as_ref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "set anthropic.api_key or ANTHROPIC_API_KEY".to_string(),
        ),
        pigs_llm::Provider::OpenAI => (
            "OPENAI_API_KEY / config.openai.api_key".to_string(),
            agent
                .config
                .openai
                .api_key
                .as_ref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "set openai.api_key or OPENAI_API_KEY (use openai.base_url for compatible endpoints)"
                .to_string(),
        ),
    };
    items.push(CheckItem {
        name: format!("API credentials ({model})"),
        ok: present,
        detail: if present {
            format!("ok ({need})")
        } else {
            format!("missing ({hint})")
        },
    });
"""
    new = """    // Resolved model + provider credentials
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
"""
    if "use pigs_llm::detect_provider;" in dt:
        dt = dt.replace("use pigs_llm::detect_provider;\n", "")
    if old in dt:
        dt = dt.replace(old, new)
    elif "Resolved model + provider credentials" not in dt:
        raise SystemExit("doctor credential block not found for replace")
    doc.write_text(dt, encoding="utf-8")
    print("patched doctor")


def patch_docs() -> None:
    readme = ROOT / "README.md"
    r = readme.read_text(encoding="utf-8")
    sample = '''
# Multi-provider / multi-model catalog (optional)
# [[providers]]
# name = "openai"
# api = "openai"
# api_key = "sk-..."
# base_url = "https://api.openai.com/v1"
#
# [[providers]]
# name = "deepseek"
# api = "openai"
# api_key = "sk-..."
# base_url = "https://api.deepseek.com"
#
# [[providers]]
# name = "ollama"
# api = "openai"
# api_key = "ollama"
# base_url = "http://localhost:11434/v1"
#
# [[providers]]
# name = "anthropic"
# api = "anthropic"
# api_key = "sk-ant-..."
#
# [[models]]
# name = "sonnet"
# provider = "anthropic"
# model = "claude-sonnet-4-20250514"
# context_window = 200000
#
# [[models]]
# name = "ds-chat"
# provider = "deepseek"
# model = "deepseek-chat"
# context_window = 65536
# max_tokens = 4096
#
'''
    if "[[providers]]" not in r:
        r = r.replace(
            "# Optional MCP servers (stdio)\n",
            sample + "# Optional MCP servers (stdio)\n",
        )
    r = r.replace(
        "- **双 API 格式** — Anthropic Messages + OpenAI Chat Completions（兼容端点通过 `openai.base_url` 接入，不内置第三方供应商）",
        "- **多供应商多模型** — 配置 `[[providers]]` / `[[models]]`；仅两种线格式（Anthropic Messages / OpenAI Chat Completions）；每模型可设 `context_window`",
    )
    readme.write_text(r, encoding="utf-8")

    agents = ROOT / "AGENTS.md"
    a = agents.read_text(encoding="utf-8")
    a = a.replace(
        "双 API 格式支持（Anthropic Messages + OpenAI Chat Completions + SSE 流式）",
        "多供应商多模型（`[[providers]]`/`[[models]]` + context_window；线格式仅 Anthropic Messages / OpenAI Chat Completions + SSE）",
    )
    if "context_window" not in a:
        a = a.replace(
            "- **pigs-llm** 实现 `ApiClient` trait：仅 Anthropic Messages 与 OpenAI Chat Completions 两种线格式；`claude*` 走 Anthropic，其余走 OpenAI（可用 base_url 接兼容端点）。不内置第三方供应商品牌。",
            "- **pigs-llm** 实现 `ApiClient` trait：仅 Anthropic Messages 与 OpenAI Chat Completions 两种线格式。\n"
            "- **多供应商/多模型**：在 config 中用 `[[providers]]`（name/api/api_key/base_url）与 `[[models]]`（name/provider/model/context_window）配置；`/model` 解析 catalog 名、`provider/model`、内置别名或原始 id。\n"
            "- **context_window**：模型级上下文长度（tokens），自动压缩阈值取 `min(compact_token_threshold, context_window*0.75)`。",
        )
    agents.write_text(a, encoding="utf-8")
    print("docs patched")


def main() -> None:
    patch_config()
    patch_config_lib()
    write_provider_rs()
    patch_agent()
    write_models_rs()
    patch_commands_and_doctor()
    patch_docs()
    print("done")


if __name__ == "__main__":
    main()
