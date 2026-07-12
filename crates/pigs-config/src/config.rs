//! TOML configuration file loading and environment variable overrides.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use pigs_permissions::PermissionMode;

use crate::language::Language;

/// Error type for configuration operations.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Read(String),
    #[error("Failed to parse config: {0}")]
    Parse(String),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

/// Provider-specific configuration (API keys, base URLs).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}


/// Wire API format for a named provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiFormat {
    /// OpenAI Responses API (`/v1/responses`) — current OpenAI protocol.
    #[default]
    OpenAI,
    /// OpenAI Chat Completions (`/v1/chat/completions`) — legacy/compatible endpoints.
    OpenAIChat,
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
}

impl ApiFormat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" | "responses" | "openai-responses" => Ok(Self::OpenAI),
            "openai-chat" | "chat" | "completions" | "openai-compatible" => Ok(Self::OpenAIChat),
            "anthropic" | "claude" | "messages" => Ok(Self::Anthropic),
            other => Err(format!(
                "unknown api format '{other}' (expected openai, openai-chat, or anthropic)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::OpenAIChat => "openai-chat",
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
    /// Local selection name (alias or raw id). Includes `-pig` suffix if phased.
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
    /// True if this model was selected with a `-pig` suffix (phased runtime).
    pub is_pig: bool,
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

/// Configuration for an MCP server process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfigEntry {
    /// Server name (used as MCP tool prefix).
    pub name: String,
    /// Command to launch the server.
    pub command: String,
    /// Command arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Whether this server is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// The root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Default model to use.
    #[serde(default = "default_model")]
    pub model: String,
    /// Default permission mode.
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// UI / default reply language: `en` or `zh`.
    ///
    /// Affects the built-in system prompt language preference and REPL chrome.
    /// Slash commands accept Chinese / pinyin aliases regardless of this setting.
    #[serde(default = "default_language")]
    pub language: String,
    /// Maximum number of agent loop iterations per turn.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// Maximum output tokens for LLM responses.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Temperature for LLM responses.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// OpenAI Chat Completions configuration (also used for compatible endpoints).
    #[serde(default)]
    pub openai: ProviderConfig,
    /// Anthropic Messages API configuration.
    #[serde(default)]
    pub anthropic: ProviderConfig,
    /// Custom system prompt (prepended to the default one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Enable file logging to ~/.pigs/logs/.
    #[serde(default = "default_log_to_file")]
    pub log_to_file: bool,
    /// Log level: error, warn, info, debug, trace.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// MCP servers to auto-connect on startup.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfigEntry>,
    /// Auto-compact estimated-token threshold.
    #[serde(default = "default_compact_token_threshold")]
    pub compact_token_threshold: u64,
    /// Messages retained fully during compaction.
    #[serde(default = "default_compact_keep_recent")]
    pub compact_keep_recent: usize,
    /// Named provider endpoints (multi-vendor). Prefer this over legacy openai/anthropic blocks.
    #[serde(default)]
    pub providers: Vec<NamedProviderConfig>,
    /// Local model catalog with optional context_window and provider binding.
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// Tool lifecycle hooks.
    #[serde(default)]
    pub hooks: HooksConfig,
}

/// Collection of lifecycle hooks.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HooksConfig {
    /// Hooks run before a tool executes. Non-zero exit denies the tool.
    #[serde(default)]
    pub pre_tool_use: Vec<HookEntry>,
    /// Hooks run after a tool executes successfully or with error.
    #[serde(default)]
    pub post_tool_use: Vec<HookEntry>,
}

/// A single shell hook entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    /// Tool name matcher. Supports exact name or `*` for all tools.
    /// Prefix match with trailing `*` e.g. `mcp_*`.
    #[serde(default = "default_matcher")]
    pub matcher: String,
    /// Shell command to run.
    pub command: String,
    /// Optional timeout in seconds (default 30).
    #[serde(default = "default_hook_timeout")]
    pub timeout: u64,
    /// Whether this hook is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_matcher() -> String {
    "*".to_string()
}

fn default_hook_timeout() -> u64 {
    30
}

