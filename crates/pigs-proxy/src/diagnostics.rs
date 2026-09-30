//! HTTP 诊断抓包：每个请求/响应单独落一个明文文件。
//!
//! `basic` 只记录元数据与头，`max` 额外记录完整 body；敏感头始终打码。

use crate::config::{LogDetail, LoggingConfig};
use axum::http::HeaderMap;
use brotli::Decompressor;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct HttpDiagnostics {
    detail: LogDetail,
    directory: Arc<PathBuf>,
    counter: Arc<AtomicU64>,
}

#[derive(Clone)]
pub struct ExchangeLog {
    diagnostics: HttpDiagnostics,
    id: String,
    internal: bool,
}

pub struct ResponseCapture {
    exchange: ExchangeLog,
    stage: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    byte_count: usize,
    finished: bool,
}

impl HttpDiagnostics {
    pub fn new(config: &LoggingConfig) -> Self {
        if config.detail != LogDetail::Off {
            if let Err(error) = fs::create_dir_all(&config.directory) {
                tracing::warn!(error = %error, directory = %config.directory, "创建 HTTP 诊断日志目录失败");
            }
        }
        Self {
            detail: config.detail,
            directory: Arc::new(PathBuf::from(&config.directory)),
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn detail(&self) -> LogDetail {
        self.detail
    }

    pub fn directory(&self) -> &std::path::Path {
        self.directory.as_path()
    }

    pub fn begin_exchange(
        &self,
        internal: bool,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        body: &[u8],
    ) -> ExchangeLog {
        let sequence = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let exchange = ExchangeLog {
            diagnostics: self.clone(),
            id: format!("{millis}-{sequence:06}"),
            internal,
        };
        let stage = if internal {
            "internal-request"
        } else {
            "client-request"
        };
        exchange.write_request(stage, method, path, query, &header_pairs(headers), body);
        exchange
    }
}

impl ExchangeLog {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn write_upstream_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: &[(String, String)],
        body: &[u8],
    ) {
        self.write_request("upstream-request", method, path, query, headers, body);
    }

    /// 写一份只包含诊断字段的明文事件文件，与原始 request/response 使用同一 exchange id。
    pub fn write_event(&self, stage: &str, fields: &[String]) {
        if self.diagnostics.detail == LogDetail::Off {
            return;
        }
        let mut lines = vec![
            format!("exchange: {}", self.id),
            format!("stage: {stage}"),
            format!("internal: {}", self.internal),
        ];
        lines.extend(fields.iter().cloned());
        self.write_file(stage, lines, &[], 0, &[]);
    }

    pub fn write_response(
        &self,
        stage: &str,
        status: u16,
        headers: &[(String, String)],
        body: &[u8],
    ) {
        if self.diagnostics.detail == LogDetail::Off {
            return;
        }
        let mut meta = vec![
            format!("exchange: {}", self.id),
            format!("stage: {stage}"),
            format!("internal: {}", self.internal),
            format!("status: {status}"),
            "capture_complete: true".into(),
        ];
        meta.push("headers:".into());
        meta.extend(render_headers(headers));
        self.write_file(stage, meta, body, body.len(), headers);
    }

    pub fn capture_response(
        &self,
        stage: &str,
        status: u16,
        headers: &[(String, String)],
    ) -> Option<ResponseCapture> {
        (self.diagnostics.detail != LogDetail::Off).then(|| ResponseCapture {
            exchange: self.clone(),
            stage: stage.to_string(),
            status,
            headers: headers.to_vec(),
            body: Vec::new(),
            byte_count: 0,
            finished: false,
        })
    }

    pub fn client_response_stage(&self) -> &'static str {
        if self.internal {
            "internal-response"
        } else {
            "client-response"
        }
    }

    fn write_request(
        &self,
        stage: &str,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: &[(String, String)],
        body: &[u8],
    ) {
        if self.diagnostics.detail == LogDetail::Off {
            return;
        }
        let target = redact_target(path, query);
        let mut meta = vec![
            format!("exchange: {}", self.id),
            format!("stage: {stage}"),
            format!("internal: {}", self.internal),
            format!("method: {method}"),
            format!("target: {target}"),
        ];
        meta.push("headers:".into());
        meta.extend(render_headers(headers));
        self.write_file(stage, meta, body, body.len(), headers);
    }

    fn write_file(
        &self,
        stage: &str,
        meta: Vec<String>,
        body: &[u8],
        body_len: usize,
        headers: &[(String, String)],
    ) {
        let mut text = meta.join("\n");
        text.push_str("\n\nbody:\n");
        match self.diagnostics.detail {
            LogDetail::Off => return,
            LogDetail::Basic => {
                text.push_str(&format!("<omitted: {body_len} bytes; detail=basic>\n"));
            }
            LogDetail::Max => {
                let decoded = decode_body(headers, body);
                text.push_str(&String::from_utf8_lossy(&decoded));
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
        }
        let path = self
            .diagnostics
            .directory
            .join(format!("{}.{}.txt", self.id, stage));
        if let Err(error) = fs::write(&path, text) {
            tracing::warn!(error = %error, path = %path.display(), "写 HTTP 诊断日志失败");
        }
    }
}

impl ResponseCapture {
    pub fn push(&mut self, bytes: &[u8]) {
        self.byte_count += bytes.len();
        if self.exchange.diagnostics.detail == LogDetail::Max {
            self.body.extend_from_slice(bytes);
        }
    }

