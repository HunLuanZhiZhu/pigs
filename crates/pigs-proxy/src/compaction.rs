//! Routing-layer context-window compaction.
//!
//! When a request's estimated token count exceeds the model's context window
//! (multiplied by a coefficient, e.g. 0.9), the routing layer automatically
//! compacts the conversation by:
//!
//! 1. Checking a prefix cache — if the same message prefix was compacted before,
//!    reuse the cached summary.
//! 2. If no cache hit, send old messages to the LLM for summarization (via
//!    loopback), store the result in cache, and replace the old messages.
//! 3. Re-submit the compacted request through the routing layer for re-evaluation.
//!
//! This is transparent to the harness/Agent layer — they never know compaction happened.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::{CompactionConfig, Endpoint, default_context_window_for};
use crate::loopback::INTERNAL_PHASE_HEADER;
use crate::protocol::Protocol;

/// Error type for compaction operations.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("compaction failed: {0}")]
    Failed(String),
    #[error("LLM summarization request failed: {0}")]
    LlmRequest(String),
    #[error("LLM summarization returned empty response")]
    EmptySummary,
    #[error("max compaction rounds ({0}) exceeded without fitting context window")]
    MaxRoundsExceeded(u32),
}

/// A cached compaction result for a specific message prefix.
#[derive(Debug, Clone)]
struct CompactionCacheEntry {
    /// The summary message that replaces the compacted prefix.
    summary_message: serde_json::Value,
    /// Number of original messages that were compacted into the summary.
    compacted_count: usize,
    /// Token count of the original prefix (for diagnostics).
    original_tokens: u64,
}

/// In-memory compaction cache keyed by prefix hash.
/// Key format: "{model}:{prefix_hash}"
pub struct CompactionCache {
    entries: HashMap<String, CompactionCacheEntry>,
}

impl CompactionCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<&CompactionCacheEntry> {
        self.entries.get(key)
    }

    fn insert(&mut self, key: String, entry: CompactionCacheEntry) {
        self.entries.insert(key, entry);
    }
}

impl Default for CompactionCache {
    fn default() -> Self {
        Self::new()
    }
}

/// The structured summarization prompt sent to the LLM.
/// Inspired by opencode's SUMMARY_TEMPLATE and cline's summary format.
const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a conversation summarizer. Summarize the following conversation context into a structured summary.

Output a summary with these sections:

## Objective
What the user is trying to accomplish.

## Important Details
Key technical decisions, constraints, and context that must be preserved.

## Work State
- Completed: What has been done.
- Active: What is currently being worked on.
- Blocked: Any blockers or unresolved issues.

## Relevant Files
Files that have been read, modified, or discussed. Include exact paths.

## Key Code & Commands
Important code snippets, commands, or configurations that were established.

## Next Step
What should happen next to continue the work.

Rules:
- Preserve exact file paths, symbol names, error messages, and commands.
- Be concise but complete — do not lose critical context.
- Never mention the compaction process or that this is a summary.
- Write in the same language as the conversation."#;

