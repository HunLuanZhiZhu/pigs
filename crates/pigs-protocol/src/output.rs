//! 将上游 JSON/SSE 响应归一化为单轮模型输出。 / Normalize upstream JSON/SSE responses into a single model turn.
//!
//! 归一化用于编排，不改写业务内容或原生工具结构。 / Normalization serves orchestration without rewriting payload semantics.
//! 停止原因及 usage 保留原值。 / Preserve stop reasons and usage values.

use crate::route::Protocol;
use serde_json::{json, Value};

/// 保留原生表示的工具调用。 / Tool call retaining its native representation.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// 关联工具结果的标识。 / ID linking a tool call to its result.
    pub id: String,
    /// 工具名。 / Tool name.
    pub name: String,
    /// 原生参数值，不强制解析字符串。 / Native arguments; string arguments stay strings.
    pub arguments: Value,
    /// 完整原生工具调用块，供客户端响应复用。 / Full native tool-call block reused in client output.
    pub native: Value,
}

impl ToolCall {
    pub fn arguments_json(&self) -> String {
        match &self.arguments {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

/// 有序的上游内容片段。 / Ordered upstream content part.
///
/// 编排仅解释文本和工具调用；其它片段保持原生形式。 / Orchestration interprets text/tool calls while preserving other native parts.
/// 包括思考、推理和搜索调用等。 / Includes reasoning and search-related items.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// 用户可见文本。 / User-visible text.
    Text(String),
    /// 协议原生思考字段。 / Protocol-native reasoning text.
    Reasoning(String),
    /// 原生工具调用。 / Native tool call.
    ToolCall(ToolCall),
    /// 其它原生输出内容，不解释。 / Other native output items, passed through unchanged.
    Native(Value),
}

/// 供编排使用的单轮模型输出。 / One model turn normalized for orchestration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelOutput {
    /// 拼接后的可见文本，可能包含控制标记。 / Joined visible text, possibly including control markers.
    pub text: String,
    /// 本轮工具调用列表；空表示无调用。 / Tool calls requested in this turn; empty means none.
    pub tool_calls: Vec<ToolCall>,
    /// 客户端内容的权威有序序列。 / Authoritative ordered parts for client output.
    pub parts: Vec<Part>,
    /// 原生停止原因。 / Native stop reason.
    pub stop_reason: Option<String>,
    /// 未改写的原生 usage 对象。 / Unmodified native usage object.
    pub usage: Option<Value>,
}

impl ModelOutput {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.tool_calls.is_empty()
    }

    /// 上游报告生成达到 token 上限。 / Upstream indicates truncation at the token limit.
    pub fn is_truncated(&self) -> bool {
        matches!(
            self.stop_reason.as_deref(),
            Some("length" | "max_tokens" | "max_output_tokens")
        )
    }
}

/// 解析非流式 JSON 响应。 / Parse a non-streaming JSON response.
pub fn parse_json_output(protocol: Protocol, body: &Value) -> ModelOutput {
    match protocol {
        Protocol::OpenAI => parse_chat_json(body),
        Protocol::Anthropic => parse_anthropic_json(body),
        Protocol::Responses => parse_responses_json(body),
    }
}

/// 汇总 SSE 全文，不重复转发增量。 / Summarize an SSE transcript after deltas have already been streamed.
pub fn parse_sse_output(protocol: Protocol, sse: &str) -> ModelOutput {
    let mut acc = SseOutputAccumulator::new(protocol);
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim_start();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        acc.push(&value);
    }
    acc.finish()
}

// ---------------------------------------------------------------- 非流式 / Non-streaming

fn parse_chat_json(body: &Value) -> ModelOutput {
    let choice = body.get("choices").and_then(|c| c.get(0));
    let message = choice.and_then(|c| c.get("message"));
    let text = message
        .and_then(|m| m.get("content"))
        .map(join_text_content)
        .unwrap_or_default();
    let tool_calls: Vec<ToolCall> = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|t| t.as_array())
        .map(|calls| calls.iter().map(chat_tool_call).collect())
        .unwrap_or_default();
    let mut parts = Vec::new();
    if let Some(reasoning) = message.and_then(chat_reasoning_text) {
        parts.push(Part::Reasoning(reasoning));
    }
    if !text.is_empty() {
        parts.push(Part::Text(text.clone()));
    }
    parts.extend(tool_calls.iter().cloned().map(Part::ToolCall));
    ModelOutput {
        text,
        tool_calls,
        parts,
        stop_reason: choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|r| r.as_str())
            .map(String::from),
        usage: body.get("usage").cloned(),
    }
}

fn parse_anthropic_json(body: &Value) -> ModelOutput {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut parts = Vec::new();
    if let Some(blocks) = body.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    let piece = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !piece.is_empty() {
                        text.push_str(piece);
                        parts.push(Part::Text(piece.to_string()));
                    }
                }
                Some("tool_use") => {
                    let call = ToolCall {
                    id: block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                        arguments: block.get("input").cloned().unwrap_or_else(|| json!({})),
                        native: block.clone(),
                    };
                    tool_calls.push(call.clone());
                    parts.push(Part::ToolCall(call));
                }
                // 思考及服务端工具块不做解释。 / Preserve reasoning and server-tool blocks unchanged.
                _ => parts.push(Part::Native(block.clone())),
            }
        }
    }
    ModelOutput {
        text,
        tool_calls,
        parts,
        stop_reason: body
            .get("stop_reason")
            .and_then(|r| r.as_str())
            .map(String::from),
        usage: body.get("usage").cloned(),
    }
}

