//! Session persistence — JSONL-based session storage with compaction.

pub mod compact;
pub mod session;

pub use compact::{
    compact_session, compact_session_truncate, needs_compaction, Compactor, CompactConfig,
    CompactionError,
};
pub use session::{Session, SessionError, SessionMetadata};
