//! 子请求传输接口：编排器对"怎么发 HTTP"完全无感知，只依赖此 trait。

use bytes::Bytes;
use std::sync::Arc;

/// 流式响应中接收**带类型增量**的回调。
///
/// 文本含控制标记（过滤由编排层负责）；思考增量原样转发、不过滤。
pub type LiveSink = Arc<dyn Fn(pigs_protocol::LiveEvent) + Send + Sync>;

/// 旧名（只有文本）保留为别名，便于调用方渐进迁移。
pub type TextSink = LiveSink;

/// 一个协议原生的子请求（发往上游 = 经 loopback 走 proxy 透传通道）。
#[derive(Debug, Clone)]
pub struct SubRequest {
    /// 协议路径（如 `/v1/messages`）。
    pub path: String,
    /// 原查询串（有就原样带上）。
    pub query: Option<String>,
    /// 需要随行的端到端头（鉴权、会话头等）。
    pub headers: Vec<(String, String)>,
    /// JSON body。
    pub body: Bytes,
}

/// 子请求的响应。流式与非流式都返回**完整 body**（增量已另行回调）：
/// 编排需要在全文上判定控制标记，所以累积的原文始终要拿得到。
#[derive(Debug, Clone)]
pub struct SubResponse {
    pub status: u16,
    /// 响应 content-type（判定 JSON / SSE 用）。
    pub content_type: Option<String>,
    pub body: Bytes,
}

/// 传输错误（proxy 负责转成协议错误返回给客户端）。
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("子请求传输失败: {0}")]
    Send(String),
    #[error("上游返回错误状态 {status}: {body}")]
    Upstream { status: u16, body: String },
}

pub type TransportResult = std::result::Result<SubResponse, TransportError>;

/// 传输抽象：生产实现 = proxy 的 loopback；测试实现 = 脚本化的假上游。
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// 非流式发送（客户端没要流式时走这里）。
    async fn send(&self, req: SubRequest) -> TransportResult;

    /// 流式发送：上游 SSE 增量到达时即时回调 `sink`，同时返回累积的完整响应。
    ///
    /// 两者都由实现负责——回调用于"边收边转发"（文本与思考分开报），
    /// 返回值用于在全文上判定 PIGEND/PIGFAIL 与工具调用。
    async fn send_streaming(
        &self,
        req: SubRequest,
        protocol: pigs_protocol::Protocol,
        sink: LiveSink,
    ) -> TransportResult;
}
