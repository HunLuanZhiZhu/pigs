//! 相位提示词 —— 从 `pigs_prompts` re-export。
//! Phase prompts — re-exported from `pigs_prompts`.
//!
//! 提示词模板已外置为纯文本文件，放在 `crates/pigs-prompts/prompts/`，
//! 编译时用 `include_str!` 嵌入。本模块仅做 re-export，
//! 现有调用方无需改动。
//! Prompt templates live as plain-text files in
//! `crates/pigs-prompts/prompts/`, compiled in via `include_str!`.
//! This module re-exports them so existing call sites work unchanged.
//!
//! 三个 re-exported 函数 / three re-exported functions:
//! - `pre_user_payload`：Pre 相位 user 消息（含失败路径）/ Pre-phase user message (with failure paths).
//! - `executor_user_payload`：Executor 相位 user 消息（含 Pre 产物 + Post 反馈）/ Executor-phase user message (with Pre output + Post feedback).
//! - `post_user_payload`：Post 相位 user 消息（含 Pre 产物 + Executor 草稿）/ Post-phase user message (with Pre output + Executor draft).

// 从 pigs_prompts re-export三个相位 user payload 构造函数。
// Re-export the three phase user-payload builders from pigs_prompts.
pub use pigs_prompts::{executor_user_payload, post_user_payload, pre_user_payload};
