//! pigs-protocol：三种协议的共享类型与操作。 / Shared types and operations for three API protocols.
//!
//! 底层协议模块：路径路由、模型后缀、请求体修改、响应解析与合成。 / Low-level protocol layer: routing, model suffixes, request rewriting, response parsing, and synthesis.
//! 提供 JSON 与 SSE 的处理能力。 / Supports JSON and SSE payloads.
//! 不执行 HTTP 通信。 / Does not perform HTTP I/O.

pub mod output;
pub mod response;
pub mod route;
pub mod sse;
pub mod surgery;

pub use output::{
    all_tool_result_ids, continuation_resume_items, model_output_transcript_items,
    parse_json_output, parse_sse_output, trailing_tool_result_ids, trailing_tool_result_items,
    ModelOutput, Part, ToolCall,
};
pub use response::{
    extract_response_text, extract_sse_text, synthesize_json, synthesize_sse, LiveEvent,
    ResponseContent, SseTextStream, StreamEncoder,
};
pub use route::{has_pigs, parse_pigs_model, protocol_from_path, strip_pigs_suffix, PigsMode, Protocol};
pub use sse::is_sse_content_type;
pub use surgery::{
    append_instruction, append_to_last_user_text, append_transcript_items, extract_last_user_text,
    get_model, has_client_stream, push_assistant_message, push_user_message, set_model, set_stream,
    strip_tools, SUFFIX_SEPARATOR,
};

/// 编解码错误。 / Codec error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("请求 body 不是合法 JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),
    #[error("上游响应不是合法 JSON（content-type={content_type}）: {reason}；body 前 300 字符: {snippet}")]
    InvalidJsonWithBody {
        reason: String,
        content_type: String,
        snippet: String,
    },
    #[error("无法在 body 中定位最后一条 user 消息（协议 {0:?}）")]
    NoUserMessage(Protocol),
}

pub type Result<T> = std::result::Result<T, Error>;