fn parse_responses_json(body: &Value) -> ModelOutput {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut parts = Vec::new();
    if let Some(items) = body.get("output").and_then(|o| o.as_array()) {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    let piece = responses_item_text(item);
                    if !piece.is_empty() {
                        text.push_str(&piece);
                        parts.push(Part::Text(piece));
                    }
                }
                Some("function_call") => {
                    let call = responses_tool_call(item);
                    tool_calls.push(call.clone());
                    parts.push(Part::ToolCall(call));
                }
                // 推理与外部调用项保留原样。 / Preserve reasoning and external-call items unchanged.
                _ => parts.push(Part::Native(item.clone())),
            }
        }
    }
    ModelOutput {
        text,
        tool_calls,
        parts,
        // Responses 截断原因位于 incomplete_details。 / Responses reports truncation through incomplete_details.
        stop_reason: body
            .pointer("/incomplete_details/reason")
            .and_then(|r| r.as_str())
            .map(String::from),
        usage: body.get("usage").cloned(),
    }
}

/// 从字符串或文本块数组提取文本。 / Extract text from a string or text-block array.
fn join_text_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// 提取 Chat 思考字段（多种字段名）。 / Extract Chat reasoning across provider-specific field names.
fn chat_reasoning_text(message: &Value) -> Option<String> {
    ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|key| {
            message
                .get(*key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
}

/// 拼接 Responses 消息中的 output_text。 / Join output_text segments in Responses messages.
fn responses_item_text(item: &Value) -> String {
    item.get("content")
        .and_then(|c| c.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn chat_tool_call(call: &Value) -> ToolCall {
    let function = call.get("function");
    ToolCall {
        id: call
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: function
            .and_then(|f| f.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        arguments: function
            .and_then(|f| f.get("arguments"))
            .cloned()
            .unwrap_or_else(|| Value::String("{}".into())),
        native: call.clone(),
    }
}

fn responses_tool_call(item: &Value) -> ToolCall {
    ToolCall {
        id: item
            .get("call_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        arguments: item
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::String("{}".into())),
        native: item.clone(),
    }
}

// ---------------------------------------------------------------- SSE 汇总 / SSE aggregation

/// 将 SSE 事件聚合为完整模型轮次。 / Aggregate SSE events into a complete model turn.
struct SseOutputAccumulator {
    protocol: Protocol,
    output: ModelOutput,
    /// Chat 按索引拼接工具参数。 / Chat: assemble tool arguments by index.
    chat_calls: Vec<ToolCall>,
    /// Chat 累积的思考文本。 / Chat: accumulated reasoning text.
    chat_reasoning: String,
    /// Anthropic 按内容块索引追踪状态。 / Anthropic: track blocks by content index.
    anthropic_blocks: Vec<AnthropicBlock>,
    /// Responses 按条目标识拼接函数参数。 / Responses: accumulate function-call arguments by item ID.
    responses_calls: Vec<ToolCall>,
}

enum AnthropicBlock {
    /// 累积增量的文本块。 / Text block accumulating deltas.
    Text { buf: String },
    /// 累积思考增量及签名。 / Reasoning block collecting deltas and signatures.
    Thinking { buf: String, signature: String },
    /// 工具调用块。 / Tool-call block.
    ToolUse,
    /// 其它块保留原始起始帧。 / Preserve the original start frame for other block types.
    Other { native: Value },
}

impl SseOutputAccumulator {
    fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            output: ModelOutput::default(),
            chat_calls: Vec::new(),
            chat_reasoning: String::new(),
            anthropic_blocks: Vec::new(),
            responses_calls: Vec::new(),
        }
    }

    fn push(&mut self, event: &Value) {
        match self.protocol {
            Protocol::OpenAI => self.push_chat(event),
            Protocol::Anthropic => self.push_anthropic(event),
            Protocol::Responses => self.push_responses(event),
        }
    }

    fn push_chat(&mut self, event: &Value) {
        if event.get("usage").is_some() {
            self.output.usage = event.get("usage").cloned();
        }
        let Some(choice) = event.get("choices").and_then(|c| c.get(0)) else {
            return;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            self.output.stop_reason = Some(reason.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };
        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            self.output.text.push_str(content);
        }
        if let Some(reasoning) = chat_reasoning_text(delta) {
            self.chat_reasoning.push_str(&reasoning);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for call in calls {
                let index = call
                    .get("index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(self.chat_calls.len() as u64) as usize;
                while self.chat_calls.len() <= index {
                    self.chat_calls.push(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: Value::String(String::new()),
                        native: json!({"type": "function"}),
                    });
                }
                let slot = &mut self.chat_calls[index];
                if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                    slot.id = id.to_string();
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(|v| v.as_str()) {
                        slot.name.push_str(name);
                    }
                    if let Some(part) = function.get("arguments").and_then(|v| v.as_str()) {
                        if let Value::String(args) = &mut slot.arguments {
                            args.push_str(part);
                        }
                    }
                }
            }
        }
    }

    fn push_anthropic(&mut self, event: &Value) {
        match event.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.output.usage = Some(usage.clone());
                }
            }
            Some("content_block_start") => {
                let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                let kind = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                while self.anthropic_blocks.len() <= index {
                    self.anthropic_blocks.push(AnthropicBlock::Text { buf: String::new() });
                }
                if kind == "thinking" {
                    self.anthropic_blocks[index] = AnthropicBlock::Thinking {
                        buf: String::new(),
                        signature: String::new(),
                    };
                    return;
                }
                if kind == "tool_use" {
                    self.anthropic_blocks[index] = AnthropicBlock::ToolUse;
                    // 起始帧通常只有空参数，后续增量再拼接。 / Start frames often have empty input; later deltas supply arguments.
                    // 若起始帧已提供参数则原样保留。 / Preserve complete arguments already present in the start frame.
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    let arguments = match &input {
                        Value::Object(map) if !map.is_empty() => input.clone(),
                        _ => Value::String(String::new()),
                    };
                    self.output.tool_calls.push(ToolCall {
                        id: block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        name: block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        arguments,
                        native: block,
                    });
                } else if kind == "text" {
                    self.anthropic_blocks[index] = AnthropicBlock::Text { buf: String::new() };
                } else {
                    // 未知块保留原样。 / Preserve unknown blocks unchanged.
                    self.anthropic_blocks[index] = AnthropicBlock::Other { native: block };
                }
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                            self.output.text.push_str(t);
                            self.anthropic_block_mut_text(index, t);
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                            self.anthropic_block_mut_thinking(index, |block| {
                                if let AnthropicBlock::Thinking { buf, .. } = block {
                                    buf.push_str(t);
                                }
                            });
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(t) = delta.get("signature").and_then(|t| t.as_str()) {
                            self.anthropic_block_mut_thinking(index, |block| {
                                if let AnthropicBlock::Thinking { signature, .. } = block {
                                    signature.push_str(t);
                                }
                            });
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(part) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            if let Some(call) = self.tool_call_at_block(index) {
                                if let Value::String(args) = &mut call.arguments {
                                    args.push_str(part);
                                }
                            }
                        }
                    }
                    // 思考与签名增量不进入可见文本。 / Reasoning/signature deltas do not enter visible text.
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(|r| r.as_str()) {
                    self.output.stop_reason = Some(reason.to_string());
                }
                if let Some(usage) = event.get("usage") {
                    self.output.usage = Some(merge_usage(self.output.usage.take(), usage));
                }
            }
            _ => {}
        }
    }

    /// 向指定索引的文本块追加内容。 / Append to the text block at an index.
    fn anthropic_block_mut_text(&mut self, index: usize, text: &str) {
        if let Some(AnthropicBlock::Text { buf }) = self.anthropic_blocks.get_mut(index) {
            buf.push_str(text);
        }
    }

    /// 修改指定索引的思考块。 / Update the reasoning block at an index.
    fn anthropic_block_mut_thinking<F: FnOnce(&mut AnthropicBlock)>(
        &mut self,
        index: usize,
        edit: F,
    ) {
        if let Some(block @ AnthropicBlock::Thinking { .. }) = self.anthropic_blocks.get_mut(index) {
            edit(block);
        }
    }

    /// 按内容块索引查找工具调用。 / Find a tool call by content-block index.
    fn tool_call_at_block(&mut self, index: usize) -> Option<&mut ToolCall> {
        let mut seen = 0usize;
        for (block_index, block) in self.anthropic_blocks.iter().enumerate() {
            if matches!(block, AnthropicBlock::ToolUse) {
                if block_index == index {
                    return self.output.tool_calls.get_mut(seen);
                }
                seen += 1;
            }
        }
        None
    }

    fn push_responses(&mut self, event: &Value) {
        match event.get("type").and_then(|t| t.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                    self.output.text.push_str(delta);
                }
            }
            Some("response.output_item.added") => {
                let item = event.get("item").cloned().unwrap_or(Value::Null);
                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    // 参数可能一次给全，也可能逐帧给出。 / Arguments may arrive all at once or in deltas.
                    self.responses_calls.push(responses_tool_call(&item));
                    if let Some(call) = self.responses_calls.last_mut() {
                        if call.arguments.is_null() {
                            call.arguments = Value::String(String::new());
                        }
                    }
                }
            }
            Some("response.function_call_arguments.delta") => {
                if let Some(part) = event.get("delta").and_then(|d| d.as_str()) {
                    if let Some(call) = self.responses_call_mut(event) {
                        if let Value::String(args) = &mut call.arguments {
                            args.push_str(part);
                        }
                    }
                }
            }
            Some("response.completed") => {
                if let Some(response) = event.get("response") {
                    self.output.usage = response.get("usage").cloned();
                    if let Some(reason) = response
                        .pointer("/incomplete_details/reason")
                        .and_then(|r| r.as_str())
                    {
                        self.output.stop_reason = Some(reason.to_string());
                    }
                    // 最终 output[] 为权威内容序列。 / Final output[] is the authoritative ordered content.
                    if let Some(items) = response.get("output").and_then(|o| o.as_array()) {
                        if !items.is_empty() {
                            let mut rebuilt = parse_responses_json(&json!({"output": items}));
                            rebuilt.usage = self.output.usage.clone();
                            rebuilt.stop_reason = self.output.stop_reason.clone();
                            self.output = rebuilt;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn responses_call_mut(&mut self, event: &Value) -> Option<&mut ToolCall> {
        let item_id = event.get("item_id").and_then(|v| v.as_str());
        self.responses_calls
            .iter_mut()
            .find(|call| match item_id {
                Some(id) => call.native.get("id").and_then(|v| v.as_str()) == Some(id),
                None => true,
            })
    }

    fn finish(mut self) -> ModelOutput {
        match self.protocol {
            Protocol::OpenAI => {
                self.output.tool_calls =
                    self.chat_calls.into_iter().map(finish_chat_call).collect();
                let mut parts = Vec::new();
                if !self.chat_reasoning.is_empty() {
                    parts.push(Part::Reasoning(self.chat_reasoning.clone()));
                }
                if !self.output.text.is_empty() {
                    parts.push(Part::Text(self.output.text.clone()));
                }
                parts.extend(self.output.tool_calls.iter().cloned().map(Part::ToolCall));
                self.output.parts = parts;
            }
            Protocol::Responses if !self.responses_calls.is_empty() => {
                self.output.tool_calls = self
                    .responses_calls
                    .into_iter()
                    .map(|mut call| {
                        // 将拼接参数写回原生对象。 / Write assembled arguments back to the native object.
                        let arguments = call.arguments_json();
                        if let Some(obj) = call.native.as_object_mut() {
                            obj.insert("arguments".into(), Value::String(arguments));
                        }
                        call
                    })
                    .collect();
            }
            Protocol::Anthropic => {
                // 完成工具参数拼接并写回。 / Finalize tool arguments in the native block.
                for call in &mut self.output.tool_calls {
                    let raw = call.arguments_json();
                    let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = call.native.as_object_mut() {
                        obj.insert("input".into(), parsed.clone());
                    }
                    call.arguments = parsed;
                }
                // 按内容块顺序构造客户端响应。 / Build client parts in content-block order.
                let mut calls = self.output.tool_calls.iter().cloned();
                let mut parts = Vec::new();
                for block in &self.anthropic_blocks {
                    match block {
                        AnthropicBlock::Text { buf } => {
                            if !buf.is_empty() {
                                parts.push(Part::Text(buf.clone()));
                            }
                        }
                        AnthropicBlock::Thinking { buf, signature } => {
                            parts.push(Part::Native(json!({
                                "type": "thinking",
                                "thinking": buf,
                                "signature": signature
                            })))
                        }
                        AnthropicBlock::ToolUse => {
                            if let Some(call) = calls.next() {
                                parts.push(Part::ToolCall(call));
                            }
                        }
                        AnthropicBlock::Other { native } => {
                            parts.push(Part::Native(native.clone()))
                        }
                    }
                }
                self.output.parts = parts;
            }
            _ => {}
        }
        self.output
    }
}

/// 将 Chat 流式工具参数合并到原生对象。 / Merge streamed Chat tool arguments into the native object.
fn finish_chat_call(mut call: ToolCall) -> ToolCall {
    let arguments = call.arguments_json();
    call.native = json!({
        "id": call.id,
        "type": "function",
        "function": {"name": call.name, "arguments": arguments}
    });
    call.arguments = Value::String(arguments);
    call
}

/// 浅合并 usage；后者覆盖同名字段，不在此求和。 / Shallow-merge usage, preferring newer fields without summation.
fn merge_usage(previous: Option<Value>, next: &Value) -> Value {
    match previous {
        Some(Value::Object(mut base)) => {
            if let Some(extra) = next.as_object() {
                for (key, value) in extra {
                    base.insert(key.clone(), value.clone());
                }
            }
            Value::Object(base)
        }
        _ => next.clone(),
    }
}

// ---------------------------------------------------------------- 恢复判定 / Resume detection

/// 识别末尾工具结果并返回其 ID 列表。 / Detect trailing tool results and return their IDs.
///
/// 从尾部倒查连续工具结果，遇到其它消息即停止。 / Scan backward through consecutive tool results only.
/// 空列表表示非工具恢复请求。 / Empty means not a tool-resume request.
pub fn all_tool_result_ids(protocol: Protocol, body: &Value) -> Vec<String> {
    match protocol {
        Protocol::OpenAI => all_ids(body.get("messages"), |message| {
            (message.get("role").and_then(|r| r.as_str()) == Some("tool"))
                .then(|| message.get("tool_call_id").and_then(|v| v.as_str()))
                .flatten()
                .map(|id| vec![id.to_string()])
        }),
        Protocol::Anthropic => all_ids(body.get("messages"), |message| {
            if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                return None;
            }
            let parts = message.get("content")?.as_array()?;
            let ids: Vec<String> = parts
                .iter()
                .filter_map(|part| {
                    (part.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                        .then(|| part.get("tool_use_id").and_then(|v| v.as_str()))
                        .flatten()
                        .map(String::from)
                })
                .collect();
            (!ids.is_empty()).then_some(ids)
        }),
        Protocol::Responses => all_ids(body.get("input"), |item| {
            (item.get("type").and_then(|t| t.as_str()) == Some("function_call_output"))
                .then(|| item.get("call_id").and_then(|v| v.as_str()))
                .flatten()
                .map(|id| vec![id.to_string()])
        }),
    }
}

pub fn trailing_tool_result_ids(protocol: Protocol, body: &Value) -> Vec<String> {
    match protocol {
        Protocol::OpenAI => trailing_ids(body.get("messages"), |message| {
            (message.get("role").and_then(|r| r.as_str()) == Some("tool"))
                .then(|| message.get("tool_call_id").and_then(|v| v.as_str()))
                .flatten()
                .map(|id| vec![id.to_string()])
        }),
        // Anthropic 用户消息须全是工具结果块。 / Anthropic user content must consist entirely of tool_result blocks.
        Protocol::Anthropic => trailing_ids(body.get("messages"), |message| {
            if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                return None;
            }
            let parts = message.get("content")?.as_array()?;
            if parts.is_empty() {
                return None;
            }
            let ids: Vec<String> = parts
                .iter()
                .map(|part| {
                    (part.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                        .then(|| part.get("tool_use_id").and_then(|v| v.as_str()))
                        .flatten()
                        .map(String::from)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(ids)
        }),
        Protocol::Responses => trailing_ids(body.get("input"), |item| {
            (item.get("type").and_then(|t| t.as_str()) == Some("function_call_output"))
                .then(|| item.get("call_id").and_then(|v| v.as_str()))
                .flatten()
                .map(|id| vec![id.to_string()])
        }),
    }
}

/// 返回从匹配工具结果开始的客户端原生尾部。 / Return native client items from matched tool results through the request end.
///
/// 仅定位本次恢复新增的历史，不删改任何条目。 / Locate newly added resume history without altering items.
/// 保留工具结果之后的提醒及用户消息。 / Preserve reminders and user messages after tool results.
pub fn continuation_resume_items(
    protocol: Protocol,
    body: &Value,
    pending_ids: &[String],
) -> Vec<Value> {
    let wants = |id: &str| pending_ids.iter().any(|pending| pending == id);
    match protocol {
        Protocol::OpenAI => {
            let Some(items) = body.get("messages").and_then(|value| value.as_array()) else {
                return Vec::new();
            };
            let Some(start) = items.iter().position(|message| {
                message.get("role").and_then(|r| r.as_str()) == Some("tool")
                    && message
                        .get("tool_call_id")
                        .and_then(|value| value.as_str())
                        .is_some_and(&wants)
            }) else {
                return Vec::new();
            };
            items[start..].to_vec()
        }
        Protocol::Anthropic => {
            let Some(items) = body.get("messages").and_then(|value| value.as_array()) else {
                return Vec::new();
            };
            let Some(start) = items.iter().position(|message| {
                if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                    return false;
                }
                message
                    .get("content")
                    .and_then(|value| value.as_array())
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                                && part
                                    .get("tool_use_id")
                                    .and_then(|value| value.as_str())
                                    .is_some_and(&wants)
                        })
                    })
            }) else {
                return Vec::new();
            };
            items[start..].to_vec()
        }
        Protocol::Responses => {
            let Some(items) = body.get("input").and_then(|value| value.as_array()) else {
                return Vec::new();
            };
            let Some(start) = items.iter().position(|item| {
                item.get("type").and_then(|t| t.as_str()) == Some("function_call_output")
                    && item
                        .get("call_id")
                        .and_then(|value| value.as_str())
                        .is_some_and(&wants)
            }) else {
                return Vec::new();
            };
            items[start..].to_vec()
        }
    }
}

