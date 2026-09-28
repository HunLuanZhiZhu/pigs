//! 子请求传输接口：编排器对"怎么发 HTTP"完全无感知，只依赖此 trait。

use bytes::Bytes;

/// 一个协议原生的子请求（发往上游 = 经 loopback 走 proxy 透传通道）。
#[derive(Debug, Clone)]
pub struct SubRequest {
    /// 协议路径（如 `/v1/messages`）。
    pub path: String,
    /// 需要随行的端到端头（鉴权、会话头等）。
    pub headers: Vec<(String, String)>,
    /// JSON body。
    pub body: Bytes,
}

/// 子请求的响应（编排强制 `stream:false`，正常情况下 body 是 JSON；
/// 若上游无视并回 SSE，也按 SSE 文本提取兜底）。
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
    async fn send(&self, req: SubRequest) -> TransportResult;
}
