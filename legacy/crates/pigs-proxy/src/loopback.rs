//! HTTP loopback transport for protocol-native phase subrequests.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use async_trait::async_trait;
use futures_util::StreamExt;
use pigs_api::protocol::{HttpRequestEnvelope, Protocol};
use pigs_api::transport::{PhaseTransport, TransportError, TransportResponse, TransportTextSink};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{json, Map, Value};

/// Header that marks a verified internal phase subrequest.
pub const INTERNAL_PHASE_HEADER: &str = "x-pigs-phase-loopback";

/// Sends phase subrequests back through the local proxy HTTP handler.
pub struct LoopbackPhaseTransport {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl LoopbackPhaseTransport {
    /// Creates a transport for the configured proxy listen address.
    pub fn new(listen: SocketAddr, token: String) -> Result<Self, TransportError> {
        let connect_addr = match listen.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen.port())
            }
            IpAddr::V6(ip) if ip.is_unspecified() => {
                SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), listen.port())
            }
            _ => listen,
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|error| TransportError::Other(error.to_string()))?;
        Ok(Self {
            client,
            base_url: format!("http://{connect_addr}"),
            token,
        })
    }

    fn request_builder(
        &self,
        request: &HttpRequestEnvelope,
    ) -> Result<reqwest::RequestBuilder, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|error| TransportError::Other(error.to_string()))?;
        let url = format!("{}{}", self.base_url, request.path_and_query);
        let mut builder = self.client.request(method, url);
        for header in &request.headers {
            if is_hop_by_hop(&header.name)
                || header.name.eq_ignore_ascii_case(INTERNAL_PHASE_HEADER)
            {
                continue;
            }
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|error| TransportError::Other(error.to_string()))?;
            let value = HeaderValue::from_bytes(&header.value)
                .map_err(|error| TransportError::Other(error.to_string()))?;
            builder = builder.header(name, value);
        }
        let body = serde_json::to_vec(&request.body)
            .map_err(|error| TransportError::Other(error.to_string()))?;
        Ok(builder
            .header(INTERNAL_PHASE_HEADER, &self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body))
    }
}

#[async_trait]
impl PhaseTransport for LoopbackPhaseTransport {
    async fn send(
        &self,
        request: HttpRequestEnvelope,
    ) -> Result<TransportResponse, TransportError> {
        let response = self
            .request_builder(&request)?
            .send()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?;
        let status = response.status();
        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        let bytes = response
            .bytes()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?;
        if !status.is_success() {
            return Err(TransportError::Http {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        let body = if is_sse {
            let stream = std::str::from_utf8(&bytes).map_err(|error| {
                TransportError::Other(format!("invalid UTF-8 in loopback SSE: {error}"))
            })?;
            parse_sse_response(request.protocol, stream)?
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|error| TransportError::Other(format!("invalid loopback JSON: {error}")))?
        };
        Ok(TransportResponse { body })
    }

    async fn send_streaming(
        &self,
        request: HttpRequestEnvelope,
        text: TransportTextSink,
    ) -> Result<TransportResponse, TransportError> {
        let response = self
            .request_builder(&request)?
            .send()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?;
        let status = response.status();
        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        if !is_sse {
            let bytes = response
                .bytes()
                .await
                .map_err(|error| TransportError::Other(error.to_string()))?;
            if !status.is_success() {
                return Err(TransportError::Http {
                    status: status.as_u16(),
                    message: String::from_utf8_lossy(&bytes).into_owned(),
                });
            }
            let body: Value = serde_json::from_slice(&bytes).map_err(|error| {
                TransportError::Other(format!("invalid loopback JSON: {error}"))
            })?;
            let output = request
                .extract_response(&body)
                .map_err(|error| TransportError::Other(error.to_string()))?;
            if !output.visible_text.is_empty() {
                text(output.visible_text);
            }
            return Ok(TransportResponse { body });
        }

        if !status.is_success() {
            let bytes = response
                .bytes()
                .await
                .map_err(|error| TransportError::Other(error.to_string()))?;
            return Err(TransportError::Http {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }

        let mut stream = response.bytes_stream();
        let mut pending = Vec::new();
        let mut complete = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| TransportError::Other(error.to_string()))?;
            pending.extend_from_slice(&chunk);
            while let Some((frame, consumed)) = take_sse_frame_bytes(&pending)? {
                complete.push_str(&frame);
                complete.push_str("\n\n");
                pending.drain(..consumed);
                if let Some(delta) = visible_delta(request.protocol, &frame)? {
                    if !delta.is_empty() {
                        text(delta);
                    }
                }
            }
        }
        if !pending.is_empty() {
            let remainder = std::str::from_utf8(&pending).map_err(|error| {
                TransportError::Other(format!("invalid UTF-8 in loopback SSE: {error}"))
            })?;
            complete.push_str(remainder);
        }
        let body = parse_sse_response(request.protocol, &complete)?;
        Ok(TransportResponse { body })
    }
}

fn take_sse_frame_bytes(buffer: &[u8]) -> Result<Option<(String, usize)>, TransportError> {
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));
    let (index, separator) = match (lf, crlf) {
        (Some(left), Some(right)) => {
            if left.0 <= right.0 {
                left
            } else {
                right
            }
        }
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => return Ok(None),
    };
    let frame = std::str::from_utf8(&buffer[..index])
        .map_err(|error| TransportError::Other(format!("invalid UTF-8 in loopback SSE: {error}")))?
        .replace('\r', "");
    Ok(Some((frame, index + separator)))
}

