//! Pigs API — 相位化 Agent HTTP 服务器 + 相位运行时模块。
//! Pigs API — phased agent HTTP server + phased runtime modules.
//!
//! 本 crate 包含：
//! This crate contains:
//! - `server` — OpenAI 兼容的本地 HTTP API（/health, /v1/models, /v1/chat/completions）
//!   OpenAI-compatible local HTTP API
//! - `phased_runtime` — Pre→Executor→Post 三相位 Agent 运行时
//!   Pre→Executor→Post phased agent runtime
//! - `phased_api_convert` — OpenAI 请求格式 → 相位运行时输入的转换层
//!   OpenAI request → phased runtime conversion
//! - `phased_markers` — 控制标记 PIGEND / PIGFAILED 的检测与清理
//!   Control marker PIGEND / PIGFAILED detection and stripping
//! - `phased_phase` — Phase 枚举
//!   Phase enum
//! - `phased_tools` — 相位运行时的工具注册表（复用 pigs-tools 全量工具）
//!   Tool registry for phased runtime (reuses pigs-tools)
//! - `phased_prompts` — re-export pigs_prompts 的提示词函数
//!   Re-export of pigs_prompts prompt functions

/// OpenAI 兼容的本地 HTTP API 服务器。
/// OpenAI-compatible local HTTP API server.
pub mod server;

/// OpenAI 请求 → 相位运行时输入的转换层。
/// OpenAI request → phased runtime conversion layer.
pub mod phased_api_convert;

/// 相位化 Agent 运行时（Pre→Executor→Post + 标记路由）。
/// Phased agent runtime (Pre→Executor→Post + marker routing).
pub mod phased_runtime;

/// Phase 枚举（Pre / Executor / Post）。
/// Phase enum (Pre / Executor / Post).
pub mod phased_phase;

/// 控制标记 PIGEND / PIGFAILED 的检测与清理。
/// Control marker PIGEND / PIGFAILED detection and stripping.
pub mod phased_markers;

/// 相位运行时的工具注册表。
/// Tool registry for the phased runtime.
pub mod phased_tools;

/// 提示词 re-export（从 pigs_prompts）。
/// Prompt re-export (from pigs_prompts).
pub mod phased_prompts;
