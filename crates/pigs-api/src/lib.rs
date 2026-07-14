//! Pigs API — 相位化 Agent 运行时模块库。
//! Pigs API — phased agent runtime modules.
//!
//! 本 crate 包含：
//! This crate contains:
//! - `phased_runtime` — Pre→Executor→Post 三相位 Agent 运行时
//!   Pre→Executor→Post phased agent runtime
//! - `phased_api_convert` — OpenAI 请求格式 → 相位运行时输入的转换层
//!   OpenAI request → phased runtime conversion
//! - `format` — 三种 API 格式的请求解析与响应构造
//!   Request parsing / response construction for three API formats
//! - `phased_markers` — 控制标记 PIGEND / PIGFAIL 的检测与清理
//!   Control marker PIGEND / PIGFAIL detection and stripping
//! - `phased_phase` — Phase 枚举
//!   Phase enum
//! - `phased_tools` — 相位运行时的工具注册表（复用 pigs-tools）
//!   Tool registry for phased runtime (reuses pigs-tools)
//! - `phased_prompts` — re-export pigs_prompts 的提示词函数
//!   Re-export of pigs_prompts prompt functions
//!
//! HTTP 服务器已移至 `pigs-proxy` crate。
//! HTTP server has been moved to the `pigs-proxy` crate.

/// 三种 API 格式的请求解析与响应构造。
/// Request parsing and response construction for three API formats.
pub mod format;

/// OpenAI 请求 → 相位运行时输入的转换层。
/// Protocol-native request preservation, phase mutation, and output extraction.
pub mod protocol;

/// OpenAI request → phased runtime conversion layer.
pub mod phased_api_convert;

/// 相位化 Agent 运行时（Pre→Executor→Post + 标记路由）。
/// Phased agent runtime (Pre→Executor→Post + marker routing).
pub mod phased_runtime;

/// Protocol-native JSON and SSE response encoding.
pub mod output;

/// Protocol-native HTTP phase runtime.
pub mod http_runtime;

/// Bounded in-memory external-tool continuations.
pub mod continuation;

/// Transport abstraction used by the HTTP phase runtime.
pub mod transport;

/// Pure Pre -> Executor -> Post state transitions.
pub mod orchestration;

/// Phase 枚举（Pre / Executor / Post）。
/// Phase enum (Pre / Executor / Post).
pub mod phased_phase;

/// 控制标记 PIGEND / PIGFAIL 的检测与清理。
/// Control marker PIGEND / PIGFAIL detection and stripping.
pub mod phased_markers;

/// 相位运行时的工具注册表。
/// Tool registry for the phased runtime.
pub mod phased_tools;

/// 提示词 re-export（从 pigs_prompts）。
/// Prompt re-export (from pigs_prompts).
pub mod phased_prompts;
