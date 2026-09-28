//! Permission system — modes, policies, and interactive prompters.
//!
//! The permission model controls which tools the agent can execute and under
//! what conditions. Permission modes are ordered by escalation level, allowing
//! simple comparison (`active_mode >= required_mode`).

pub mod mode;
pub mod policy;
pub mod prompter;

pub use mode::PermissionMode;
pub use policy::{PermissionOutcome, PermissionPolicy, PermissionRequest};
pub use prompter::{CliPermissionPrompter, PermissionDecision, PermissionPrompter};