fn default_log_to_file() -> bool {
    true
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_permission_mode() -> String {
    "workspace_write".to_string()
}

fn default_language() -> String {
    Language::Zh.as_str().to_string()
}

fn default_max_turns() -> u32 {
    50
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_temperature() -> f32 {
    1.0
}

fn default_compact_token_threshold() -> u64 {
    100_000
}

fn default_compact_keep_recent() -> usize {
    4
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            model: default_model(),
            permission_mode: default_permission_mode(),
            language: default_language(),
            max_turns: default_max_turns(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            compact_token_threshold: default_compact_token_threshold(),
            compact_keep_recent: default_compact_keep_recent(),
            openai: ProviderConfig::default(),
            anthropic: ProviderConfig::default(),
            system_prompt: None,
            log_to_file: default_log_to_file(),
            log_level: default_log_level(),
            mcp_servers: Vec::new(),
            providers: Vec::new(),
            models: Vec::new(),
            hooks: HooksConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from the default location (`~/.pigs/config.toml`).
    /// Falls back to defaults if the file doesn't exist.
    pub fn load() -> Result<Self, ConfigError> {
        let config_path = Self::config_path();
        if config_path.exists() {
            Self::load_from(&config_path)
        } else {
            Ok(Self::default())
        }
    }

    /// Load layered configuration:
    /// 1. `~/.pigs/config.toml` (global)
    /// 2. `{workspace}/.pigs/config.toml` (project overrides, if present)
    /// 3. `{workspace}/.pigs/config.local.toml` (machine-local overrides; gitignored)
    pub fn load_layered(workspace: &Path) -> Result<Self, ConfigError> {
        let mut config = Self::load()?;
        let project_path = workspace.join(".pigs").join("config.toml");
        if project_path.exists() {
            let project = Self::load_from(&project_path)?;
            config.merge_project_overrides(project);
        }
        // Local-only overlay for machine-specific endpoints (e.g. llama-server).
        // Intended to stay out of git via `.pigs/config.local.toml` / `.pigs/*.local.*`.
        let local_path = workspace.join(".pigs").join("config.local.toml");
        if local_path.exists() {
            let local = Self::load_from(&local_path)?;
            config.merge_project_overrides(local);
        }
        Ok(config)
    }

    /// Merge project-level overrides on top of the current config.
    /// Fields that differ from defaults in `project` replace the current values.
    /// Lists (`mcp_servers`, hooks) are extended rather than replaced.
    pub fn merge_project_overrides(&mut self, project: AppConfig) {
        let defaults = AppConfig::default();

        if project.model != defaults.model {
            self.model = project.model;
        }
        if project.permission_mode != defaults.permission_mode {
            self.permission_mode = project.permission_mode;
        }
        if project.language != defaults.language {
            self.language = project.language;
        }
        if project.max_turns != defaults.max_turns {
            self.max_turns = project.max_turns;
        }
        if project.max_tokens != defaults.max_tokens {
            self.max_tokens = project.max_tokens;
        }
        if (project.temperature - defaults.temperature).abs() > f32::EPSILON {
            self.temperature = project.temperature;
        }
        if project.compact_token_threshold != defaults.compact_token_threshold {
            self.compact_token_threshold = project.compact_token_threshold;
        }
        if project.compact_keep_recent != defaults.compact_keep_recent {
            self.compact_keep_recent = project.compact_keep_recent;
        }
        if project.system_prompt.is_some() {
            self.system_prompt = project.system_prompt;
        }
        if project.log_to_file != defaults.log_to_file {
            self.log_to_file = project.log_to_file;
        }
        if project.log_level != defaults.log_level {
            self.log_level = project.log_level;
        }

        // Provider overlays: only if project set a key/url
        if project.openai.api_key.is_some() {
            self.openai.api_key = project.openai.api_key;
        }
        if project.openai.base_url.is_some() {
            self.openai.base_url = project.openai.base_url;
        }
        if project.anthropic.api_key.is_some() {
            self.anthropic.api_key = project.anthropic.api_key;
        }
        if project.anthropic.base_url.is_some() {
            self.anthropic.base_url = project.anthropic.base_url;
        }

        // Append project MCP servers, providers, models, hooks
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

    /// Load configuration from a specific file path.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Read(format!("Failed to read {path:?}: {e}")))?;
        let config: AppConfig = toml::from_str(&content)
            .map_err(|e| ConfigError::Parse(format!("Failed to parse TOML: {e}")))?;
        Ok(config)
    }

    /// Get the default config file path (`~/.pigs/config.toml`).
    pub fn config_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".pigs").join("config.toml")
    }

    /// Get the sessions directory (`~/.pigs/sessions/`).
    pub fn sessions_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".pigs").join("sessions")
    }

    /// Get the logs directory (`~/.pigs/logs/`).
    pub fn logs_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".pigs").join("logs")
    }

    /// Apply environment variable overrides.
    /// Priority: env vars > config file values.
    pub fn with_env_overrides(mut self) -> Self {
        if let Ok(model) = std::env::var("PIGS_MODEL") {
            self.model = model;
        }
        if let Ok(mode) = std::env::var("PIGS_PERMISSION_MODE") {
            self.permission_mode = mode;
        }
        if let Ok(lang) = std::env::var("PIGS_LANGUAGE") {
            self.language = lang;
        }
        if let Ok(max_turns) = std::env::var("PIGS_MAX_TURNS") {
            if let Ok(parsed) = max_turns.parse::<u32>() {
                self.max_turns = parsed;
            }
        }
        if let Ok(max_tokens) = std::env::var("PIGS_MAX_TOKENS") {
            if let Ok(parsed) = max_tokens.parse::<u32>() {
                self.max_tokens = parsed;
            }
        }
        if let Ok(temp) = std::env::var("PIGS_TEMPERATURE") {
            if let Ok(parsed) = temp.parse::<f32>() {
                self.temperature = parsed;
            }
        }
        if let Ok(v) = std::env::var("PIGS_COMPACT_TOKEN_THRESHOLD") {
            if let Ok(parsed) = v.parse::<u64>() {
                self.compact_token_threshold = parsed;
            }
        }
        if let Ok(v) = std::env::var("PIGS_COMPACT_KEEP_RECENT") {
            if let Ok(parsed) = v.parse::<usize>() {
                self.compact_keep_recent = parsed;
            }
        }
        if let Ok(prompt) = std::env::var("PIGS_SYSTEM_PROMPT") {
            self.system_prompt = Some(prompt);
        }

        // Provider API keys from env
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            self.openai.api_key = Some(key);
        }
        if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
            self.openai.base_url = Some(url);
        }
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            self.anthropic.api_key = Some(key);
        }
        if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
            self.anthropic.base_url = Some(url);
        }
        if let Ok(level) = std::env::var("PIGS_LOG_LEVEL") {
            self.log_level = level;
        }
        if let Ok(log_to_file) = std::env::var("PIGS_LOG_TO_FILE") {
            self.log_to_file = matches!(log_to_file.to_lowercase().as_str(), "1" | "true" | "yes");
        }

        self
    }

    /// Parse the permission mode string into a PermissionMode.
    pub fn permission_mode_parsed(&self) -> Result<PermissionMode, ConfigError> {
        use std::str::FromStr;
        PermissionMode::from_str(&self.permission_mode).map_err(ConfigError::Invalid)
    }

    /// Parse the language preference (`en` / `zh`).
    pub fn language_parsed(&self) -> Result<Language, ConfigError> {
        self.language
            .parse::<Language>()
            .map_err(ConfigError::Invalid)
    }

    /// Effective language, falling back to English on invalid values.
    pub fn language_or_default(&self) -> Language {
        self.language_parsed().unwrap_or_default()
    }

    /// Save the configuration to the default location.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Self::config_path())
    }

    /// Save configuration to a specific path (creates parent dirs).
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError::Read(format!("Failed to create config directory: {e}")))?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Parse(format!("Failed to serialize config: {e}")))?;
        std::fs::write(path, content)
            .map_err(|e| ConfigError::Read(format!("Failed to write config: {e}")))?;
        Ok(())
    }

    /// Upsert a named provider into `[[providers]]` (by name).
    pub fn upsert_provider(&mut self, provider: NamedProviderConfig) {
        let name = provider.name.clone();
        if let Some(existing) = self.providers.iter_mut().find(|p| p.name == name) {
            *existing = provider;
        } else {
            self.providers.push(provider);
        }
        // Keep legacy blocks in sync for openai/anthropic so env-less reloads stay consistent.
        if name == "openai" {
            if let Some(p) = self.providers.iter().find(|p| p.name == "openai") {
                self.openai.api_key = p.api_key.clone();
                self.openai.base_url = p.base_url.clone();
            }
        } else if name == "anthropic" {
            if let Some(p) = self.providers.iter().find(|p| p.name == "anthropic") {
                self.anthropic.api_key = p.api_key.clone();
                self.anthropic.base_url = p.base_url.clone();
            }
        }
    }

    /// Upsert a catalog model into `[[models]]` (by local name).
    pub fn upsert_model(&mut self, model: ModelConfig) {
        if let Some(existing) = self.models.iter_mut().find(|m| m.name == model.name) {
            *existing = model;
        } else {
            self.models.push(model);
        }
    }

    /// Set the default selected model name and persist-ready field.
    pub fn set_default_model(&mut self, name: impl Into<String>) {
        self.model = name.into();
    }

    /// Ensure the config directory exists and create a default config if none exists.
    pub fn ensure_config_dir() -> Result<(), ConfigError> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError::Read(format!("Failed to create config directory: {e}")))?;
        }
        let sessions_dir = Self::sessions_dir();
        std::fs::create_dir_all(&sessions_dir)
            .map_err(|e| ConfigError::Read(format!("Failed to create sessions directory: {e}")))?;
        let logs_dir = Self::logs_dir();
        std::fs::create_dir_all(&logs_dir)
            .map_err(|e| ConfigError::Read(format!("Failed to create logs directory: {e}")))?;
        Ok(())
    }
    /// Built-in default context window guesses by model id pattern.
    pub fn default_context_window_for(model_id: &str) -> u64 {
        if model_id.to_ascii_lowercase().starts_with("claude") {
            200_000
        } else {
            // OpenAI-family and unknown/third-party ids default to 128k.
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
    ///
    /// If the selection ends with `-pig`, the suffix is stripped and the
    /// underlying model is resolved. The `ResolvedModel.name` preserves the
    /// `-pig` suffix so callers know this is a phased (wrapped) model.
    pub fn resolve_model(&self, selection: &str) -> Result<ResolvedModel, ConfigError> {
        let selection = selection.trim();
        if selection.is_empty() {
            return Err(ConfigError::Invalid("model selection is empty".into()));
        }

        // Detect and strip `-pig` suffix: this model goes through the phased runtime.
        let (base_selection, is_pig) = if let Some(stripped) = selection.strip_suffix("-pig") {
            (stripped.to_string(), true)
        } else {
            (selection.to_string(), false)
        };

        let mut resolved = self.resolve_model_inner(&base_selection)?;
        if is_pig {
            resolved.name = format!("{}-pig", resolved.name);
            resolved.is_pig = true;
        }
        Ok(resolved)
    }

    fn resolve_model_inner(&self, selection: &str) -> Result<ResolvedModel, ConfigError> {
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
                    providers.iter().find(|p| {
                        let api = p.api.to_ascii_lowercase();
                        api == "openai"
                            || api == "responses"
                            || api == "openai-chat"
                            || api == "chat"
                            || api == "completions"
                            || api == "openai-compatible"
                    })
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
            ApiFormat::OpenAI | ApiFormat::OpenAIChat => "https://api.openai.com/v1",
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
            is_pig: false,
        })
    }

}



