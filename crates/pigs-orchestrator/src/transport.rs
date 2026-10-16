//! 子请求传输接口：编排器只依赖此 trait，不关心 HTTP 发送细节。 / Subrequest transport interface: the orchestrator depends only on this trait, not HTTP mechanics.

use bytes::Bytes;
use std::sync::Arc;

/// 流式响应中接收**带类型增量**的回调。 / Callback for typed streaming deltas.
///
/// 文本中的控制标记由编排层过滤；思考增量原样转发。 / The orchestrator filters text markers; reasoning deltas pass through unchanged.
pub type LiveSink = Arc<dyn Fn(pigs_protocol::LiveEvent) + Send + Sync>;

/// 旧名保留为别名，便于渐进迁移。 / Retain the legacy alias for gradual caller migration.
pub type TextSink = LiveSink;

/// 协议原生子请求，经回环走代理透传通道。 / Protocol-native subrequest sent upstream through the loopback passthrough channel.
#[derive(Debug, Clone)]
pub struct SubRequest {
    /// 协议路径（如 `/v1/messages`）。 / Protocol path (for example, `/v1/messages`).
    pub path: String,
    /// 原查询串，原样保留。 / Original query string, preserved unchanged.
    pub query: Option<String>,
    /// 端到端请求头（鉴权、会话等）。 / End-to-end headers (authentication, session, etc.).
    pub headers: Vec<(String, String)>,
    /// JSON 请求体。 / JSON request body.
    pub body: Bytes,
}

/// 子请求响应：流式与非流式都返回完整响应体（增量另行回调）。 / Subresponse: both modes return the full body; streaming deltas are reported separately.
/// 编排需要完整原文来判定控制标记。 / The orchestrator needs the complete text to inspect control markers.
#[derive(Debug, Clone)]
pub struct SubResponse {
    pub status: u16,
    /// 响应内容类型，用于区分 JSON / SSE。 / Response content type for JSON/SSE detection.
    pub content_type: Option<String>,
    pub body: Bytes,
}

/// 传输错误，由代理转换为客户端协议错误。 / Transport error converted by the proxy into a client-facing protocol error.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("子请求传输失败: {0}")]
    Send(String),
    #[error("上游返回错误状态 {status}: {body}")]
    Upstream { status: u16, body: String },
}

pub type TransportResult = std::result::Result<SubResponse, TransportError>;

/// 传输抽象：生产环境用代理回环，测试用模拟上游。 / Transport abstraction: proxy loopback in production, scripted mock upstream in tests.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// 非流式发送。 / Non-streaming send.
    async fn send(&self, req: SubRequest) -> TransportResult;

    /// 流式发送：SSE 增量到达时回调 `sink`，并返回累积的完整响应。 / Streaming send: invoke `sink` on each SSE delta and return the accumulated full response.
    ///
    /// 实现同时负责增量回调（区分文本和思考）与完整响应。 / Implementations handle both typed delta callbacks and the full response.
    /// 完整响应用于判定控制标记和工具调用。 / The full response is used to detect control markers and tool calls.
    async fn send_streaming(
        &self,
        req: SubRequest,
        protocol: pigs_protocol::Protocol,
        sink: LiveSink,
    ) -> TransportResult;
}
