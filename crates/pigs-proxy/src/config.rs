//! 配置：只有部署位置需要配置。模板即实际配置（include_str! 指向仓库根 config.toml）。

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
    /// 三个协议各一个上游地址：base + 客户端原样路径 = 上游 URL。
    pub upstream: Upstreams,
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
        Ok(toml::from_str(&raw)?)
    }

    /// 默认配置（首次运行生成用）。
    pub fn example() -> &'static str {
        EXAMPLE_CONFIG
    }
}