fn visible_delta(protocol: Protocol, frame: &str) -> Result<Option<String>, TransportError> {
    let event = frame.lines().find_map(|line| line.strip_prefix("event: "));
    let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
        return Ok(None);
    };
    if data == "[DONE]" {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|error| TransportError::Other(format!("invalid loopback SSE JSON: {error}")))?;
    if value.get("error").is_some()
        || event == Some("error")
        || event == Some("response.failed")
        || value.get("type").and_then(Value::as_str) == Some("response.failed")
    {
        return Err(TransportError::Other(format!(
            "loopback SSE error: {value}"
        )));
    }
    let delta = match protocol {
        Protocol::OpenAiChat => value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str),
        Protocol::AnthropicMessages => value
            .get("delta")
            .filter(|_| {
                event == Some("content_block_delta")
                    || value.get("type").and_then(Value::as_str) == Some("content_block_delta")
            })
            .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("text_delta"))
            .and_then(|delta| delta.get("text"))
            .and_then(Value::as_str),
        Protocol::OpenAiResponses => {
            if event == Some("response.output_text.delta")
                || value.get("type").and_then(Value::as_str) == Some("response.output_text.delta")
            {
                value.get("delta").and_then(Value::as_str)
            } else {
                None
            }
        }
    };
    Ok(delta.map(str::to_owned))
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "connection" | "transfer-encoding"
    )
}

fn parse_sse_response(protocol: Protocol, stream: &str) -> Result<Value, TransportError> {
    let normalized;
    let stream = if stream.contains('\r') {
        normalized = stream.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        stream
    };
    if protocol == Protocol::OpenAiChat && !stream.lines().any(|line| line.trim() == "data: [DONE]")
    {
        return Err(TransportError::Other(
            "loopback Chat stream ended without [DONE]".into(),
        ));
    }
    let events = parse_sse_events(stream)?;
    match protocol {
        Protocol::OpenAiChat => parse_chat_sse(&events),
        Protocol::AnthropicMessages => parse_anthropic_sse(&events),
        Protocol::OpenAiResponses => parse_responses_sse(&events),
    }
}

