// 配置结构与加载
// Provider 级字段（api_key/models/model_map/max_retries 等）可被两种协议共用
// Endpoint 级字段覆盖 Provider 级同名字段

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub log: LogConfig,
    #[serde(default)]
    pub provider: Vec<Provider>,
    /// Compaction settings (context-window auto-compaction at the routing layer).
    #[serde(default)]
    pub compaction: CompactionConfig,

    // --- pigs 顶层字段（相位运行时用）/ pigs top-level fields ---
    /// UI / 回复语言：`zh`（默认）或 `en`。
    /// UI / reply language: zh (default) or en.
    ///
    /// Used by the phased runtime (`serve()`) to select prompt language.
    /// CLI-specific fields live in `pig.toml` (`AppConfig`).
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
    "zh".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
    // 清洗请求体中 content 为空/空白的 input(messages) 项，避免上游报错
    #[serde(default = "default_true")]
    pub clean_empty_content: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct LogConfig {
    #[serde(default = "default_level")]
    pub level: String,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_true")]
    pub to_stdout: bool,
    #[serde(default)]
    pub to_file: String,
    #[serde(default = "default_rotate_size")]
    pub rotate_size_mb: u64,
    #[serde(default = "default_rotate_keep")]
    pub rotate_keep: usize,
}

fn default_level() -> String {
    "info".into()
}
fn default_format() -> String {
    "pretty".into()
}
fn default_true() -> bool {
    true
}
fn default_rotate_size() -> u64 {
    50
}
fn default_rotate_keep() -> usize {
    7
}

// Provider 级共用字段（可选），Endpoint 级同名字段覆盖之
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderCommon {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Option<Vec<String>>,
    #[serde(default)]
    pub model_map: Option<HashMap<String, String>>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub retry_on_status: Option<Vec<StatusSpec>>,
    // 业务错误码重试：解析响应 body 中的 error.code 字段，命中则重试
    #[serde(default)]
    pub retry_on_code: Option<Vec<i64>>,
    #[serde(default)]
    pub key_mode: Option<KeyMode>,
    #[serde(default)]
    pub path_mode: Option<PathMode>,
    // 强制思考强度档位（如 "xhigh"/"max"/"high"）；"passthrough" 透传不修改；
    // 缺失则按协议取默认最高档（OpenAI/Responses=xhigh，Anthropic=max）
    #[serde(default)]
    pub thinking_effort: Option<String>,
    /// Default context window size in tokens for all models on this provider.
    /// Used by the routing-layer auto-compaction. Overridden by `context_windows` per-model.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Per-model context window sizes (in tokens).
    /// Key = model name (as in `models`), Value = context window size.
    /// Example: { "xopglm52" = 500000, "xopdeepseekv4pro" = 1000000 }
    #[serde(default)]
    pub context_windows: Option<HashMap<String, u64>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(flatten)]
    pub common: ProviderCommon,
    #[serde(default)]
    pub openai: Option<EndpointRaw>,
    #[serde(default)]
    pub anthropic: Option<EndpointRaw>,
    #[serde(default)]
    pub responses: Option<EndpointRaw>,
}

// 原始 endpoint 配置，字段全可选，缺失的从 provider 级取
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EndpointRaw {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
    pub model_map: Option<HashMap<String, String>>,
    pub max_retries: Option<u32>,
    pub retry_on_status: Option<Vec<StatusSpec>>,
    pub retry_on_code: Option<Vec<i64>>,
    pub key_mode: Option<KeyMode>,
    pub path_mode: Option<PathMode>,
    #[serde(default)]
    pub thinking_effort: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub context_windows: Option<HashMap<String, u64>>,
}

// 合并后的有效 endpoint
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
    pub key_mode: KeyMode,
    pub models: Vec<String>,
    pub model_map: HashMap<String, String>,
    pub retry_on_status: Vec<StatusSpec>,
    pub retry_on_code: Vec<i64>,
    pub max_retries: u32,
    pub path_mode: PathMode,
    pub thinking_effort: Option<String>,
    pub context_window: Option<u64>,
    pub context_windows: HashMap<String, u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    Override,
    Passthrough,
}

