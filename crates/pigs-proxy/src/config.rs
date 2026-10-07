//! 配置：部署位置 + HTTP 诊断日志。模板即实际配置（include_str! 指向仓库根 config.toml）。

use serde::{Deserialize, Serialize};

/// 嵌入模板 = 仓库根的 config.toml 本体，`--example` 可导出副本。
pub const EXAMPLE_CONFIG: &str = include_str!("../../../config.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 监听地址。
    pub listen: String,
    /// 留空 = 透传客户端 key；填了则覆盖。
    #[serde(default)]
    pub key: String,
    /// HTTP 诊断日志。内部测试阶段默认 max。
    #[serde(default)]
    pub logging: LoggingConfig,
    /// PIGS 编排次数控制。
    #[serde(default)]
    pub orchestration: OrchestrationConfig,
    /// 三个协议各一个上游地址：base + 客户端原样路径 = 上游 URL。
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
    /// off = 不生成 HTTP 明文抓包；basic = 只记元数据；max = 元数据 + 完整 body。
    #[serde(default)]
    pub detail: LogDetail,
    /// HTTP 抓包文件目录。
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
    /// 返回 total_tokens 最大的一次真实上游调用的完整 usage，供 coding agent 判断上下文占用。
    Max,
    /// 将本次客户端请求触发的真实上游调用 usage 数值字段逐项相加，供评测统计真实消耗。
    Sum,
}

impl Default for UsageMode {
    fn default() -> Self {
        Self::Max
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationConfig {
    /// 一轮任务最多允许多少次高层 Executor 执行；工具暂停/恢复不重复计数。
    #[serde(default = "default_max_executor_runs")]
    pub max_executor_runs: u32,
    /// Post 没有输出控制标记时，最多额外重试多少次协议响应。
    #[serde(default = "default_max_post_protocol_retries")]
    pub max_post_protocol_retries: u32,
    /// 最终向下游汇报 usage 的语义：max（日常 coding agent）或 sum（评测真实消耗）。
    #[serde(default)]
    pub usage_mode: UsageMode,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            max_executor_runs: default_max_executor_runs(),
            max_post_protocol_retries: default_max_post_protocol_retries(),
            usage_mode: UsageMode::Max,
        }
    }
}

fn default_max_executor_runs() -> u32 {
    pigs_orchestrator::DEFAULT_MAX_EXECUTOR_RUNS
}

fn default_max_post_protocol_retries() -> u32 {
    pigs_orchestrator::DEFAULT_MAX_POST_PROTOCOL_RETRIES
}

/// 三协议各自的上游前缀。
///
/// 路径约定：chat `/chat/completions`、responses `/responses` 不带版本段
/// （版本属于 base）；anthropic `/v1/messages` 自带 `/v1`（版本属于协议，
/// base 不带）——A 社的 v1 是协议的一部分，对其他协议不是。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upstreams {
    /// OpenAI Chat 上游（接 /chat/completions）。
    pub openai: String,
    /// Responses 上游（接 /responses）。
    pub responses: String,
    /// Anthropic 上游（接 /v1/messages）。
    pub anthropic: String,
}

impl Upstreams {
    /// `--base-url` 统一覆盖用：三个协议指到同一个前缀。
    pub fn same(base_url: &str) -> Self {
        Self {
            openai: base_url.to_string(),
            responses: base_url.to_string(),
            anthropic: base_url.to_string(),
        }
    }
}

impl Config {
    /// 从当前目录的 config.toml 加载。
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&raw)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.orchestration.max_executor_runs == 0 {
            anyhow::bail!("orchestration.max_executor_runs 必须大于 0");
        }
        Ok(())
    }

    /// 默认配置（首次运行生成用）。
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
                max_executor_runs: pigs_orchestrator::DEFAULT_MAX_EXECUTOR_RUNS,
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
max_executor_runs = 6
max_post_protocol_retries = 1
usage_mode = "sum"
[upstream]
openai = "http://a"
responses = "http://b"
anthropic = "http://c"
"#,
        )
        .unwrap();
        assert_eq!(config.orchestration.max_executor_runs, 6);
        assert_eq!(config.orchestration.max_post_protocol_retries, 1);
        assert_eq!(config.orchestration.usage_mode, UsageMode::Sum);
    }

    #[test]
    fn logging_detail_parses_all_supported_values() {
        assert_eq!("off".parse::<LogDetail>().unwrap(), LogDetail::Off);
        assert_eq!("basic".parse::<LogDetail>().unwrap(), LogDetail::Basic);
        assert_eq!("max".parse::<LogDetail>().unwrap(), LogDetail::Max);
        assert!("verbose".parse::<LogDetail>().is_err());
    }
}
