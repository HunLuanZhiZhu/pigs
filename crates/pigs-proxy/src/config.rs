//! 配置：部署、编排与 HTTP 诊断；嵌入根目录模板。 / Deployment, orchestration, and HTTP diagnostics configuration; embeds the root template.

use serde::{Deserialize, Serialize};

/// 嵌入根目录配置模板，供 `--example` 导出。 / Embed the root configuration template for `--example` export.
pub const EXAMPLE_CONFIG: &str = include_str!("../../../config.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 监听地址。 / Listen address.
    pub listen: String,
    /// 空值透传客户端密钥，非空值覆盖。 / Empty preserves the client key; nonempty overrides it.
    #[serde(default)]
    pub key: String,
    /// HTTP 诊断日志，默认详细级别为 max。 / HTTP diagnostic logging, defaulting to max detail.
    #[serde(default)]
    pub logging: LoggingConfig,
    /// PIGS 编排次数限制。 / PIGS orchestration limits.
    #[serde(default)]
    pub orchestration: OrchestrationConfig,
    /// 每种协议独立配置上游基础地址。 / Separate upstream base URL for each protocol.
    pub upstream: Upstreams,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogDetail {
    Off,
    Basic,
    Max,
}

impl Default for LogDetail {
    fn default() -> Self {
        Self::Max
    }
}

impl std::str::FromStr for LogDetail {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "basic" => Ok(Self::Basic),
            "max" => Ok(Self::Max),
            other => anyhow::bail!("未知日志详细程度 {other:?}；可选 off / basic / max"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// off 不记录；basic 记录元数据；max 记录元数据及完整请求体。 / off disables capture; basic logs metadata; max logs metadata and full bodies.
    #[serde(default)]
    pub detail: LogDetail,
    /// HTTP 抓包文件目录。 / HTTP capture directory.
    #[serde(default = "default_http_log_dir")]
    pub directory: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            detail: LogDetail::Max,
            directory: default_http_log_dir(),
        }
    }
}

fn default_http_log_dir() -> String {
    "logs/http".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageMode {
    /// 返回 token 最多的上游调用的完整 usage，用于上下文估计。 / Return the full usage of the largest upstream call for context-size estimation.
    Max,
    /// 汇总本次请求全部上游调用的 usage 数值，供评测计费。 / Sum numeric usage fields across all upstream calls for evaluation.
    Sum,
}

impl Default for UsageMode {
    fn default() -> Self {
        Self::Max
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationConfig {
    /// 每轮高层 Exec 执行上限；工具暂停/恢复不重复计数。 / Max high-level Exec runs per turn, excluding tool pause/resume.
    #[serde(default = "default_max_exec_runs", alias = "max_executor_runs")]
    pub max_exec_runs: u32,
    /// Post 缺少控制标记时的协议重试次数上限。 / Max retries when Post omits a control marker.
    #[serde(default = "default_max_post_protocol_retries")]
    pub max_post_protocol_retries: u32,
    /// 下游 usage 汇报模式：max 用于日常，sum 用于评测。 / Downstream usage mode: max for daily agents, sum for evaluation.
    #[serde(default)]
    pub usage_mode: UsageMode,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            max_exec_runs: default_max_exec_runs(),
            max_post_protocol_retries: default_max_post_protocol_retries(),
            usage_mode: UsageMode::Max,
        }
    }
}

fn default_max_exec_runs() -> u32 {
    pigs_orchestrator::DEFAULT_MAX_EXEC_RUNS
}

fn default_max_post_protocol_retries() -> u32 {
    pigs_orchestrator::DEFAULT_MAX_POST_PROTOCOL_RETRIES
}

/// 三种协议各自的上游前缀。 / Upstream base URLs for the three protocols.
///
/// 路径约定：Chat 与 Responses 的路径不含版本段。 / Chat and Responses paths omit a version component.
/// 版本属于基础地址；Anthropic `/v1/messages` 自带版本段。 / Their version belongs to the base URL; Anthropic includes `/v1` in its path.
/// Anthropic 的基础地址不含该版本段。 / The Anthropic base URL excludes that version component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upstreams {
    /// OpenAI Chat 上游地址。 / Upstream base URL for OpenAI Chat.
    pub openai: String,
    /// Responses 上游地址。 / Upstream base URL for Responses.
    pub responses: String,
    /// Anthropic 上游地址。 / Upstream base URL for Anthropic.
    pub anthropic: String,
}

impl Upstreams {
    /// `--base-url` 覆盖：三种协议共用一个基础地址。 / `--base-url` override: all three protocols share one base URL.
    pub fn same(base_url: &str) -> Self {
        Self {
            openai: base_url.to_string(),
            responses: base_url.to_string(),
            anthropic: base_url.to_string(),
        }
    }
}

impl Config {
    /// 从指定配置文件加载。 / Load from the specified configuration file.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&raw)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.orchestration.max_exec_runs == 0 {
            anyhow::bail!("orchestration.max_exec_runs 必须大于 0");
        }
        Ok(())
    }

    /// 默认配置模板，供首次运行生成。 / Default template for initial configuration generation.
    pub fn example() -> &'static str {
        EXAMPLE_CONFIG
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_defaults_to_max_when_section_is_absent() {
        let config: Config = toml::from_str(
            r#"
listen = "127.0.0.1:3927"
key = ""
[upstream]
openai = "http://a"
responses = "http://b"
anthropic = "http://c"
"#,
        )
        .unwrap();
        assert_eq!(config.logging.detail, LogDetail::Max);
        assert_eq!(config.logging.directory, "logs/http");
        assert_eq!(
            config.orchestration,
            OrchestrationConfig {
                max_exec_runs: pigs_orchestrator::DEFAULT_MAX_EXEC_RUNS,
                max_post_protocol_retries:
                    pigs_orchestrator::DEFAULT_MAX_POST_PROTOCOL_RETRIES,
                usage_mode: UsageMode::Max,
            }
        );
    }

    #[test]
    fn orchestration_values_are_configurable() {
        let config: Config = toml::from_str(
            r#"
listen = "127.0.0.1:3927"
key = ""
[orchestration]
max_exec_runs = 6
max_post_protocol_retries = 1
usage_mode = "sum"
[upstream]
openai = "http://a"
responses = "http://b"
anthropic = "http://c"
"#,
        )
        .unwrap();
        assert_eq!(config.orchestration.max_exec_runs, 6);
        assert_eq!(config.orchestration.max_post_protocol_retries, 1);
        assert_eq!(config.orchestration.usage_mode, UsageMode::Sum);
    }

    #[test]
    fn legacy_max_executor_runs_key_is_still_accepted() {
        let legacy: OrchestrationConfig = toml::from_str("max_executor_runs = 7").unwrap();
        assert_eq!(legacy.max_exec_runs, 7);

        let modern: OrchestrationConfig = toml::from_str("max_exec_runs = 7").unwrap();
        assert_eq!(modern.max_exec_runs, 7);

        let serialized = toml::to_string(&legacy).unwrap();
        assert!(serialized.contains("max_exec_runs = 7"));
        assert!(!serialized.contains("max_executor_runs"));
    }

    #[test]
    fn logging_detail_parses_all_supported_values() {
        assert_eq!("off".parse::<LogDetail>().unwrap(), LogDetail::Off);
        assert_eq!("basic".parse::<LogDetail>().unwrap(), LogDetail::Basic);
        assert_eq!("max".parse::<LogDetail>().unwrap(), LogDetail::Max);
        assert!("verbose".parse::<LogDetail>().is_err());
    }
}
