//! pigs-protocol —— 三种协议的公共字典。
//!
//! 全仓库最底层 crate：路径路由、`-pig` 后缀规则、请求体手术（改 model / 关流式 /
//! 去 tools / 替换最后一条 user 消息）、响应文本提取（JSON 与 SSE）、最终答复合成。
//! 只依赖 serde_json，不做任何 HTTP。

pub mod output;
pub mod response;
pub mod route;
pub mod sse;
pub mod surgery;

pub use output::{
    parse_json_output, parse_sse_output, trailing_tool_result_ids, ModelOutput, Part, ToolCall,
};
pub use response::{
    extract_response_text, extract_sse_text, synthesize_json, synthesize_sse, LiveEvent,
    ResponseContent, SseTextStream, StreamEncoder,
};
pub use route::{has_pig, protocol_from_path, strip_pig_suffix, Protocol};
pub use sse::is_sse_content_type;
pub use surgery::{
    append_instruction, append_to_last_user_text, extract_last_user_text, get_model, has_client_stream,
    push_assistant_message, push_user_message, set_model, set_stream, strip_tools,
    SUFFIX_SEPARATOR,
};

/// 编解码错误。
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
