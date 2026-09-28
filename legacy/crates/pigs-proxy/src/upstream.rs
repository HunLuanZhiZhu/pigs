// 上游请求客户端与流式透传

use crate::config::{Endpoint, KeyMode, PathMode};
use crate::protocol::Protocol;
use anyhow::Result;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use reqwest::Client;
use std::time::Duration;

pub struct UpstreamClient {
    http: Client,
    loopback_http: Client,
}

impl Default for UpstreamClient {
    fn default() -> Self {
        Self::new()
    }
}

impl UpstreamClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .expect("构建 HTTP 客户端失败");
        let loopback_http = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(600))
            .build()
            .expect("构建 loopback HTTP 客户端失败");
        Self {
            http,
            loopback_http,
        }
    }

    fn upstream_url(&self, ep: &Endpoint, protocol: Protocol) -> String {
        let base = ep.base_url.trim_end_matches('/');
        match ep.path_mode {
            PathMode::Append => format!("{}{}", base, protocol.append_suffix()),
            PathMode::Full => ep.base_url.clone(),
        }
    }

    pub async fn send(
        &self,
        ep: &Endpoint,
        protocol: Protocol,
        body: &Bytes,
        client_headers: &HeaderMap,
    ) -> Result<UpstreamResponse> {
        let url = self.upstream_url(ep, protocol);
        let client = reqwest::Url::parse(&url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .filter(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
            .map(|_| &self.loopback_http)
            .unwrap_or(&self.http);
        let mut req = client.request(Method::POST, &url);

        let use_override = matches!(ep.key_mode, KeyMode::Override) && !ep.api_key.is_empty();
        if use_override {
            req = match protocol {
                Protocol::OpenAI | Protocol::Responses => req.bearer_auth(&ep.api_key),
                Protocol::Anthropic => req
                    .header("x-api-key", &ep.api_key)
                    .header("anthropic-version", "2023-06-01"),
            };
        } else {
            // Passthrough: 首先尝试从客户端头提取 auth key
            // Passthrough: first try to extract auth key from client headers
            let mut has_anthropic_version = false;
            let mut has_auth = false;
            for (name, value) in client_headers.iter() {
                let name_lower = name.as_str().to_lowercase();
                if name_lower == "authorization" || name_lower == "x-api-key" {
                    req = req.header(name, value);
                    has_auth = true;
                }
                if name_lower == "anthropic-version" {
                    has_anthropic_version = true;
                }
            }

            // 回退：客户端头中没有 auth key 时，用 endpoint 的 api_key
            // Fallback: if client headers had no auth key, use ep.api_key
            if !has_auth && !ep.api_key.is_empty() {
                req = match protocol {
                    Protocol::OpenAI | Protocol::Responses => req.bearer_auth(&ep.api_key),
                    Protocol::Anthropic => req.header("x-api-key", &ep.api_key),
                };
            }

            if protocol == Protocol::Anthropic && !has_anthropic_version {
                req = req.header("anthropic-version", "2023-06-01");
            }
        }

        for (name, value) in client_headers.iter() {
            let name_lower = name.as_str().to_lowercase();
            if matches!(
                name_lower.as_str(),
                "authorization"
                    | "x-api-key"
                    | "anthropic-version"
                    | "host"
                    | "content-length"
                    | "connection"
                    | "transfer-encoding"
            ) {
                continue;
            }
            req = req.header(name, value);
        }

        req = req.header("content-type", "application/json");

        let resp = req.body(body.clone()).send().await?;

        let status = StatusCode::from_u16(resp.status().as_u16())?;
        let is_stream = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.contains("text/event-stream"))
            .unwrap_or(false);

        let upstream_headers = resp.headers().clone();

        Ok(UpstreamResponse {
            status,
            is_stream,
            headers: upstream_headers,
            url: url.clone(),
            resp: Some(resp),
            body_bytes: None,
            preloaded: None,
        })
    }
}