/// 提取本次新增的尾部工具结果，保留原生 JSON。 / Extract newly submitted trailing tool results as native JSON.
///
/// 旧接口兼容；正式恢复使用 [`tool_result_items_for_ids`]。 / Legacy API; production resume uses [`tool_result_items_for_ids`] .
pub fn trailing_tool_result_items(protocol: Protocol, body: &Value) -> Vec<Value> {
    match protocol {
        Protocol::OpenAI => trailing_items(body.get("messages"), |message| {
            message.get("role").and_then(|r| r.as_str()) == Some("tool")
        }),
        Protocol::Anthropic => trailing_items(body.get("messages"), |message| {
            if message.get("role").and_then(|r| r.as_str()) != Some("user") {
                return false;
            }
            let Some(parts) = message.get("content").and_then(|c| c.as_array()) else {
                return false;
            };
            !parts.is_empty()
                && parts.iter().all(|part| {
                    part.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                })
        }),
        Protocol::Responses => trailing_items(body.get("input"), |item| {
            item.get("type").and_then(|t| t.as_str()) == Some("function_call_output")
        }),
    }
}

/// 将模型轮次转成下一请求的原生助手记录。 / Convert a model turn into native assistant history for the next request.
///
/// 服务端负责保留相位对话记录。 / The server preserves phase transcript history.
/// 客户端只需回填工具结果。 / The client only supplies tool results.
pub fn model_output_transcript_items(protocol: Protocol, output: &ModelOutput) -> Vec<Value> {
    model_output_transcript_items_with_thinking(protocol, output, false)
}

