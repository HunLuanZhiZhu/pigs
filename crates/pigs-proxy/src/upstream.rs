//! 透传客户端：把请求原样搬向上游，把响应原样搬回客户端。
//!
//! 关键修复（legacy 踩过的坑）：`content-encoding` 必须随 body 转发——
//! 请求头转发 accept-encoding 后上游会回压缩体，撕掉标签客户端就会把
//! gzip 字节当明文解析（"Failed to parse JSON"）。

use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use reqwest::Client;

/// 逐跳头 / 连接管理头：由本段连接自己决定，不转发。
const HOP_SKIP: &[&str] = &["host", "content-length", "connection", "transfer-encoding"];

#[derive(Debug, Clone)]
pub struct Upstream {
    client: Client,
    base_url: String,
    /// 非空时覆盖客户端的鉴权头。
    key: String,
}

impl Upstream {
    pub fn new(base_url: &str, key: &str) -> Self {
        Self {
            client: Client::builder()
                // 不设总超时：长流式对话不受"10 分钟毒墙"限制
                .connect_timeout(std::time::Duration::from_secs(30))
                // 透传必须逐字节：关掉自动解压，content-encoding 与 body 一起搬给客户端
                .gzip(false)
                .brotli(false)
                .deflate(false)
                .build()
                .expect("构建上游 HTTP 客户端失败"),
            base_url: base_url.trim_end_matches('/').to_string(),
            key: key.to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 转发头集合：跳过逐跳头；`key` 覆盖模式时替换鉴权头，其余端到端头
    /// （content-type、anthropic-version、x-opencode-session 等）原样保留。
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

    /// 拼上游 URL：base_url + 原路径（+ 原查询串）。
    fn url(&self, path: &str, query: Option<&str>) -> String {
        let mut url = format!("{}{}", self.base_url, path_char(path));
        if let Some(q) = query.filter(|q| !q.is_empty()) {
            url.push('?');
            url.push_str(q);
        }
        url
    }

    /// 发送透传请求，返回状态码、响应头与响应（body 由调用方决定全读还是流式）。
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
