# pigs

可执行入口（bin）。目标体量：**几十行**。

## 计划内容

- CLI 参数：`--listen` / `--base-url`（覆盖 config.toml）、`--example`（导出配置模板）、`--help`
- 加载 `config.toml`（缺省生成后退出，同 mini-proxy 习惯）
- 初始化日志（`logs/pigs.log`，参数写死）
- 调用 `pigs_proxy::serve(config)` 并阻塞

## 不放什么

任何业务逻辑。本 crate 出现第 100 行代码就是拆分信号。