/// Estimate token count for a request body's messages array.
/// Uses chars/4 heuristic. Counts message text + tool_use input + tool_result output.
pub fn estimate_tokens(body: &serde_json::Value) -> u64 {
    let messages = match body.get("messages").and_then(|m| m.as_array()) {
        Some(arr) => arr,
        None => return 0,
    };

    let mut total_chars: usize = 0;

    for msg in messages {
        // OpenAI Chat format: { role, content } where content is string or array
        // Anthropic format: { role, content } where content is array of blocks
        // Responses format: { role, content } similar

        // Handle string content (OpenAI Chat simple format)
        if let Some(content_str) = msg.get("content").and_then(|c| c.as_str()) {
            total_chars += content_str.len();
        }

        // Handle array content (all protocols' block format)
        if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content_arr {
                // Text block: { type: "text", text: "..." }
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    total_chars += text.len();
                }
                // Tool use: { type: "tool_use", input: {...} }
                if let Some(input) = block.get("input") {
                    if let Ok(s) = serde_json::to_string(input) {
                        total_chars += s.len();
                    }
                }
                // Tool result: { type: "tool_result", content: "..." or [...] }
                if let Some(result) = block.get("content") {
                    if let Some(s) = result.as_str() {
                        total_chars += s.len();
                    } else if let Some(arr) = result.as_array() {
                        for r_block in arr {
                            if let Some(text) = r_block.get("text").and_then(|t| t.as_str()) {
                                total_chars += text.len();
                            }
                        }
                    }
                }
            }
        }

        // Also count role string
        if let Some(role) = msg.get("role").and_then(|r| r.as_str()) {
            total_chars += role.len();
        }
    }

    // Also count system prompt if present (OpenAI format: top-level "system" or messages[0])
    // For Anthropic: top-level "system" field
    if let Some(system) = body.get("system") {
        if let Some(s) = system.as_str() {
            total_chars += s.len();
        } else if let Some(arr) = system.as_array() {
            for block in arr {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    total_chars += text.len();
                }
            }
        }
    }

    (total_chars as u64) / 4
}

/// Compute a hash of the message prefix (all messages except the last `keep_recent`).
/// Used as the cache key for compaction results.
fn hash_prefix(messages: &[serde_json::Value], keep_recent: usize) -> String {
    if messages.len() <= keep_recent {
        return String::new();
    }
    let split_point = messages.len() - keep_recent;
    let prefix = &messages[..split_point];

    // Simple hash: serialize prefix to JSON string and use a basic hash.
    // This avoids external dependencies. For production, consider blake3.
    let json = serde_json::to_string(prefix).unwrap_or_default();
    // FNV-1a 64-bit hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in json.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Build the cache key from model name and prefix hash.
fn cache_key(model: &str, prefix_hash: &str) -> String {
    format!("{model}:{prefix_hash}")
}

/// Configuration for the loopback used by compaction.
pub struct LoopbackConfig {
    pub base_url: String,
    pub token: String,
}

/// Main compaction entry point. Called from `handle_passthrough` before dispatch.
///
/// Checks if the request body exceeds the context window threshold.
/// If so, compacts via cache or LLM summarization, then re-evaluates.
/// Returns the (possibly compacted) body.
pub async fn ensure_context_fits(
    body: serde_json::Value,
    model: &str,
    endpoint: &Endpoint,
    protocol: Protocol,
    config: &CompactionConfig,
    cache: &Arc<Mutex<CompactionCache>>,
    loopback: &LoopbackConfig,
    client_model: &str,
    round: u32,
) -> Result<serde_json::Value, CompactionError> {
    if !config.enabled {
        return Ok(body);
    }

    // Resolve context window: per-model > provider-level default > name-based heuristic
    let context_window = endpoint
        .context_windows
        .get(model)
        .copied()
        .or(endpoint.context_window)
        .unwrap_or_else(|| default_context_window_for(model));
    let threshold = ((context_window as f64) * config.coefficient) as u64;
    let estimated = estimate_tokens(&body);

    if estimated <= threshold {
        tracing::debug!(
            "compaction: tokens {} <= threshold {}, no compaction needed",
            estimated,
            threshold
        );
        return Ok(body);
    }

    tracing::info!(
        "compaction: tokens {} > threshold {} (context_window={}, coefficient={}), compacting (round {}/{})",
        estimated,
        threshold,
        context_window,
        config.coefficient,
        round,
        config.max_rounds
    );

    // Prevent infinite recursion
    if round >= config.max_rounds {
        tracing::warn!(
            "compaction: max rounds ({}) exceeded, falling back to truncation",
            config.max_rounds
        );
        return Ok(fallback_truncate(
            body,
            config.keep_recent,
            threshold,
        ));
    }

    let messages = body
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();

    if messages.len() <= config.keep_recent {
        tracing::warn!(
            "compaction: only {} messages, keep_recent={}, cannot compact further",
            messages.len(),
            config.keep_recent
        );
        return Ok(body);
    }

    let prefix_hash = hash_prefix(&messages, config.keep_recent);
    let key = cache_key(model, &prefix_hash);

    // Check cache
    let cached_entry = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();

    let compacted_body = if let Some(entry) = cached_entry {
        tracing::info!(
            "compaction: cache hit for prefix {prefix_hash}, reusing cached summary ({} messages → 1 summary + {} recent)",
            entry.compacted_count,
            config.keep_recent
        );
        replace_prefix_with_summary(body, &entry.summary_message, entry.compacted_count)
    } else {
        // No cache: do LLM summarization
        let summary_message = compact_with_llm(
            &body,
            &messages,
            model,
            endpoint,
            protocol,
            config,
            loopback,
            client_model,
        )
        .await?;

        let split_point = messages.len() - config.keep_recent;
        let entry = CompactionCacheEntry {
            summary_message: summary_message.clone(),
            compacted_count: split_point,
            original_tokens: estimated,
        };
        cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, entry);

        replace_prefix_with_summary(body, &summary_message, split_point)
    };

    // Re-evaluate: recursively check if the compacted body still exceeds threshold.
    // This implements the "re-submit to routing layer" loop.
    Box::pin(ensure_context_fits(
        compacted_body,
        model,
        endpoint,
        protocol,
        config,
        cache,
        loopback,
        client_model,
        round + 1,
    ))
    .await
}

