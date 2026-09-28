//! Configuration management — TOML config file, environment variables, and AGENTS.md parsing.

pub mod agents_md;
pub mod config;
pub mod language;
pub mod memory;
pub mod rules;
pub mod skills;
pub mod sub_agent_def;

pub use agents_md::{
    build_system_prompt, build_system_prompt_from_dir, build_system_prompt_with_source,
    load_agents_md, load_project_context, ProjectContext, PROJECT_CONTEXT_FILENAMES,
};
pub use config::{
    ApiFormat, AppConfig, HookEntry, HooksConfig, McpServerConfigEntry, ModelConfig,
    NamedProviderConfig, ProviderConfig, ResolvedModel,
};
pub use language::Language;
pub use memory::{
    add_memory_note, format_memory_for_prompt, load_memory, remove_memory_notes, MemorySource,
    MemoryStore,
};
pub use rules::{format_rules_for_prompt, load_rules, RuleDoc};
pub use skills::{
    find_skill, format_skill_body, format_skills_for_prompt, load_skills, skill_search_dirs, Skill,
};
