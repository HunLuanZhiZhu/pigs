//! pigs —— 可执行入口。电源开关：解析参数 → 读配置 → 起服务。
//! 红线：这里不出现任何业务逻辑。

mod logging;

use anyhow::{bail, Result};

const CONFIG_PATH: &str = "config.toml";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("pigs —— 只做编排的前置代理\n");
        println!("用法:");
        println!("  pigs                        运行服务（读 ./config.toml，缺省自动生成并启动）");
        println!("  pigs --listen 127.0.0.1:3927     覆盖监听地址");
        println!("  pigs --base-url http://…         覆盖上游地址（mini-proxy 或任意上游 API）");
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

    // 加载配置；无文件则生成默认配置并继续启动（passthrough 无需 key，开箱即用）
    let mut config = if std::path::Path::new(CONFIG_PATH).exists() {
        pigs_proxy::Config::load(CONFIG_PATH)?
    } else {
        println!("未发现 {CONFIG_PATH}，已生成默认配置。");
        std::fs::write(CONFIG_PATH, pigs_proxy::config_template())?;
        pigs_proxy::Config::load(CONFIG_PATH)?
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
    if config.upstream.openai.is_empty()
        || config.upstream.responses.is_empty()
        || config.upstream.anthropic.is_empty()
    {
        bail!("upstream 三个协议的地址都要配置：请编辑 {CONFIG_PATH} 或用 --base-url 统一覆盖");
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