/// Replace the first `compacted_count` messages with a single summary message.
fn replace_prefix_with_summary(
    mut body: serde_json::Value,
    summary_message: &serde_json::Value,
    compacted_count: usize,
) -> serde_json::Value {
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        if compacted_count <= messages.len() {
            // Remove the old prefix and insert the summary at the beginning
            messages.drain(0..compacted_count);
            messages.insert(0, summary_message.clone());
        }
    }
    body
}

/// Send old messages to the LLM for summarization via loopback.
///
/// Constructs a summarization request (system prompt + serialized old messages)
/// and sends it to the same upstream endpoint through the proxy's loopback.
async fn compact_with_llm(
    body: &serde_json::Value,
    messages: &[serde_json::Value],
    model: &str,
    _endpoint: &Endpoint,
    protocol: Protocol,
    config: &CompactionConfig,
    loopback: &LoopbackConfig,
    client_model: &str,
) -> Result<serde_json::Value, CompactionError> {
    let split_point = messages.len() - config.keep_recent;
    let old_messages = &messages[..split_point];

    // Serialize old messages as text for the summarization prompt
    let conversation_text = serialize_messages_for_summary(old_messages);

    // Build the summarization request body based on protocol
    let (path, summary_body) = build_summary_request(
        model,
        &conversation_text,
        protocol,
        config.summary_max_tokens,
    );

    tracing::debug!(
        "compaction: sending summarization request to {} ({} old messages, {} chars)",
        path,
        old_messages.len(),
        conversation_text.len()
    );

    // Send via loopback HTTP
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| CompactionError::LlmRequest(e.to_string()))?;

    let url = format!("{}{}", loopback.base_url, path);
    let response = client
        .post(&url)
        .header(INTERNAL_PHASE_HEADER, &loopback.token)
        .header("content-type", "application/json")
        .json(&summary_body)
        .send()
        .await
        .map_err(|e| CompactionError::LlmRequest(format!("loopback request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(CompactionError::LlmRequest(format!(
            "summarization request failed: {status} - {text}"
        )));
    }

    let response_json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| CompactionError::LlmRequest(format!("failed to parse summary response: {e}")))?;

    // Extract the summary text from the response (protocol-dependent)
    let summary_text = extract_summary_text(&response_json, protocol)
        .ok_or(CompactionError::EmptySummary)?;

    if summary_text.trim().is_empty() {
        return Err(CompactionError::EmptySummary);
    }

    tracing::info!(
        "compaction: received summary ({} chars) from LLM",
        summary_text.len()
    );

    // Build the summary message that will replace the old prefix
    Ok(build_summary_message(&summary_text, protocol, client_model))
}