fn parse_sse_events(stream: &str) -> Result<Vec<(Option<String>, Value)>, TransportError> {
    let mut events = Vec::new();
    for frame in stream.split("\n\n") {
        let event = frame
            .lines()
            .find_map(|line| line.strip_prefix("event: "))
            .map(str::to_owned);
        let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        let value: Value = serde_json::from_str(data).map_err(|error| {
            TransportError::Other(format!("invalid loopback SSE JSON: {error}"))
        })?;
        if value.get("error").is_some() || event.as_deref() == Some("error") {
            return Err(TransportError::Other(format!(
                "loopback SSE error: {value}"
            )));
        }
        events.push((event, value));
    }
    if events.is_empty() {
        return Err(TransportError::Other(
            "loopback SSE contained no JSON events".into(),
        ));
    }
    Ok(events)
}

fn parse_chat_sse(events: &[(Option<String>, Value)]) -> Result<Value, TransportError> {
    let mut text = String::new();
    let mut role = "assistant".to_string();
    let mut calls: Vec<Value> = Vec::new();
    let mut message_extensions = Map::new();
    let mut finish_reason = Value::Null;
    let mut usage = None;
    let mut id = None;
    for (_, event) in events {
        id = id.or_else(|| event.get("id").cloned());
        usage = event.get("usage").cloned().or(usage);
        let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
        else {
            continue;
        };
        if let Some(value) = choice
            .get("delta")
            .and_then(|delta| delta.get("role"))
            .and_then(Value::as_str)
        {
            role = value.to_owned();
        }
        if let Some(value) = choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
        {
            text.push_str(value);
        }
        if let Some(tool_deltas) = choice
            .get("delta")
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(Value::as_array)
        {
            merge_chat_tool_deltas(&mut calls, tool_deltas);
        }
        if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
            for (key, value) in delta {
                if !matches!(key.as_str(), "role" | "content" | "tool_calls") {
                    match (message_extensions.get_mut(key), value) {
                        (Some(Value::String(current)), Value::String(addition)) => {
                            current.push_str(addition);
                        }
                        _ => {
                            message_extensions.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }
        if choice
            .get("finish_reason")
            .is_some_and(|value| !value.is_null())
        {
            finish_reason = choice["finish_reason"].clone();
        }
    }
    if finish_reason.is_null() {
        return Err(TransportError::Other(
            "loopback Chat stream ended without finish_reason".into(),
        ));
    }
    let mut message = json!({"role": role, "content": text});
    if let Some(message) = message.as_object_mut() {
        message.extend(message_extensions);
    }
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    Ok(json!({
        "id": id.unwrap_or_else(|| json!("chatcmpl_loopback")),
        "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
        "usage": usage.unwrap_or_else(|| json!({}))
    }))
}

fn merge_chat_tool_deltas(calls: &mut Vec<Value>, deltas: &[Value]) {
    for delta in deltas {
        let index = delta.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        while calls.len() <= index {
            calls.push(json!({
                "id": "", "type": "function",
                "function": {"name": "", "arguments": ""}
            }));
        }
        let target = &mut calls[index];
        append_string(target, delta, "id");
        if let Some(function) = delta.get("function") {
            append_nested_string(target, function, "function", "name");
            append_nested_string(target, function, "function", "arguments");
            if let (Some(target), Some(function)) = (
                target.get_mut("function").and_then(Value::as_object_mut),
                function.as_object(),
            ) {
                for (key, value) in function {
                    if !matches!(key.as_str(), "name" | "arguments") {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        if let (Some(target), Some(delta)) = (target.as_object_mut(), delta.as_object()) {
            for (key, value) in delta {
                if !matches!(key.as_str(), "index" | "id" | "function") {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

fn append_string(target: &mut Value, source: &Value, key: &str) {
    let Some(addition) = source.get(key).and_then(Value::as_str) else {
        return;
    };
    let entry = target
        .as_object_mut()
        .and_then(|object| object.get_mut(key))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned()
        + addition;
    target[key] = Value::String(entry);
}

fn append_nested_string(target: &mut Value, source: &Value, parent: &str, key: &str) {
    let Some(addition) = source.get(key).and_then(Value::as_str) else {
        return;
    };
    let current = target
        .get(parent)
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or("");
    target[parent][key] = Value::String(format!("{current}{addition}"));
}

fn append_block_string(target: &mut Value, delta: &Value, key: &str) {
    let Some(addition) = delta.get(key).and_then(Value::as_str) else {
        return;
    };
    let current = target.get(key).and_then(Value::as_str).unwrap_or("");
    target[key] = Value::String(format!("{current}{addition}"));
}

fn parse_anthropic_sse(events: &[(Option<String>, Value)]) -> Result<Value, TransportError> {
    let mut content: Vec<Value> = Vec::new();
    let mut id = json!("msg_loopback");
    let mut model = Value::Null;
    let mut stop_reason = Value::Null;
    let mut usage = Map::new();
    let mut tool_json: Vec<String> = Vec::new();
    let mut open_blocks: Vec<bool> = Vec::new();
    let mut saw_message_start = false;
    let mut saw_message_stop = false;
    for (event_name, event) in events {
        match event_name
            .as_deref()
            .or_else(|| event.get("type").and_then(Value::as_str))
        {
            Some("message_start") => {
                saw_message_start = true;
                if let Some(message) = event.get("message") {
                    id = message.get("id").cloned().unwrap_or(id);
                    model = message.get("model").cloned().unwrap_or(model);
                    if let Some(value) = message.get("usage").and_then(Value::as_object) {
                        usage.extend(value.clone());
                    }
                }
            }
            Some("content_block_start") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while content.len() <= index {
                    content.push(Value::Null);
                    tool_json.push(String::new());
                    open_blocks.push(false);
                }
                if open_blocks[index] {
                    return Err(TransportError::Other(format!(
                        "duplicate Anthropic content_block_start at index {index}"
                    )));
                }
                content[index] = event.get("content_block").cloned().unwrap_or(Value::Null);
                open_blocks[index] = true;
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let Some(block) = content.get_mut(index) else {
                    return Err(TransportError::Other(format!(
                        "Anthropic content_block_delta references unknown index {index}"
                    )));
                };
                let Some(delta) = event.get("delta") else {
                    continue;
                };
                if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                    let addition = delta.get("text").and_then(Value::as_str).unwrap_or("");
                    let current = block.get("text").and_then(Value::as_str).unwrap_or("");
                    block["text"] = Value::String(format!("{current}{addition}"));
                } else if delta.get("type").and_then(Value::as_str) == Some("thinking_delta") {
                    append_block_string(block, delta, "thinking");
                } else if delta.get("type").and_then(Value::as_str) == Some("signature_delta") {
                    append_block_string(block, delta, "signature");
                } else if delta.get("type").and_then(Value::as_str) == Some("input_json_delta") {
                    let partial = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let Some(buffer) = tool_json.get_mut(index) else {
                        return Err(TransportError::Other(format!(
                            "Anthropic input_json_delta references unknown index {index}"
                        )));
                    };
                    if !partial.is_empty() {
                        buffer.push_str(partial);
                    }
                }
            }
            Some("content_block_stop") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let Some(block) = content.get_mut(index) else {
                    return Err(TransportError::Other(format!(
                        "Anthropic content_block_stop references unknown index {index}"
                    )));
                };
                let Some(is_open) = open_blocks.get_mut(index) else {
                    return Err(TransportError::Other(format!(
                        "Anthropic content_block_stop references unknown index {index}"
                    )));
                };
                if !*is_open {
                    return Err(TransportError::Other(format!(
                        "duplicate Anthropic content_block_stop at index {index}"
                    )));
                }
                *is_open = false;
                if let Some(partial) = tool_json.get(index).filter(|value| !value.is_empty()) {
                    block["input"] = serde_json::from_str(partial).map_err(|error| {
                        TransportError::Other(format!(
                            "invalid Anthropic tool input JSON at content index {index}: {error}"
                        ))
                    })?;
                }
            }
            Some("message_delta") => {
                stop_reason = event
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .cloned()
                    .unwrap_or(stop_reason);
                if let Some(value) = event.get("usage").and_then(Value::as_object) {
                    usage.extend(value.clone());
                }
            }
            Some("message_stop") => saw_message_stop = true,
            _ => {}
        }
    }
    if !saw_message_start {
        return Err(TransportError::Other(
            "loopback Anthropic stream ended without message_start".into(),
        ));
    }
    if open_blocks.iter().any(|is_open| *is_open) {
        return Err(TransportError::Other(
            "loopback Anthropic stream ended with an open content block".into(),
        ));
    }
    if stop_reason.is_null() {
        return Err(TransportError::Other(
            "loopback Anthropic stream ended without stop_reason".into(),
        ));
    }
    if !saw_message_stop {
        return Err(TransportError::Other(
            "loopback Anthropic stream ended without message_stop".into(),
        ));
    }
    content.retain(|value| !value.is_null());
    Ok(json!({
        "id": id, "type": "message", "role": "assistant", "model": model,
        "content": content, "stop_reason": stop_reason, "usage": usage
    }))
}

fn parse_responses_sse(events: &[(Option<String>, Value)]) -> Result<Value, TransportError> {
    for (event_name, event) in events.iter().rev() {
        if event_name.as_deref() == Some("response.failed")
            || event.get("type").and_then(Value::as_str) == Some("response.failed")
        {
            return Err(TransportError::Other(format!(
                "loopback response failed: {event}"
            )));
        }
        if event_name.as_deref() == Some("response.completed")
            || event.get("type").and_then(Value::as_str) == Some("response.completed")
        {
            return event.get("response").cloned().ok_or_else(|| {
                TransportError::Other("response.completed has no response object".into())
            });
        }
    }
    Err(TransportError::Other(
        "loopback Responses stream ended without response.completed".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(event: &str, value: Value) -> String {
        format!("event: {event}\ndata: {value}\n\n")
    }

    #[test]
    fn byte_framing_preserves_utf8_split_across_network_chunks() {
        let frame = format!("data: {}\r\n\r\n", json!({"text": "执行"}));
        let bytes = frame.as_bytes();
        let split = bytes
            .windows("执".len())
            .position(|window| window == "执".as_bytes())
            .unwrap()
            + 1;
        let mut pending = bytes[..split].to_vec();
        assert!(take_sse_frame_bytes(&pending).unwrap().is_none());
        pending.extend_from_slice(&bytes[split..]);
        let (decoded, consumed) = take_sse_frame_bytes(&pending).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert!(decoded.contains("执行"));
        assert!(!decoded.contains('\r'));
    }

    #[test]
    fn chat_sse_accepts_crlf_frames() {
        let stream = format!(
            "data: {}\r\n\r\ndata: {}\r\n\r\ndata: [DONE]\r\n\r\n",
            json!({
                "id": "chatcmpl_1",
                "choices": [{"index": 0, "delta": {"content": "完成"}, "finish_reason": null}]
            }),
            json!({
                "id": "chatcmpl_1",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            })
        );

        let response = parse_sse_response(Protocol::OpenAiChat, &stream).unwrap();
        assert_eq!(response["choices"][0]["message"]["content"], "完成");
    }

    #[test]
    fn chat_sse_requires_done_sentinel() {
        let stream = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({
                "id": "chatcmpl_1",
                "choices": [{"index": 0, "delta": {"content": "done"}, "finish_reason": null}]
            }),
            json!({
                "id": "chatcmpl_1",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            })
        );

        assert!(matches!(
            parse_sse_response(Protocol::OpenAiChat, &stream),
            Err(TransportError::Other(message)) if message.contains("[DONE]")
        ));
    }

    #[test]
    fn malformed_streams_return_errors_instead_of_success() {
        let anthropic = [
            event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": "orphan"}
                }),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
        .join("");
        assert!(matches!(
            parse_sse_response(Protocol::AnthropicMessages, &anthropic),
            Err(TransportError::Other(message)) if message.contains("unknown index")
        ));

        let no_stop = [
            event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "model": "claude", "usage": {}}
                }),
            ),
            event(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {}
                }),
            ),
        ]
        .join("");
        assert!(matches!(
            parse_sse_response(Protocol::AnthropicMessages, &no_stop),
            Err(TransportError::Other(message)) if message.contains("message_stop")
        ));

        let open_block = [
            event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "model": "claude", "usage": {}}
                }),
            ),
            event(
                "content_block_start",
                json!({
                    "type": "content_block_start", "index": 0,
                    "content_block": {"type": "text", "text": ""}
                }),
            ),
            event(
                "message_delta",
                json!({
                    "type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {}
                }),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
        .join("");
        assert!(matches!(
            parse_sse_response(Protocol::AnthropicMessages, &open_block),
            Err(TransportError::Other(message)) if message.contains("open content block")
        ));

        let responses = event(
            "response.created",
            json!({
                "type": "response.created",
                "response": {"id": "resp_1", "status": "in_progress"}
            }),
        );
        assert!(matches!(
            parse_sse_response(Protocol::OpenAiResponses, &responses),
            Err(TransportError::Other(message)) if message.contains("response.completed")
        ));
    }

    #[test]
    fn chat_sse_preserves_message_and_function_extensions() {
        let stream = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "id": "chatcmpl_1",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "reasoning_content": "think ",
                        "provider_message": {"trace": "kept"},
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "run",
                                "arguments": "{\"x\":" ,
                                "provider_function": {"strict": true}
                            }
                        }]
                    },
                    "finish_reason": null
                }]
            }),
            json!({
                "id": "chatcmpl_1",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "reasoning_content": "more",
                        "tool_calls": [{
                            "index": 0,
                            "function": {"arguments": "1}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
        );

        let response = parse_sse_response(Protocol::OpenAiChat, &stream).unwrap();
        let message = &response["choices"][0]["message"];
        assert_eq!(message["provider_message"], json!({"trace": "kept"}));
        assert_eq!(message["reasoning_content"], "think more");
        assert_eq!(
            message["tool_calls"][0]["function"]["provider_function"],
            json!({"strict": true})
        );
        assert_eq!(
            message["tool_calls"][0]["function"]["arguments"],
            "{\"x\":1}"
        );
    }

    #[test]
    fn anthropic_sse_preserves_thinking_and_signature_deltas() {
        let stream = [
            event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "model": "claude", "usage": {}}
                }),
            ),
            event(
                "content_block_start",
                json!({
                    "type": "content_block_start", "index": 0,
                    "content_block": {"type": "thinking", "thinking": "", "signature": ""}
                }),
            ),
            event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "thinking_delta", "thinking": "private"}
                }),
            ),
            event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "signature_delta", "signature": "signed"}
                }),
            ),
            event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
            event(
                "message_delta",
                json!({
                    "type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {}
                }),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
        .join("");

        let response = parse_sse_response(Protocol::AnthropicMessages, &stream).unwrap();
        assert_eq!(response["content"][0]["thinking"], "private");
        assert_eq!(response["content"][0]["signature"], "signed");
    }

    #[test]
    fn anthropic_sse_accumulates_split_tool_input_json() {
        let stream = [
            event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "model": "claude", "usage": {}}
                }),
            ),
            event(
                "content_block_start",
                json!({
                    "type": "content_block_start", "index": 0,
                    "content_block": {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {}}
                }),
            ),
            event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}
                }),
            ),
            event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": "\"a\"}"}
                }),
            ),
            event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
            event(
                "message_delta",
                json!({
                    "type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {}
                }),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
        .join("");

        let response = parse_sse_response(Protocol::AnthropicMessages, &stream).unwrap();
        assert_eq!(response["content"][0]["input"], json!({"path": "a"}));
    }
}