pub struct UpstreamResponse {
    pub status: StatusCode,
    pub is_stream: bool,
    pub headers: HeaderMap,
    /// 上游 URL（用于日志记录）。
    /// Upstream URL (for logging).
    pub url: String,
    pub resp: Option<reqwest::Response>,
    // 非流式：完整 body
    pub body_bytes: Option<Bytes>,
    // 流式：预读的 chunks + 剩余 stream
    pub preloaded: Option<(
        Vec<Bytes>,
        Box<dyn futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin>,
    )>,
}

impl UpstreamResponse {
    // 非流式：读完整 body
    // 流式：读前几个 chunk 判断是否含 error，保留剩余 stream
    pub async fn preload_body(&mut self) {
        // 非流式
        if self.body_bytes.is_none() && !self.is_stream {
            if let Some(resp) = self.resp.take() {
                self.body_bytes = Some(resp.bytes().await.unwrap_or_else(|_| Bytes::new()));
            }
            return;
        }

        // 流式：读前几个 chunk，保留剩余 stream
        if self.preloaded.is_none() && self.is_stream {
            if let Some(resp) = self.resp.take() {
                let mut stream = resp.bytes_stream();
                let mut chunks: Vec<Bytes> = Vec::new();
                let mut buf = String::new();

                // 最多读 16 个 chunk，用于判断是否含 error
                for _ in 0..16 {
                    match stream.next().await {
                        Some(Ok(chunk)) => {
                            buf.push_str(&String::from_utf8_lossy(&chunk));
                            chunks.push(chunk);
                            // 遇到 error 事件 → 停止（是错误，可重试）
                            if buf.contains("event: error")
                                && buf.contains("\"error\":")
                                && buf.contains("\"code\":")
                            {
                                break;
                            }
                            // 遇到有效内容事件 → 停止（不是错误）
                            if buf.contains("response.output_text")
                                || buf.contains("response.completed")
                                || buf.contains("response.output_item")
                                || buf.contains("content_block_delta")
                                || buf.contains("chat.completion.chunk")
                            {
                                break;
                            }
                        }
                        _ => break,
                    }
                }

                // 保留剩余 stream（用于转发时拼合）
                let remaining: Box<
                    dyn futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin,
                > = Box::new(stream);
                self.preloaded = Some((chunks, remaining));
            }
        }
    }

    // 从已读内容中解析业务错误码
    pub fn extract_error_code(&self) -> Option<i64> {
        // 非流式
        if let Some(bytes) = &self.body_bytes {
            let val: serde_json::Value = serde_json::from_slice(bytes).ok()?;
            let code = val.get("error")?.get("code")?;
            if let Some(n) = code.as_i64() {
                return Some(n);
            }
            if let Some(s) = code.as_str() {
                return s.parse::<i64>().ok();
            }
            return None;
        }

        // 流式：从预读 chunks 拼接后查找
        if let Some((chunks, _)) = &self.preloaded {
            let text: String = chunks
                .iter()
                .map(|c| String::from_utf8_lossy(c).to_string())
                .collect::<String>();

            for line in text.lines() {
                if line.starts_with("data:") {
                    let json_str = line.trim_start_matches("data:").trim();
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if let Some(code) = val.get("error").and_then(|e| e.get("code")) {
                            if let Some(n) = code.as_i64() {
                                return Some(n);
                            }
                            if let Some(s) = code.as_str() {
                                return s.parse::<i64>().ok();
                            }
                        }
                    }
                }
            }
        }

        None
    }

    pub async fn into_axum(self) -> Response<Body> {
        let mut builder = Response::builder().status(self.status);

        for (name, value) in self.headers.iter() {
            let name_lower = name.as_str().to_lowercase();
            if matches!(
                name_lower.as_str(),
                "content-length" | "connection" | "transfer-encoding" | "content-encoding"
            ) {
                continue;
            }
            if let Ok(name) = HeaderName::from_bytes(name.as_ref()) {
                if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
                    builder = builder.header(name, value);
                }
            }
        }

        // 非流式
        if !self.is_stream {
            if let Some(bytes) = self.body_bytes {
                return builder.body(Body::from(bytes)).unwrap();
            }
            if let Some(resp) = self.resp {
                let bytes = resp.bytes().await.unwrap_or_else(|_| Bytes::new());
                return builder.body(Body::from(bytes)).unwrap();
            }
            return builder.body(Body::empty()).unwrap();
        }

        // 流式：预读 chunks + 剩余 stream 拼合成完整流
        if let Some((chunks, remaining)) = self.preloaded {
            // 把已读 chunks 作为即时流，剩余 stream 接在后面
            let chunk_stream = stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
            let combined = chunk_stream.chain(
                remaining.map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))),
            );
            let body = Body::from_stream(combined);
            return builder.body(body).unwrap();
        }

        // 未预读的流式：直接流式转发
        if let Some(resp) = self.resp {
            let stream = resp.bytes_stream();
            let body = Body::from_stream(stream);
            return builder.body(body).unwrap();
        }

        builder.body(Body::empty()).unwrap()
    }
}

