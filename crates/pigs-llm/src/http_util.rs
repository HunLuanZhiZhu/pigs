//! Shared HTTP / SSE helpers for LLM providers.
//!
//! Patterns drawn from reference clients:
//! - claw-code `api` SSE framing + status mapping
//! - codex / pi OpenAI clients: Retry-After, stream usage options

use pigs_core::ApiError;

/// Parse `Retry-After` header value (seconds) with a default.
pub fn retry_after_secs(response: &reqwest::Response, default_secs: u64) -> u64 {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default_secs)
        .clamp(1, 600)
}

/// Map HTTP status + body into a typed `ApiError`.
pub fn map_http_status(status: u16, body: String) -> ApiError {
    let lower = body.to_ascii_lowercase();
    if lower.contains("context")
        && (lower.contains("window")
            || lower.contains("length")
            || lower.contains("too long")
            || lower.contains("maximum context"))
    {
        return ApiError::ContextWindowExceeded(body);
    }

    match status {
        401 | 403 => ApiError::Auth(body),
        404 => ApiError::ModelNotFound(body),
        429 => ApiError::RateLimited {
            retry_after_secs: 30,
        },
        _ => ApiError::Http { status, body },
    }
}

/// Map a finished `reqwest::Response` error status into `ApiError`.
pub async fn map_error_response(response: reqwest::Response) -> ApiError {
    let status = response.status().as_u16();
    if status == 429 {
        let retry = retry_after_secs(&response, 30);
        // Consume body for diagnostics but keep typed rate limit.
        let _body = response.text().await.unwrap_or_default();
        return ApiError::RateLimited {
            retry_after_secs: retry,
        };
    }
    let body = response.text().await.unwrap_or_default();
    map_http_status(status, body)
}

/// Whether an error should abort retries immediately.
pub fn is_non_retryable(err: &ApiError) -> bool {
    matches!(
        err,
        ApiError::Auth(_)
            | ApiError::ModelNotFound(_)
            | ApiError::ContextWindowExceeded(_)
            | ApiError::Config(_)
    )
}

/// Join base URL with a path, avoiding double slashes.
pub fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn context_window_detection() {
        let err = map_http_status(400, "This model's maximum context length is 128000 tokens".into());
        assert!(matches!(err, ApiError::ContextWindowExceeded(_)));
    }

    #[test]
    fn join_url_trims_slashes() {
        assert_eq!(
            join_url("https://api.openai.com/v1/", "/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }
}