    pub fn finish(mut self) {
        self.flush(true);
        self.finished = true;
    }

    fn flush(&self, complete: bool) {
        if self.exchange.diagnostics.detail == LogDetail::Off {
            return;
        }
        let mut meta = vec![
            format!("exchange: {}", self.exchange.id),
            format!("stage: {}", self.stage),
            format!("internal: {}", self.exchange.internal),
            format!("status: {}", self.status),
            format!("capture_complete: {complete}"),
            format!("body_bytes: {}", self.byte_count),
            "headers:".into(),
        ];
        meta.extend(render_headers(&self.headers));
        self.exchange.write_file(
            &self.stage,
            meta,
            &self.body,
            self.byte_count,
            &self.headers,
        );
    }
}

impl Drop for ResponseCapture {
    fn drop(&mut self) {
        if !self.finished {
            self.flush(false);
        }
    }
}

pub fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or("<non-utf8>").to_string(),
            )
        })
        .collect()
}

fn render_headers(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .map(|(name, value)| {
            let rendered = if sensitive_header(name) {
                "<redacted>"
            } else {
                value
            };
            format!("{name}: {rendered}")
        })
        .collect()
}

fn sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "x-auth-token"
            | "cookie"
            | "set-cookie"
            | "x-pigs-loopback"
    )
}

fn redact_target(path: &str, query: Option<&str>) -> String {
    if query.is_none() {
        if let Some((base, embedded_query)) = path.split_once('?') {
            return redact_target(base, Some(embedded_query));
        }
    }
    let Some(query) = query.filter(|query| !query.is_empty()) else {
        return path.to_string();
    };
    let redacted = query
        .split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let lower = key.to_ascii_lowercase();
            if ["key", "token", "secret", "auth", "password"]
                .iter()
                .any(|needle| lower.contains(needle))
            {
                format!("{key}=<redacted>")
            } else if value.is_empty() {
                key.to_string()
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{redacted}")
}

fn decode_body(headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
    let encoding = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, value)| value.trim().to_ascii_lowercase());
    let Some(encoding) = encoding else {
        return body.to_vec();
    };
    match encoding.as_str() {
        "gzip" => read_all(GzDecoder::new(body)).unwrap_or_else(|| body.to_vec()),
        "deflate" => read_all(ZlibDecoder::new(body))
            .or_else(|| read_all(DeflateDecoder::new(body)))
            .unwrap_or_else(|| body.to_vec()),
        "br" => read_all(Decompressor::new(body, 4096)).unwrap_or_else(|| body.to_vec()),
        _ => body.to_vec(),
    }
}

fn read_all<R: Read>(mut reader: R) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn max_log_writes_body_and_redacts_credentials() {
        let dir = std::env::temp_dir().join(format!("pigs-http-log-{}", uuid::Uuid::new_v4()));
        let diagnostics = HttpDiagnostics::new(&LoggingConfig {
            detail: LogDetail::Max,
            directory: dir.to_string_lossy().into_owned(),
        });
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let exchange = diagnostics.begin_exchange(
            false,
            "POST",
            "/chat/completions",
            Some("api_key=secret&x=1"),
            &headers,
            br#"{"hello":"world"}"#,
        );
        let path = dir.join(format!("{}.client-request.txt", exchange.id()));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("authorization: <redacted>"));
        assert!(text.contains("api_key=<redacted>&x=1"));
        assert!(text.contains(r#"{"hello":"world"}"#));
        assert!(!text.contains("Bearer secret"));
        assert_eq!(
            redact_target("https://example.test/a?token=secret&x=1", None),
            "https://example.test/a?token=<redacted>&x=1"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn basic_stream_capture_records_size_without_body() {
        let dir = std::env::temp_dir().join(format!("pigs-http-log-{}", uuid::Uuid::new_v4()));
        let diagnostics = HttpDiagnostics::new(&LoggingConfig {
            detail: LogDetail::Basic,
            directory: dir.to_string_lossy().into_owned(),
        });
        let headers = HeaderMap::new();
        let exchange = diagnostics.begin_exchange(false, "GET", "/x", None, &headers, &[]);
        let mut capture = exchange
            .capture_response("client-response", 200, &[])
            .unwrap();
        capture.push(b"abc");
        capture.finish();
        let path = dir.join(format!("{}.client-response.txt", exchange.id()));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("body_bytes: 3"));
        assert!(text.contains("<omitted: 3 bytes; detail=basic>"));
        assert!(!text.contains("abc"));
        let _ = fs::remove_dir_all(dir);
    }
}
