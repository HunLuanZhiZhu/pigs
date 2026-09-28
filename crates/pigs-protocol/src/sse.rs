//! SSE 内容类型判定与流式解析辅助。

/// 响应 content-type 是否为 SSE 流。
pub fn is_sse_content_type(content_type: Option<&str>) -> bool {
    content_type
        .map(|ct| ct.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sse_case_insensitively() {
        assert!(is_sse_content_type(Some("text/event-stream; charset=utf-8")));
        assert!(is_sse_content_type(Some("Text/Event-Stream")));
        assert!(!is_sse_content_type(Some("application/json")));
        assert!(!is_sse_content_type(None));
    }
}