/// 思考模式的 Chat 工具轮次即使没有新增思考，也保留空字段。 /
/// Keep an empty reasoning_content field on thinking-mode Chat tool turns.
pub fn model_output_transcript_items_with_thinking(
    protocol: Protocol,
    output: &ModelOutput,
    thinking_mode: bool,
) -> Vec<Value> {
    match protocol {
        Protocol::OpenAI => {
            let mut message = json!({"role": "assistant"});
            let text = output
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            message["content"] = if text.is_empty() && !output.tool_calls.is_empty() {
                Value::Null
            } else {
                json!(text)
            };
            let reasoning = output
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Reasoning(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            if !reasoning.is_empty() || (thinking_mode && !output.tool_calls.is_empty()) {
                message["reasoning_content"] = json!(reasoning);
            }
            if !output.tool_calls.is_empty() {
                message["tool_calls"] = Value::Array(
                    output
                        .tool_calls
                        .iter()
                        .map(|call| call.native.clone())
                        .collect(),
                );
            }
            vec![message]
        }
        Protocol::Anthropic => {
            let content: Vec<Value> = output
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Text(text) => Some(json!({"type": "text", "text": text})),
                    Part::Reasoning(text) => {
                        Some(json!({"type": "thinking", "thinking": text}))
                    }
                    Part::ToolCall(call) => Some(call.native.clone()),
                    Part::Native(block) => Some(block.clone()),
                })
                .collect();
            vec![json!({"role": "assistant", "content": content})]
        }
        Protocol::Responses => output
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(json!({
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}]
                })),
                Part::Reasoning(text) => Some(json!({
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": text}]
                })),
                Part::ToolCall(call) => Some(call.native.clone()),
                Part::Native(item) => Some(item.clone()),
            })
            .collect(),
    }
}

