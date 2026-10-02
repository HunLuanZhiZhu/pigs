//! pigs —— 可执行入口。电源开关：解析参数 → 读配置 → 起服务。
//! 红线：这里不出现任何业务逻辑。

mod logging;

use anyhow::{bail, Result};

const CONFIG_PATH: &str = "config.toml";
const LOCAL_CONFIG_PATH: &str = "config.local.toml";

fn resolve_existing_config_path(local_exists: bool, default_exists: bool) -> Option<&'static str> {
    if local_exists {
        Some(LOCAL_CONFIG_PATH)
    } else if default_exists {
        Some(CONFIG_PATH)
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("pigs —— 只做编排的前置代理\n");
        println!("用法:");
        println!("  pigs                        运行服务（优先读 ./config.local.toml，否则 ./config.toml；都缺失时生成 config.toml）");
        println!("  pigs --listen 127.0.0.1:3927     覆盖监听地址");
        println!("  pigs --base-url http://…         覆盖上游地址（mini-proxy 或任意上游 API）");
        println!("  pigs --log-detail off|basic|max  覆盖 HTTP 诊断日志详细程度");
        println!("  pigs --log-dir PATH              覆盖 HTTP 诊断日志目录");
        println!("  pigs --example              导出配置模板到 config.example.toml");
        println!("  pigs -h | --help            显示此帮助\n");
        println!("model 带 -pigs 后缀的请求走 Pre→Executor→Post 编排，其余原样透传。");
        return Ok(());
    }

    if args.iter().any(|a| a == "--example") {
        std::fs::write("config.example.toml", pigs_proxy::config_template())?;
        println!("已导出配置模板到 config.example.toml");
        return Ok(());
    }

    // 本地私有配置优先；都不存在时生成公开默认配置并继续启动。
    let existing_config_path = resolve_existing_config_path(
        std::path::Path::new(LOCAL_CONFIG_PATH).exists(),
        std::path::Path::new(CONFIG_PATH).exists(),
    );
    let (mut config, active_config_path) = if let Some(path) = existing_config_path {
        if path == LOCAL_CONFIG_PATH {
            println!("检测到 {LOCAL_CONFIG_PATH}，优先使用本地配置。");
        }
        (pigs_proxy::Config::load(path)?, path)
    } else {
        println!("未发现 {LOCAL_CONFIG_PATH} 或 {CONFIG_PATH}，已生成默认 {CONFIG_PATH}。");
        std::fs::write(CONFIG_PATH, pigs_proxy::config_template())?;
        (pigs_proxy::Config::load(CONFIG_PATH)?, CONFIG_PATH)
    };

    // CLI 覆盖
    let pick = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(listen) = pick("--listen") {
        config.listen = listen;
    }
    if let Some(base_url) = pick("--base-url") {
        config.upstream = pigs_proxy::Upstreams::same(&base_url);
    }
    if let Some(detail) = pick("--log-detail") {
        config.logging.detail = detail.parse()?;
    }
    if let Some(directory) = pick("--log-dir") {
        config.logging.directory = directory;
    }
    if config.upstream.openai.is_empty()
        || config.upstream.responses.is_empty()
        || config.upstream.anthropic.is_empty()
    {
        bail!(
            "upstream 三个协议的地址都要配置：请编辑 {active_config_path} 或用 --base-url 统一覆盖"
        );
    }

    // 日志：控制台 + logs/pigs.log.<日期>，RUST_LOG 可调级别
    let _log_guard = logging::init()?;
    tracing::info!(
        listen = %config.listen,
        openai = %config.upstream.openai,
        responses = %config.upstream.responses,
        anthropic = %config.upstream.anthropic,
        "pigs 启动"
    );

    pigs_proxy::serve(config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_config_has_priority() {
        assert_eq!(
            resolve_existing_config_path(true, true),
            Some(LOCAL_CONFIG_PATH)
        );
    }

    #[test]
    fn falls_back_to_default_config() {
        assert_eq!(resolve_existing_config_path(false, true), Some(CONFIG_PATH));
    }

    #[test]
    fn reports_no_existing_config_when_both_are_missing() {
        assert_eq!(resolve_existing_config_path(false, false), None);
    }
}
