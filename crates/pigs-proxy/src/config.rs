//! 配置：只有部署位置需要配置。模板即实际配置（include_str! 指向仓库根 config.toml）。

use serde::{Deserialize, Serialize};

/// 嵌入模板 = 仓库根的 config.toml 本体，`--example` 可导出副本。
pub const EXAMPLE_CONFIG: &str = include_str!("../../../config.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 监听地址。
    pub listen: String,
    /// 上游地址：mini-proxy 或任意直连上游 API。
    pub base_url: String,
    /// 留空 = 透传客户端 key；填了则覆盖。
    #[serde(default)]
    pub key: String,
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