/// Serialize messages into a text format for the summarization prompt.
fn serialize_messages_for_summary(messages: &[serde_json::Value]) -> String {
    let mut output = String::new();
    for (i, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("unknown");
        output.push_str(&format!("--- Message {i} [{role}] ---\n"));

        // String content
        if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
            output.push_str(content);
            output.push('\n');
        }

        // Array content (blocks)
        if let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) {
            for block in blocks {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            output.push_str(text);
                            output.push('\n');
                        }
                    }
                    "tool_use" | "function_call" => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                        output.push_str(&format!("[Tool Call: {name}]\n"));
                        if let Some(input) = block.get("input") {
                            let input_str = serde_json::to_string_pretty(input).unwrap_or_default();
                            // Truncate large inputs
                            let truncated = if input_str.len() > 2000 {
                                format!("{}...(truncated)", &input_str[..2000])
                            } else {
                                input_str
                            };
                            output.push_str(&truncated);
                            output.push('\n');
                        }
                    }
                    "tool_result" => {
                        output.push_str("[Tool Result]\n");
                        if let Some(content) = block.get("content") {
                            if let Some(s) = content.as_str() {
                                let truncated = if s.len() > 1000 {
                                    format!("{}...(truncated)", &s[..1000])
                                } else {
                                    s.to_string()
                                };
                                output.push_str(&truncated);
                                output.push('\n');
                            } else if let Some(arr) = content.as_array() {
                                for r in arr {
                                    if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                                        let truncated = if text.len() > 1000 {
                                            format!("{}...(truncated)", &text[..1000])
                                        } else {
                                            text.to_string()
                                        };
                                        output.push_str(&truncated);
                                        output.push('\n');
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // Unknown block type, serialize as-is
                        let s = serde_json::to_string(block).unwrap_or_default();
                        output.push_str(&s);
                        output.push('\n');
                    }
                }
            }
        }
        output.push('\n');
    }
    output
}

/// Build the summarization request body for the given protocol.
fn build_summary_request(
    model: &str,
    conversation_text: &str,
    protocol: Protocol,
    max_tokens: u32,
) -> (String, serde_json::Value) {
    let (path, body) = match protocol {
        Protocol::OpenAI => {
            // OpenAI Chat Completions: /chat/completions
            let body = serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "stream": false,
                "messages": [
                    { "role": "system", "content": SUMMARIZATION_SYSTEM_PROMPT },
                    { "role": "user", "content": format!("Summarize the following conversation:\n\n{conversation_text}") }
                ]
            });
            ("/chat/completions", body)
        }
        Protocol::Anthropic => {
            // Anthropic Messages: /v1/messages
            let body = serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "stream": false,
                "system": SUMMARIZATION_SYSTEM_PROMPT,
                "messages": [
                    { "role": "user", "content": format!("Summarize the following conversation:\n\n{conversation_text}") }
                ]
            });
            ("/v1/messages", body)
        }
        Protocol::Responses => {
            // OpenAI Responses: /responses
            let body = serde_json::json!({
                "model": model,
                "max_output_tokens": max_tokens,
                "stream": false,
                "instructions": SUMMARIZATION_SYSTEM_PROMPT,
                "input": format!("Summarize the following conversation:\n\n{conversation_text}")
            });
            ("/responses", body)
        }
    };
    (path.to_string(), body)
}