#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.permission_mode, "workspace_write");
        assert_eq!(config.max_turns, 50);
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn test_permission_mode_parsed() {
        let config = AppConfig::default();
        let mode = config.permission_mode_parsed().unwrap();
        assert_eq!(mode, PermissionMode::WorkspaceWrite);
    }

    #[test]
    fn test_env_overrides() {
        std::env::set_var("PIGS_MODEL", "gpt-4o");
        std::env::set_var("PIGS_PERMISSION_MODE", "readonly");
        let config = AppConfig::default().with_env_overrides();
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.permission_mode, "readonly");
        std::env::remove_var("PIGS_MODEL");
        std::env::remove_var("PIGS_PERMISSION_MODE");
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = AppConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.model, deserialized.model);
        assert_eq!(config.max_turns, deserialized.max_turns);
    }

    #[test]
    fn test_merge_project_overrides() {
        let mut base = AppConfig {
            model: "gpt-4o".to_string(),
            mcp_servers: vec![McpServerConfigEntry {
                name: "base".into(),
                command: "echo".into(),
                args: vec![],
                env: Default::default(),
                enabled: true,
            }],
            ..AppConfig::default()
        };

        let project = AppConfig {
            model: "gpt-4o".to_string(),
            mcp_servers: vec![McpServerConfigEntry {
                name: "project".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "demo".into()],
                env: Default::default(),
                enabled: true,
            }],
            hooks: HooksConfig {
                pre_tool_use: vec![HookEntry {
                    matcher: "bash".into(),
                    command: "echo hi".into(),
                    timeout: 5,
                    enabled: true,
                }],
                ..HooksConfig::default()
            },
            ..AppConfig::default()
        };

        base.merge_project_overrides(project);
        assert_eq!(base.model, "gpt-4o");
        assert_eq!(base.mcp_servers.len(), 2);
        assert_eq!(base.hooks.pre_tool_use.len(), 1);
    }

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
}
