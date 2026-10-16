//! HTTP 透传客户端：转发请求和响应。 / HTTP passthrough client forwarding requests and responses.
//!
//! 压缩响应的内容编码头必须与响应体一起转发。 / Preserve content-encoding with the compressed response body.
//! 上游可能依据 accept-encoding 返回压缩体。 / Upstream may compress responses according to accept-encoding.
//! 缺少编码头会导致客户端误解析压缩字节。 / Dropping the encoding header makes clients misparse compressed bytes.

use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use reqwest::Client;

/// 逐跳连接头不得转发。 / Do not forward hop-by-hop connection headers.
const HOP_SKIP: &[&str] = &["host", "content-length", "connection", "transfer-encoding"];

#[derive(Debug, Clone)]
pub struct Upstream {
    client: Client,
    /// 三协议分别配置基础地址。 / Separate base URLs for the three protocols.
    openai: String,
    responses: String,
    anthropic: String,
    /// 非空配置覆盖客户端鉴权。 / Nonempty key overrides client authentication.
    key: String,
}

impl Upstream {
    pub fn new(upstream: &crate::config::Upstreams, key: &str) -> Self {
        Self {
            client: Client::builder()
                // 不设总超时，允许长时间流式连接。 / No total timeout, allowing long-lived streams.
                .connect_timeout(std::time::Duration::from_secs(30))
                // 禁用自动解压以保持压缩体和编码头一致。 / Disable decompression to preserve body/encoding consistency.
                .gzip(false)
                .brotli(false)
                .deflate(false)
                .build()
                .expect("构建上游 HTTP 客户端失败"),
            openai: upstream.openai.trim_end_matches('/').to_string(),
            responses: upstream.responses.trim_end_matches('/').to_string(),
            anthropic: upstream.anthropic.trim_end_matches('/').to_string(),
            key: key.to_string(),
        }
    }

    /// 根据协议选基础地址；其它路径默认 OpenAI。 / Select protocol base URL; default unknown paths to OpenAI.
    ///
    /// 原路径不改写；Anthropic 的版本属于路径。 / Preserve the client path; Anthropic puts its version in the path.
    /// Chat/Responses 的版本段属于基础地址。 / Chat/Responses place versioning in the base URL.
    fn base_for(&self, path: &str) -> &str {
        match pigs_protocol::protocol_from_path(path) {
            Some(pigs_protocol::Protocol::OpenAI) => &self.openai,
            Some(pigs_protocol::Protocol::Responses) => &self.responses,
            Some(pigs_protocol::Protocol::Anthropic) => &self.anthropic,
            None => &self.openai,
        }
    }

    /// 跳过逐跳头，必要时覆盖鉴权，其它端到端头保留。 / Skip hop-by-hop headers, override auth if configured, and keep end-to-end headers.
    /// 保留 content-type、协议版本及会话头。 / Preserve content-type, protocol version, and session headers.
    pub fn forward_headers(&self, headers: &HeaderMap) -> Vec<(String, String)> {
        let override_auth = !self.key.is_empty();
        let mut out = Vec::new();
        for (name, value) in headers.iter() {
            let lower = name.as_str().to_lowercase();
            if HOP_SKIP.contains(&lower.as_str()) {
                continue;
            }
            if override_auth && matches!(lower.as_str(), "authorization" | "x-api-key") {
                continue;
            }
            out.push((
                name.as_str().to_string(),
                value.to_str().unwrap_or("").to_string(),
            ));
        }
        if override_auth {
            out.push(("authorization".into(), format!("Bearer {}", self.key)));
            out.push(("x-api-key".into(), self.key.clone()));
        }
        out
    }

    /// 上游 URL = 协议基础地址 + 原路径和查询串。 / Upstream URL = protocol base + original path and query.
    pub(crate) fn url(&self, path: &str, query: Option<&str>) -> String {
        let mut url = format!("{}{}", self.base_for(path), path_char(path));
        if let Some(q) = query.filter(|q| !q.is_empty()) {
            url.push('?');
            url.push_str(q);
        }
        url
    }

    /// 发送透传请求，返回状态码、头和响应体。 / Forward a request and return status, headers, and response body.
    pub async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: Option<&str>,
        headers: Vec<(String, String)>,
        body: Option<Bytes>,
    ) -> anyhow::Result<(StatusCode, HeaderMap, reqwest::Response)> {
        let mut req = self.client.request(method, self.url(path, query));
        for (k, v) in &headers {
            if HOP_SKIP.contains(&k.to_lowercase().as_str()) {
                continue;
            }
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        let resp = req.send().await?;
        let status = StatusCode::from_u16(resp.status().as_u16())?;
        let resp_headers = resp.headers().clone();
        Ok((status, resp_headers, resp))
    }
}

fn path_char(path: &str) -> String {
    format!("/{}", path.trim_start_matches('/'))
}