impl Default for KeyMode {
    fn default() -> Self {
        KeyMode::Passthrough
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PathMode {
    Append,
    Full,
}

impl Default for PathMode {
    fn default() -> Self {
        PathMode::Append
    }
}

// 支持单值 429 或范围字符串 "500-504"
// 用 serde_json::Value 接收再手动解析，避开 flatten + untagged 空数组的类型推断 bug
#[derive(Debug, Clone, Deserialize)]
pub struct StatusSpec(pub serde_json::Value);

impl StatusSpec {
    pub fn to_range(&self) -> Option<std::ops::RangeInclusive<u16>> {
        match &self.0 {
            serde_json::Value::Number(n) => n.as_u64().map(|c| c as u16..=c as u16),
            serde_json::Value::String(s) => {
                if let Some((a, b)) = s.split_once('-') {
                    let a = a.trim().parse::<u16>().ok()?;
                    let b = b.trim().parse::<u16>().ok()?;
                    Some(a..=b)
                } else {
                    s.trim().parse::<u16>().ok().map(|c| c..=c)
                }
            }
            _ => None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct StatusMatcher {
    ranges: Vec<std::ops::RangeInclusive<u16>>,
}

impl StatusMatcher {
    pub fn from_specs(specs: &[StatusSpec]) -> Self {
        let ranges: Vec<_> = specs.iter().filter_map(|s| s.to_range()).collect();
        StatusMatcher { ranges }
    }

    pub fn matches(&self, code: u16) -> bool {
        self.ranges.iter().any(|r| r.contains(&code))
    }
}

// 参考 new-api 默认重试状态码范围：
// 100-199, 300-399, 401-407, 409-499, 500-503, 505-523, 525-599
// 永远跳过 504 和 524（alwaysSkipRetryStatusCodes）
pub fn default_retry_specs() -> Vec<StatusSpec> {
    vec![
        StatusSpec(serde_json::Value::String("100-199".into())),
        StatusSpec(serde_json::Value::String("300-399".into())),
        StatusSpec(serde_json::Value::String("401-407".into())),
        StatusSpec(serde_json::Value::String("409-499".into())),
        StatusSpec(serde_json::Value::String("500-503".into())),
        StatusSpec(serde_json::Value::String("505-523".into())),
        StatusSpec(serde_json::Value::String("525-599".into())),
    ]
}

pub fn is_always_skip(code: u16) -> bool {
    matches!(code, 504 | 524)
}

// 讯飞 Coding Plan 默认可重试业务错误码（响应 body 中的 error.code 字段）
// 这些是临时性错误，可能随额度刷新/引擎恢复而成功：
//   10007 流量受限、10008 服务容量不足、10009 引擎连接失败、10010 引擎排队、
//   10012 引擎内部错误/排队、10110 服务忙、10222 引擎网络异常、10223 LB找不到引擎、
//   11200 授权/业务量超限、11201 次数超限、11202 秒级流控、11203 并发流控、11210 tpm超限、
//   11310 新错误类型
pub fn default_retry_codes() -> Vec<i64> {
    vec![
        10007, 10008, 10009, 10010, 10012, 10110, 10222, 10223, 11200, 11201, 11202, 11203, 11210,
        11310,
    ]
}

impl Provider {
    // 合并 provider 级与 endpoint 级配置，endpoint 级优先
    pub fn resolve_endpoint(&self, raw: &EndpointRaw) -> Option<Endpoint> {
        let base_url = raw.base_url.clone().or_else(|| None)?;
        let c = &self.common;
        Some(Endpoint {
            base_url,
            api_key: raw
                .api_key
                .clone()
                .or_else(|| c.api_key.clone())
                .unwrap_or_default(),
            key_mode: raw.key_mode.or(c.key_mode).unwrap_or_default(),
            models: raw
                .models
                .clone()
                .or_else(|| c.models.clone())
                .unwrap_or_default(),
            model_map: raw
                .model_map
                .clone()
                .or_else(|| c.model_map.clone())
                .unwrap_or_default(),
            retry_on_status: raw
                .retry_on_status
                .clone()
                .or_else(|| c.retry_on_status.clone())
                .unwrap_or_default(),
            retry_on_code: raw
                .retry_on_code
                .clone()
                .or_else(|| c.retry_on_code.clone())
                .unwrap_or_default(),
            max_retries: raw.max_retries.or(c.max_retries).unwrap_or(10000),
            path_mode: raw.path_mode.or(c.path_mode).unwrap_or_default(),
            thinking_effort: raw
                .thinking_effort
                .clone()
                .or_else(|| c.thinking_effort.clone()),
            context_window: raw
                .context_window
                .or(c.context_window),
            context_windows: raw
                .context_windows
                .clone()
                .or_else(|| c.context_windows.clone())
                .unwrap_or_default(),
        })
    }

    pub fn openai_endpoint(&self) -> Option<Endpoint> {
        self.openai.as_ref().and_then(|r| self.resolve_endpoint(r))
    }

    pub fn anthropic_endpoint(&self) -> Option<Endpoint> {
        self.anthropic
            .as_ref()
            .and_then(|r| self.resolve_endpoint(r))
    }

    pub fn responses_endpoint(&self) -> Option<Endpoint> {
        self.responses
            .as_ref()
            .and_then(|r| self.resolve_endpoint(r))
    }
}

impl Endpoint {
    pub fn status_matcher(&self) -> StatusMatcher {
        let specs = if self.retry_on_status.is_empty() {
            default_retry_specs()
        } else {
            self.retry_on_status.clone()
        };
        StatusMatcher::from_specs(&specs)
    }

    // 业务错误码列表：空则用默认（讯飞可重试码）
    pub fn retry_codes(&self) -> Vec<i64> {
        if self.retry_on_code.is_empty() {
            default_retry_codes()
        } else {
            self.retry_on_code.clone()
        }
    }

    // 模型名映射：未配置则原样返回
    pub fn map_model(&self, client_model: &str) -> String {
        self.model_map
            .get(client_model)
            .cloned()
            .unwrap_or_else(|| client_model.to_string())
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        toml::from_str(&content).with_context(|| "解析配置文件失败".to_string())
    }

    /// Load layered configuration:
    /// 1. `~/.pigs/config.toml` (global)
    /// 2. `{workspace}/.pigs/config.toml` (project overrides, if present)
    /// 3. `{workspace}/.pigs/config.local.toml` (machine-local overrides; gitignored)
    ///
    /// If `PIG_CONFIG` env var is set, it overrides the global path.
    /// If none of the files exist, returns defaults.
    pub fn load_layered(workspace: &Path) -> Result<Self> {
        // Global: ~/.pigs/config.toml (or PIG_CONFIG override)
        let global_path = std::env::var("PIG_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                home.join(".pigs").join("config.toml")
            });

        let mut config = if global_path.exists() {
            Self::load(&global_path)?
        } else {
            // Fallback: try root-level config.toml for backward compat
            let legacy = PathBuf::from("config.toml");
            if legacy.exists() {
                Self::load(&legacy)?
            } else {
                Config {
                    server: ServerConfig {
                        listen: "127.0.0.1:3927".to_string(),
                        clean_empty_content: true,
                    },
                    log: LogConfig {
                        level: default_level(),
                        format: default_format(),
                        to_stdout: true,
                        to_file: String::new(),
                        rotate_size_mb: default_rotate_size(),
                        rotate_keep: default_rotate_keep(),
                    },
                    provider: Vec::new(),
                    compaction: CompactionConfig::default(),
                    language: default_language(),
                }
            }
        };

        // Project: {workspace}/.pigs/config.toml
        let project_path = workspace.join(".pigs").join("config.toml");
        if project_path.exists() && project_path != global_path {
            let project = Self::load(&project_path)?;
            config.merge(project);
        }

        // Local: {workspace}/.pigs/config.local.toml (gitignored)
        let local_path = workspace.join(".pigs").join("config.local.toml");
        if local_path.exists() {
            let local = Self::load(&local_path)?;
            config.merge(local);
        }

        Ok(config)
    }

    /// Merge another config into self (other takes precedence for non-empty/non-default values).
    pub fn merge(&mut self, other: Config) {
        // Server: override if listen is non-empty
        if !other.server.listen.is_empty() {
            self.server.listen = other.server.listen;
        }
        self.server.clean_empty_content = other.server.clean_empty_content;

        // Log: override non-default fields
        if other.log.level != default_level() {
            self.log.level = other.log.level;
        }
        if other.log.format != default_format() {
            self.log.format = other.log.format;
        }
        self.log.to_stdout = other.log.to_stdout;
        if !other.log.to_file.is_empty() {
            self.log.to_file = other.log.to_file;
        }

        // Compaction: override if other has non-default values
        if other.compaction.coefficient != default_coefficient() {
            self.compaction.coefficient = other.compaction.coefficient;
        }
        if other.compaction.keep_recent != default_keep_recent() {
            self.compaction.keep_recent = other.compaction.keep_recent;
        }
        if other.compaction.max_rounds != default_max_rounds() {
            self.compaction.max_rounds = other.compaction.max_rounds;
        }
        if other.compaction.summary_max_tokens != default_summary_max_tokens() {
            self.compaction.summary_max_tokens = other.compaction.summary_max_tokens;
        }
        self.compaction.enabled = other.compaction.enabled;

        // Providers: merge by name (extend, don't replace)
        for provider in other.provider {
            if !self.provider.iter().any(|p| p.name == provider.name) {
                self.provider.push(provider);
            }
        }

        // Language
        if other.language != default_language() {
            self.language = other.language;
        }
    }
}

/// Compaction configuration for the routing-layer auto-compaction.
#[derive(Debug, Clone, Deserialize)]
pub struct CompactionConfig {
    /// Whether auto-compaction is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Trigger threshold coefficient (e.g. 0.9 = compact when estimated tokens exceed 90% of context window).
    #[serde(default = "default_coefficient")]
    pub coefficient: f64,
    /// Number of recent messages to keep verbatim during compaction.
    #[serde(default = "default_keep_recent")]
    pub keep_recent: usize,
    /// Maximum compaction rounds before forcing a fallback truncation.
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    /// Max tokens for the summarization LLM call.
    #[serde(default = "default_summary_max_tokens")]
    pub summary_max_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            coefficient: default_coefficient(),
            keep_recent: default_keep_recent(),
            max_rounds: default_max_rounds(),
            summary_max_tokens: default_summary_max_tokens(),
        }
    }
}

fn default_coefficient() -> f64 {
    0.9
}
fn default_keep_recent() -> usize {
    4
}
fn default_max_rounds() -> u32 {
    5
}
fn default_summary_max_tokens() -> u32 {
    4096
}

/// Infer a default context window from the model name.
/// claude* → 200_000, gpt-4* → 128_000, others → 128_000.
pub fn default_context_window_for(model: &str) -> u64 {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude") {
        200_000
    } else if lower.starts_with("gpt-4") || lower.starts_with("o1") || lower.starts_with("o3") {
        128_000
    } else {
        128_000
    }
}