/// [DEBUG-CTX] 把发往上游 LLM 的完整请求追加写入日志文件。
/// Writes the complete upstream LLM request to a log file (append mode).
///
/// 日志路径 / Log path: `~/.pig/upstream-requests.log`（即 `%USERPROFILE%\.pig\upstream-requests.log`）
/// 每条记录用分隔线隔开，包含时间戳、URL、协议、headers（脱敏 auth）、完整 body JSON。
pub fn log_upstream_request(url: &str, protocol: Protocol, body: &Bytes, headers: &HeaderMap) {
    use std::io::Write;

    let home = match dirs::home_dir() {
        Some(p) => p,
        None => return,
    };
    let log_path = home.join(".pig").join("upstream-requests.log");

    // 确保 ~/.pig/ 目录存在 / Ensure ~/.pig/ exists
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");

    // 脱敏 headers：不记录 auth key 的值 / Redact auth key values
    let header_lines: Vec<String> = headers
        .iter()
        .map(|(name, value)| {
            let name_lower = name.as_str().to_lowercase();
            let value_str = if name_lower == "authorization" || name_lower == "x-api-key" {
                "<REDACTED>".to_string()
            } else {
                String::from_utf8_lossy(value.as_bytes()).to_string()
            };
            format!("  {name}: {value_str}")
        })
        .collect();

    // 尝试格式化 JSON body / Try to pretty-print JSON body
    let body_str = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| String::from_utf8_lossy(body).to_string()))
        .unwrap_or_else(|| String::from_utf8_lossy(body).to_string());

    // 统计消息数和估算字符数 / Count messages and estimate char count
    let (msg_count, total_chars) = count_messages_and_chars(&body_str, protocol);

    let entry = format!(
        "═══════════════════════════════════════════════════════════\n\
         [{timestamp}] {protocol:?} → {url}\n\
         messages: {msg_count}, total_content_chars: {total_chars}, body_bytes: {body_len}\n\
         ── headers ──\n{headers_block}\n\
         ── body ──\n{body_str}\n",
        protocol = protocol,
        url = url,
        msg_count = msg_count,
        total_chars = total_chars,
        body_len = body.len(),
        headers_block = header_lines.join("\n"),
        body_str = body_str,
    );

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = file.write_all(entry.as_bytes());
        let _ = file.write_all(b"\n");
    }
}

/// [DEBUG-CTX] 根据 body JSON 统计 messages/input 数组的条数和总文本字符数。
fn count_messages_and_chars(body_str: &str, protocol: Protocol) -> (usize, usize) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(body_str) else {
        return (0, 0);
    };
    let collection = match protocol {
        Protocol::OpenAI | Protocol::Responses => val.get("messages").or_else(|| val.get("input")),
        Protocol::Anthropic => val.get("messages"),
    };
    let Some(arr) = collection.and_then(|v| v.as_array()) else {
        return (0, 0);
    };
    let count = arr.len();
    let total_chars: usize = arr
        .iter()
        .map(|item| {
            // 递归统计所有字符串值 / Recursively count all string values
            fn count_strings(v: &serde_json::Value) -> usize {
                match v {
                    serde_json::Value::String(s) => s.len(),
                    serde_json::Value::Array(a) => a.iter().map(count_strings).sum(),
                    serde_json::Value::Object(o) => o.values().map(count_strings).sum(),
                    _ => 0,
                }
            }
            count_strings(item)
        })
        .sum();
    (count, total_chars)
}