fn trailing_items<F>(container: Option<&Value>, is_item: F) -> Vec<Value>
where
    F: Fn(&Value) -> bool,
{
    let Some(items) = container.and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for item in items.iter().rev() {
        if !is_item(item) {
            break;
        }
        found.push(item.clone());
    }
    found.reverse();
    found
}

fn all_ids<F>(container: Option<&Value>, ids_of: F) -> Vec<String>
where
    F: Fn(&Value) -> Option<Vec<String>>,
{
    let Some(items) = container.and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    items.iter().filter_map(ids_of).flatten().collect()
}

/// 收集尾部连续工具结果的 ID。 / Collect IDs from trailing consecutive tool results.
fn trailing_ids<F>(container: Option<&Value>, ids_of: F) -> Vec<String>
where
    F: Fn(&Value) -> Option<Vec<String>>,
{
    let Some(items) = container.and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = Vec::new();
    for item in items.iter().rev() {
        match ids_of(item) {
            Some(found) => ids.extend(found),
            None => break,
        }
    }
    ids.reverse();
    ids
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn detects_token_limit_stop_reasons() {
        for reason in ["length", "max_tokens", "max_output_tokens"] {
            let out = ModelOutput {
                stop_reason: Some(reason.into()),
                ..ModelOutput::default()
            };
            assert!(out.is_truncated(), "reason={reason}");
        }
        let normal = ModelOutput {
            stop_reason: Some("stop".into()),
            ..ModelOutput::default()
        };
        assert!(!normal.is_truncated());
    }

    #[test]
    fn chat_json_with_tool_calls_keeps_native_shape() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1", "type": "function",
                        "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
        });
        let out = parse_json_output(Protocol::OpenAI, &body);
        assert_eq!(out.text, "");
        assert_eq!(out.stop_reason.as_deref(), Some("tool_calls"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert_eq!(out.tool_calls[0].name, "Bash");
        assert_eq!(out.tool_calls[0].arguments, Value::String("{\"command\":\"ls\"}".into()));
        // 保留原生对象。 / Preserve native objects.
        assert_eq!(out.tool_calls[0].native["function"]["name"], "Bash");
        assert_eq!(out.usage.unwrap()["total_tokens"], 12);
    }

    #[test]
    fn anthropic_json_tool_use_and_text() {
        let body = json!({
            "content": [
                {"type": "text", "text": "我先看一下"},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 7}
        });
        let out = parse_json_output(Protocol::Anthropic, &body);
        assert_eq!(out.text, "我先看一下");
        assert_eq!(out.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(out.tool_calls[0].arguments, json!({"command": "ls"}));
        assert_eq!(out.tool_calls[0].native["id"], "toolu_1");
    }

    #[test]
    fn responses_json_function_call() {
        let body = json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "看一下"}]},
                {"type": "function_call", "call_id": "fc_1", "name": "Bash", "arguments": "{\"a\":1}"}
            ],
            "status": "completed",
            "usage": {"input_tokens": 3, "output_tokens": 4}
        });
        let out = parse_json_output(Protocol::Responses, &body);
        assert_eq!(out.text, "看一下");
        assert_eq!(out.tool_calls[0].id, "fc_1");
        assert_eq!(out.tool_calls[0].arguments, Value::String("{\"a\":1}".into()));
    }

    /// 流式文本与工具参数需完整重组。 / Reassemble streamed text and tool arguments, including split fragments.
    #[test]
    fn chat_sse_accumulates_text_and_tool_arguments() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"我来\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"执行\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"Bash\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"comm\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"and\\\":\\\"ls\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            "data: [DONE]\n\n"
        );
        let out = parse_sse_output(Protocol::OpenAI, sse);
        assert_eq!(out.text, "我来执行");
        assert_eq!(out.stop_reason.as_deref(), Some("tool_calls"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert_eq!(out.tool_calls[0].name, "Bash");
        assert_eq!(out.tool_calls[0].arguments_json(), "{\"command\":\"ls\"}");
        assert_eq!(out.usage.unwrap()["total_tokens"], 3);
    }

    #[test]
    fn anthropic_sse_accumulates_tool_use_input_json() {
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"查一下\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"Bash\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"ls -la\\\"}\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":9}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        let out = parse_sse_output(Protocol::Anthropic, sse);
        assert_eq!(out.text, "查一下");
        assert_eq!(out.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name, "Bash");
        assert_eq!(out.tool_calls[0].arguments, json!({"command": "ls -la"}));
        assert_eq!(out.tool_calls[0].native["input"], json!({"command": "ls -la"}));
        // usage 合并起始帧与增量帧。 / Merge usage from message_start and message_delta.
        let usage = out.usage.unwrap();
        assert_eq!(usage["input_tokens"], 5);
        assert_eq!(usage["output_tokens"], 9);
    }

    #[test]
    fn responses_sse_accumulates_function_call_arguments() {
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"看\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"id\":\"fc_item\",\"type\":\"function_call\",\"call_id\":\"fc_1\",\"name\":\"Bash\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_item\",\"delta\":\"{\\\"a\\\"\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_item\",\"delta\":\":1}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n"
        );
        let out = parse_sse_output(Protocol::Responses, sse);
        assert_eq!(out.text, "看");
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "fc_1");
        assert_eq!(out.tool_calls[0].arguments_json(), "{\"a\":1}");
        assert_eq!(out.tool_calls[0].native["arguments"], "{\"a\":1}");
        assert_eq!(out.usage.unwrap()["output_tokens"], 2);
    }

    #[test]
    fn plain_text_responses_still_parse() {
        let body = json!({"choices": [{"message": {"content": "答案"}, "finish_reason": "stop"}]});
        let out = parse_json_output(Protocol::OpenAI, &body);
        assert_eq!(out.text, "答案");
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.stop_reason.as_deref(), Some("stop"));
    }

    /// Anthropic 非流式思考块必须保留。 / Preserve non-streaming Anthropic thinking blocks.
    #[test]
    fn anthropic_thinking_blocks_survive_json() {
        let body = json!({
            "content": [
                {"type": "thinking", "thinking": "先想想", "signature": "sig-1"},
                {"type": "text", "text": "答案"},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}
            ],
            "stop_reason": "tool_use"
        });
        let out = parse_json_output(Protocol::Anthropic, &body);
        assert_eq!(out.text, "答案", "思考不进可见文本");
        assert_eq!(out.parts.len(), 3);
        match &out.parts[0] {
            Part::Native(block) => {
                assert_eq!(block["type"], "thinking");
                assert_eq!(block["thinking"], "先想想");
                assert_eq!(block["signature"], "sig-1", "签名必须原样保留");
            }
            other => panic!("思考块被丢了: {other:?}"),
        }
        assert_eq!(out.parts[1], Part::Text("答案".into()));
        assert!(matches!(&out.parts[2], Part::ToolCall(call) if call.id == "t1"));
    }

    /// 合并流式思考与签名增量。 / Merge streaming reasoning/signature deltas into one native block.
    #[test]
    fn anthropic_thinking_sse_is_reassembled() {
        let sse = concat!(
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"想一想\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"结论\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n"
        );
        let out = parse_sse_output(Protocol::Anthropic, sse);
        assert_eq!(out.text, "结论");
        assert_eq!(out.parts.len(), 2);
        match &out.parts[0] {
            Part::Native(block) => {
                assert_eq!(block["type"], "thinking");
                assert_eq!(block["thinking"], "想一想");
                assert_eq!(block["signature"], "sig");
            }
            other => panic!("流式思考块丢了: {other:?}"),
        }
        assert_eq!(out.parts[1], Part::Text("结论".into()));
    }

    /// 保留 Chat 思考字段。 / Preserve Chat reasoning fields.
    #[test]
    fn chat_reasoning_is_kept() {
        let body = json!({"choices": [{
            "message": {"content": "答案", "reasoning_content": "想过了"},
            "finish_reason": "stop"
        }]});
        let out = parse_json_output(Protocol::OpenAI, &body);
        assert_eq!(out.text, "答案");
        assert_eq!(out.parts[0], Part::Reasoning("想过了".into()));
        assert_eq!(out.parts[1], Part::Text("答案".into()));

        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"先想\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"答案\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
        );
        let out = parse_sse_output(Protocol::OpenAI, sse);
        assert_eq!(out.text, "答案");
        assert_eq!(out.parts[0], Part::Reasoning("先想".into()));
        assert_eq!(out.parts[1], Part::Text("答案".into()));
    }

    /// Responses 推理条目（含密文）原样保留。 / Preserve Responses reasoning items, including encrypted content.
    #[test]
    fn responses_reasoning_item_passes_through() {
        let reasoning = json!({
            "type": "reasoning", "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "想了"}],
            "encrypted_content": "opaque-blob"
        });
        let body = json!({"output": [
            reasoning.clone(),
            {"type": "message", "content": [{"type": "output_text", "text": "答案"}]}
        ]});
        let out = parse_json_output(Protocol::Responses, &body);
        assert_eq!(out.text, "答案");
        assert_eq!(out.parts[0], Part::Native(reasoning));

        // 流式以最终完成帧的 output[] 为准。 / For streams, the completed frame's output[] is authoritative.
        let sse = format!(
            "data: {}\n\n",
            json!({"type": "response.completed", "response": {
                "status": "completed",
                "usage": {"output_tokens": 3},
                "output": [
                    {"type": "reasoning", "id": "rs_1", "encrypted_content": "blob"},
                    {"type": "message", "content": [{"type": "output_text", "text": "答案"}]}
                ]
            }})
        );
        let out = parse_sse_output(Protocol::Responses, &sse);
        assert_eq!(out.text, "答案");
        assert_eq!(out.usage.unwrap()["output_tokens"], 3);
        assert!(matches!(&out.parts[0], Part::Native(v) if v["encrypted_content"] == "blob"));
    }

    #[test]
    fn trailing_tool_results_detection() {
        // Chat 尾部是工具消息。 / Chat ends with a tool message.
        let body = json!({"messages": [
            {"role": "user", "content": "跑一下"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "call_1"}]},
            {"role": "tool", "tool_call_id": "call_1", "content": "输出"}
        ]});
        assert_eq!(trailing_tool_result_ids(Protocol::OpenAI, &body), vec!["call_1"]);
        assert_eq!(
            trailing_tool_result_items(Protocol::OpenAI, &body),
            vec![json!({"role": "tool", "tool_call_id": "call_1", "content": "输出"})]
        );
        // 普通用户消息结尾不构成工具恢复。 / A trailing ordinary user message is not a tool resume.
        let body = json!({"messages": [
            {"role": "tool", "tool_call_id": "call_1", "content": "输出"},
            {"role": "user", "content": "继续"}
        ]});
        assert!(trailing_tool_result_ids(Protocol::OpenAI, &body).is_empty());
        assert_eq!(all_tool_result_ids(Protocol::OpenAI, &body), vec!["call_1"]);
        assert_eq!(
            continuation_resume_items(Protocol::OpenAI, &body, &["call_1".into()]),
            vec![
                json!({"role": "tool", "tool_call_id": "call_1", "content": "输出"}),
                json!({"role": "user", "content": "继续"})
            ]
        );

        // Anthropic 用户内容必须全部为工具结果。 / Anthropic user content must contain only tool_result blocks.
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "x"}]}
        ]});
        assert_eq!(trailing_tool_result_ids(Protocol::Anthropic, &body), vec!["toolu_1"]);
        assert_eq!(
            trailing_tool_result_items(Protocol::Anthropic, &body),
            vec![json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "x"}]})]
        );
        // 混合文本内容不算工具恢复。 / Mixed text content is not a tool resume.
        let body = json!({"messages": [
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "x"},
                {"type": "text", "text": "顺带说一句"}
            ]}
        ]});
        assert!(trailing_tool_result_ids(Protocol::Anthropic, &body).is_empty());
        assert_eq!(all_tool_result_ids(Protocol::Anthropic, &body), vec!["toolu_1"]);
        assert_eq!(
            continuation_resume_items(Protocol::Anthropic, &body, &["toolu_1".into()]),
            vec![json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "x"},
                {"type": "text", "text": "顺带说一句"}
            ]})]
        );

        // Responses 尾部函数调用结果。 / Responses trailing function-call output.
        let body = json!({"input": [
            {"type": "function_call", "call_id": "fc_1", "name": "Bash", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "fc_1", "output": "ok"}
        ]});
        assert_eq!(trailing_tool_result_ids(Protocol::Responses, &body), vec!["fc_1"]);
        assert_eq!(
            trailing_tool_result_items(Protocol::Responses, &body),
            vec![json!({"type": "function_call_output", "call_id": "fc_1", "output": "ok"})]
        );
    }

    #[test]
    fn model_output_becomes_native_phase_transcript() {
        let chat = parse_json_output(
            Protocol::OpenAI,
            &json!({"choices": [{"message": {
                "role": "assistant",
                "content": "查一下",
                "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "Bash", "arguments": "{}"}}]
            }, "finish_reason": "tool_calls"}]}),
        );
        let chat_items = model_output_transcript_items(Protocol::OpenAI, &chat);
        assert_eq!(chat_items.len(), 1);
        assert_eq!(chat_items[0]["role"], "assistant");
        assert_eq!(chat_items[0]["tool_calls"][0]["id"], "call_1");

        let anthropic = parse_json_output(
            Protocol::Anthropic,
            &json!({"content": [
                {"type": "text", "text": "查一下"},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}}
            ], "stop_reason": "tool_use"}),
        );
        let anthropic_items = model_output_transcript_items(Protocol::Anthropic, &anthropic);
        assert_eq!(anthropic_items.len(), 1);
        assert_eq!(anthropic_items[0]["role"], "assistant");
        assert_eq!(anthropic_items[0]["content"][1]["id"], "toolu_1");

        let responses = parse_json_output(
            Protocol::Responses,
            &json!({"output": [
                {"type": "function_call", "call_id": "fc_1", "name": "Bash", "arguments": "{}"}
            ]}),
        );
        let response_items = model_output_transcript_items(Protocol::Responses, &responses);
        assert_eq!(response_items.len(), 1);
        assert_eq!(response_items[0]["type"], "function_call");
        assert_eq!(response_items[0]["call_id"], "fc_1");
    }

    /// 思考模型的空思考工具调用必须保留字段，非思考模型不受影响。 /
    /// Keep empty reasoning on thinking tool calls without affecting non-thinking calls.
    #[test]
    fn thinking_tool_call_keeps_empty_reasoning_content() {
        let call = json!({
            "id": "call_1", "type": "function",
            "function": {"name": "Bash", "arguments": "{}"}
        });
        let no_reasoning = parse_json_output(
            Protocol::OpenAI,
            &json!({"choices": [{"message": {
                "role": "assistant", "content": null, "tool_calls": [call.clone()]
            }, "finish_reason": "tool_calls"}]}),
        );
        let thinking =
            model_output_transcript_items_with_thinking(Protocol::OpenAI, &no_reasoning, true);
        assert_eq!(thinking[0]["reasoning_content"], "");
        assert_eq!(thinking[0]["content"], Value::Null);
        assert_eq!(thinking[0]["tool_calls"][0], call);

        let non_thinking =
            model_output_transcript_items_with_thinking(Protocol::OpenAI, &no_reasoning, false);
        assert!(non_thinking[0].get("reasoning_content").is_none());

        let with_reasoning = parse_json_output(
            Protocol::OpenAI,
            &json!({"choices": [{"message": {
                "role": "assistant", "reasoning_content": "original",
                "content": null, "tool_calls": [call]
            }, "finish_reason": "tool_calls"}]}),
        );
        let preserved = model_output_transcript_items_with_thinking(
            Protocol::OpenAI, &with_reasoning, true
        );
        assert_eq!(preserved[0]["reasoning_content"], "original");

        let no_tools = parse_json_output(
            Protocol::OpenAI,
            &json!({"choices": [{"message": {"role": "assistant", "content": "done"}}]}),
        );
        let plain =
            model_output_transcript_items_with_thinking(Protocol::OpenAI, &no_tools, true);
        assert!(plain[0].get("reasoning_content").is_none());
    }
}
