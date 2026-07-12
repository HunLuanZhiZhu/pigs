//! Session persistence — JSONL-based session storage with compaction.

pub mod compact;
pub mod session;

pub use compact::{compact_session, CompactConfig};
pub use session::{Session, SessionError, SessionMetadata};