/// Extract the summary text from an LLM response (protocol-dependent).
fn extract_summary_text(response: &serde_json::Value, protocol: Protocol) -> Option<String> {
    match protocol {
        Protocol::OpenAI => {
            // OpenAI Chat: choices[0].message.content
            response
                .get("choices")?
                .get(0)?
                .get("message")?
                .get("content")?
                .as_str()
                .map(|s| s.to_string())
        }
        Protocol::Anthropic => {
            // Anthropic: content[0].text
            response
                .get("content")?
                .get(0)?
                .get("text")?
                .as_str()
                .map(|s| s.to_string())
        }
        Protocol::Responses => {
            // OpenAI Responses: output[0].content[0].text
            response
                .get("output")?
                .get(0)?
                .get("content")?
                .get(0)?
                .get("text")?
                .as_str()
                .map(|s| s.to_string())
        }
    }
}

/// Build the summary message that replaces the old prefix.
/// Format depends on the protocol.
fn build_summary_message(
    summary_text: &str,
    protocol: Protocol,
    _client_model: &str,
) -> serde_json::Value {
    let preamble = "--- Conversation Summary (auto-compacted) ---\n";
    let full_text = format!("{preamble}{summary_text}\n--- End Summary ---");

    match protocol {
        Protocol::OpenAI => serde_json::json!({
            "role": "system",
            "content": full_text
        }),
        Protocol::Anthropic => serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": full_text }]
        }),
        Protocol::Responses => serde_json::json!({
            "role": "system",
            "content": full_text
        }),
    }
}

