//! 传输层抽象 —— 用于协议原生的相位子请求。
//! Transport abstraction for protocol-native phase subrequests.
//!
//! 本模块定义了 `PhaseTransport` trait，将"发送一次相位子请求"这一动作
//! 与具体的 HTTP 机制解耦。HTTP 相位运行时（`http_runtime`）通过此 trait
//! 调用底层传输（通常经 `pigs-proxy` 的 loopback 完成实际的上游 LLM 调用）。
//!
//! This module defines the `PhaseTransport` trait, decoupling "send one
//! phase subrequest" from the concrete HTTP machinery. The HTTP phase
//! runtime (`http_runtime`) uses this trait to invoke the underlying
//! transport (typically via `pigs-proxy`'s loopback to reach the real
//! upstream LLM).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::protocol::HttpRequestEnvelope;

/// 一次相位子请求返回的完整非流式响应。
/// Complete non-streaming response returned by one phase subrequest.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportResponse {
    /// 完整的协议原生 JSON 响应体。
    /// Complete protocol-native JSON response body.
    pub body: Value,
}

/// 相位传输层返回的错误。
/// Errors returned by a phase transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// 代理或上游返回了非成功状态码。
    /// The proxy or upstream returned a non-success status.
    #[error("phase subrequest failed with HTTP {status}: {message}")]
    Http {
        /// HTTP 状态码 / HTTP status code.
        status: u16,
        /// 响应错误文本 / response error text.
        message: String,
    },
    /// 传输层无法完成请求或解码响应。
    /// The transport could not complete or decode the request.
    #[error("phase transport failed: {0}")]
    Other(String),
}

/// 流式相位子请求中接收原始可见文本增量的回调类型。
/// Callback type that receives raw visible text deltas from a streaming phase subrequest.
///
/// 使用 `Arc<dyn Fn>` 以便在异步上下文中共享与克隆。
/// Uses `Arc<dyn Fn>` so it can be shared and cloned across async contexts.
pub type TransportTextSink = Arc<dyn Fn(String) + Send + Sync>;

/// 发送完整协议原生相位请求，不限定具体的 HTTP 机制。
/// Sends complete protocol-native phase requests without prescribing HTTP machinery.
///
/// 实现者通常是把请求转发给 `pigs-proxy` 的本地 loopback。
/// Implementers typically forward the request to `pigs-proxy`'s local loopback.
#[async_trait]
pub trait PhaseTransport: Send + Sync {
    /// 发送一次相位请求并返回其完整的协议原生 JSON 响应。
    /// Sends one phase request and returns its complete native JSON response.
    async fn send(&self, request: HttpRequestEnvelope)
        -> Result<TransportResponse, TransportError>;

    /// 发送一次相位请求，同时上报原始可见文本增量。
    /// Sends one phase request while reporting raw visible text deltas.
    ///
    /// 默认实现退化为非流式：先 `send` 一次，再用
    /// `HttpRequestEnvelope::extract_response` 解析出可见文本，
    /// 通过 `text` 回调一次性发出。真正的流式实现应当覆盖此方法，
    /// 在上游 SSE 增量到达时即时转发。
    ///
    /// The default implementation degrades to non-streaming: it calls
    /// `send` once, then parses the visible text via
    /// `HttpRequestEnvelope::extract_response` and emits it through the
    /// `text` callback in one shot. A real streaming implementation should
    /// override this method to forward upstream SSE deltas as they arrive.
    async fn send_streaming(
        &self,
        request: HttpRequestEnvelope,
        text: TransportTextSink,
    ) -> Result<TransportResponse, TransportError> {
        // 先做一次完整非流式发送 / Perform one complete non-streaming send.
        let response = self.send(request.clone()).await?;
        // 从响应体中解析出可见文本（去除控制标记）/ Extract visible text (markers stripped).
        let output = request
            .extract_response(&response.body)
            .map_err(|error| TransportError::Other(error.to_string()))?;
        // 如果有可见文本，通过回调一次性发出 / If visible text exists, emit it via the callback.
        if !output.visible_text.is_empty() {
            text(output.visible_text);
        }
        Ok(response)
    }
}
