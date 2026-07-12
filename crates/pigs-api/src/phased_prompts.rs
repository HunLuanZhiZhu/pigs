//! 相位提示词 —— 从 `pigs_prompts` re-export。
//! Phase prompts — re-exported from `pigs_prompts`.
//!
//! 提示词模板已外置为纯文本文件，放在 `crates/pigs-prompts/prompts/`，
//! 编译时用 `include_str!` 嵌入。本模块仅做 re-export，
//! 现有调用方无需改动。
//! Prompt templates live as plain-text files in
//! `crates/pigs-prompts/prompts/`, compiled in via `include_str!`.
//! This module re-exports them so existing call sites work unchanged.

pub use pigs_prompts::{
    executor_prompt as executor_system_prompt, executor_user_payload,
    post_prompt as post_system_prompt, post_user_payload,
    pre_prompt as pre_system_prompt, pre_user_payload,
};