/// Fallback: simple truncation when LLM summarization fails or max rounds exceeded.
/// Keeps the most recent messages and drops the rest.
fn fallback_truncate(
    mut body: serde_json::Value,
    keep_recent: usize,
    _target_tokens: u64,
) -> serde_json::Value {
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        if messages.len() > keep_recent {
            let removed = messages.len() - keep_recent;
            messages.drain(0..removed);
            // Insert a simple truncation notice
            let notice = serde_json::json!({
                "role": "system",
                "content": format!("--- {removed} earlier messages truncated (compaction fallback) ---")
            });
            messages.insert(0, notice);
        }
    }
    body
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_estimate_tokens_simple() {
        let body = serde_json::json!({
            "messages": [
                { "role": "user", "content": "Hello world" },  // 11 chars
                { "role": "assistant", "content": "Hi there!" }, // 9 chars
            ]
        });
        let tokens = estimate_tokens(&body);
        // (11 + 4 + 9 + 9) / 4 ≈ 8 (role strings counted too)
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_tokens_with_tool_use() {
        let body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": [
                    { "type": "text", "text": "Let me read that file" },
                    { "type": "tool_use", "name": "read_file", "input": { "path": "/tmp/test.rs" } }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "content": "file contents here" }
                ]}
            ]
        });
        let tokens = estimate_tokens(&body);
        assert!(tokens > 0);
    }

    #[test]
    fn test_hash_prefix_stable() {
        let messages: Vec<serde_json::Value> = vec![
            serde_json::json!({ "role": "user", "content": "msg1" }),
            serde_json::json!({ "role": "assistant", "content": "msg2" }),
            serde_json::json!({ "role": "user", "content": "msg3" }),
            serde_json::json!({ "role": "assistant", "content": "msg4" }),
        ];
        let h1 = hash_prefix(&messages, 2);
        let h2 = hash_prefix(&messages, 2);
        assert_eq!(h1, h2); // Same input → same hash
        assert!(!h1.is_empty());
    }

    #[test]
    fn test_hash_prefix_changes_with_different_keep() {
        let messages: Vec<serde_json::Value> = vec![
            serde_json::json!({ "role": "user", "content": "msg1" }),
            serde_json::json!({ "role": "assistant", "content": "msg2" }),
            serde_json::json!({ "role": "user", "content": "msg3" }),
        ];
        let h1 = hash_prefix(&messages, 1);
        let h2 = hash_prefix(&messages, 2);
        assert_ne!(h1, h2); // Different keep_recent → different prefix → different hash
    }

    #[test]
    fn test_cache_hit_miss() {
        let mut cache = CompactionCache::new();
        let key = "claude-sonnet-4:abcdef1234567890";
        let entry = CompactionCacheEntry {
            summary_message: serde_json::json!({ "role": "system", "content": "summary" }),
            compacted_count: 5,
            original_tokens: 100_000,
        };
        cache.insert(key.to_string(), entry);

        assert!(cache.get(key).is_some());
        assert!(cache.get("claude-sonnet-4:wronghash").is_none());
    }

    #[test]
    fn test_replace_prefix_with_summary() {
        let body = serde_json::json!({
            "model": "test",
            "messages": [
                { "role": "user", "content": "old1" },
                { "role": "assistant", "content": "old2" },
                { "role": "user", "content": "old3" },
                { "role": "assistant", "content": "recent1" },
                { "role": "user", "content": "recent2" },
            ]
        });
        let summary = serde_json::json!({ "role": "system", "content": "SUMMARY" });
        let result = replace_prefix_with_summary(body, &summary, 3);

        let messages = result.get("messages").unwrap().as_array().unwrap();
        assert_eq!(messages.len(), 3); // 1 summary + 2 recent
        assert_eq!(messages[0]["content"], "SUMMARY");
        assert_eq!(messages[1]["content"], "recent1");
        assert_eq!(messages[2]["content"], "recent2");
    }

    #[test]
    fn test_fallback_truncate() {
        let body = serde_json::json!({
            "messages": [
                { "role": "user", "content": "m1" },
                { "role": "assistant", "content": "m2" },
                { "role": "user", "content": "m3" },
                { "role": "assistant", "content": "m4" },
                { "role": "user", "content": "m5" },
            ]
        });
        let result = fallback_truncate(body, 2, 1000);
        let messages = result.get("messages").unwrap().as_array().unwrap();
        assert_eq!(messages.len(), 3); // 1 notice + 2 recent
        assert!(messages[0]["content"].as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn test_serialize_messages_for_summary() {
        let messages = vec![
            serde_json::json!({ "role": "user", "content": "Hello" }),
            serde_json::json!({ "role": "assistant", "content": [{ "type": "text", "text": "Hi!" }] }),
        ];
        let text = serialize_messages_for_summary(&messages);
        assert!(text.contains("Hello"));
        assert!(text.contains("Hi!"));
        assert!(text.contains("Message 0"));
        assert!(text.contains("Message 1"));
        assert!(text.contains("[user]"));
        assert!(text.contains("[assistant]"));
    }

    #[test]
    fn test_build_summary_message_openai() {
        let msg = build_summary_message("test summary", Protocol::OpenAI, "gpt-4o");
        assert_eq!(msg["role"], "system");
        assert!(msg["content"].as_str().unwrap().contains("test summary"));
    }

    #[test]
    fn test_build_summary_message_anthropic() {
        let msg = build_summary_message("test summary", Protocol::Anthropic, "claude-4");
        assert_eq!(msg["role"], "user");
        assert_eq!(msg["content"][0]["type"], "text");
        assert!(msg["content"][0]["text"].as_str().unwrap().contains("test summary"));
    }

    #[test]
    fn test_build_summary_request_openai() {
        let (path, body) = build_summary_request("gpt-4o", "conversation", Protocol::OpenAI, 4096);
        assert_eq!(path, "/chat/completions");
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body["messages"][0]["content"].as_str().unwrap().contains("summarizer"));
    }

    #[test]
    fn test_build_summary_request_anthropic() {
        let (path, body) =
            build_summary_request("claude-4", "conversation", Protocol::Anthropic, 4096);
        assert_eq!(path, "/v1/messages");
        assert_eq!(body["model"], "claude-4");
        assert!(body["system"].as_str().unwrap().contains("summarizer"));
    }
}
