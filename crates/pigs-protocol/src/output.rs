//! 把上游响应（JSON / SSE 全文）归一化成"这一轮模型输出"。
//!
//! 归一化只服务于编排判定，**不转写任何内容**：文本原样拼、工具调用保留原生 JSON
//! （回给客户端时逐字节还原）、stop_reason 与 usage 原样带出。

use crate::route::Protocol;
use serde_json::{json, Value};

/// 一次工具调用（保留原生表示）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// 关联后续工具结果的标识符。
    pub id: String,
    /// 工具名。
    pub name: String,
    /// 原生参数值（字符串参数保持字符串）。
    pub arguments: Value,
    /// 完整的原生 tool-call 对象 / 内容块（回给客户端时原样使用）。
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

/// 上游返回的一段内容（**按顺序**留给客户端，互不覆盖）。
///
/// 编排只关心 `Text` 与 `ToolCall`；其余一律原样透传，不做解释、不做删改
/// （思考块、推理条目、搜索调用……都属于这一类）。
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// 用户可见文本。
    Text(String),
    /// 思考文本（协议原生思考字段，如 `reasoning_content`）。
    Reasoning(String),
    /// 工具调用（原生形状）。
    ToolCall(ToolCall),
    /// 其它原生内容块 / output item：原样透传。
    Native(Value),
}

/// 一轮模型输出（编排所需的最小归一化）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelOutput {
    /// 用户可见文本（已按协议拼好，含有可能出现的控制标记）。
    pub text: String,
    /// 本轮要求调用的工具（空 = 这一轮没有工具调用）。
    pub tool_calls: Vec<ToolCall>,
    /// 给客户端的内容序列（顺序权威；`Text`/`ToolCall` 之外一律原样）。
    pub parts: Vec<Part>,
    /// 协议原生的停止原因（Chat 的 `finish_reason` / Anthropic 的 `stop_reason`）。
    pub stop_reason: Option<String>,
    /// 协议原生的 usage 对象（原样，不做任何改写）。
    pub usage: Option<Value>,
}

impl ModelOutput {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.tool_calls.is_empty()
    }

    /// 上游明确表示本轮因为生成 token 上限而被截断。
    pub fn is_truncated(&self) -> bool {
        matches!(
            self.stop_reason.as_deref(),
            Some("length" | "max_tokens" | "max_output_tokens")
        )
    }
}

/// 解析非流式 JSON 响应。
pub fn parse_json_output(protocol: Protocol, body: &Value) -> ModelOutput {
    match protocol {
        Protocol::OpenAI => parse_chat_json(body),
        Protocol::Anthropic => parse_anthropic_json(body),
        Protocol::Responses => parse_responses_json(body),
    }
}

/// 解析 SSE 全文（多个 `data:` 行）——增量已经流过客户端，这里只做"这一轮到底出了什么"的汇总。
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

// ---------------------------------------------------------------- 非流式

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
                // thinking / redacted_thinking / server_tool_use……：原样透传，不解释
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
                // reasoning / web_search_call / code_interpreter_call……：原样透传
                _ => parts.push(Part::Native(item.clone())),
            }
        }
    }
    ModelOutput {
        text,
        tool_calls,
        parts,
        // Responses 没有 finish_reason；截断原因在 incomplete_details
        stop_reason: body
            .pointer("/incomplete_details/reason")
            .and_then(|r| r.as_str())
            .map(String::from),
        usage: body.get("usage").cloned(),
    }
}

/// 文本 content：字符串直接用；内容块数组拼 `text`/`output_text` 字段。
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

/// Chat 的思考文本字段（各家的叫法：`reasoning_content` / `reasoning`）。
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

/// Responses 的 message item 文本：拼所有 `output_text` 片段的 `text`。
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

// ---------------------------------------------------------------- SSE 汇总

/// SSE 增量汇总器：把一串事件合成一轮输出（文本、工具调用、停止原因、usage）。
struct SseOutputAccumulator {
    protocol: Protocol,
    output: ModelOutput,
    /// Chat：按 `index` 拼工具调用参数。
    chat_calls: Vec<ToolCall>,
    /// Chat：累积的思考文本。
    chat_reasoning: String,
    /// Anthropic：按 content_block 的 index 记账（文本或工具调用）。
    anthropic_blocks: Vec<AnthropicBlock>,
    /// Responses：按 item_id 拼 function_call 参数。
    responses_calls: Vec<ToolCall>,
}

enum AnthropicBlock {
    /// 文本块（累积 text_delta）。
    Text { buf: String },
    /// 思考块（累积 thinking_delta 与 signature_delta）。
    Thinking { buf: String, signature: String },
    /// 工具调用块。
    ToolUse,
    /// 其它块（redacted_thinking / server_tool_use……）：start 帧原样留着。
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
                    // start 帧通常给 `input: {}`（参数随后由 input_json_delta 拼）；
                    // 若上游直接给全了参数，就原样留着，别清空
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
                    // 未知块（redacted_thinking / server_tool_use……）原样留着
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
                    // thinking_delta / signature_delta：不进可见文本
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

    /// 把文本追加到 index 的文本块。
    fn anthropic_block_mut_text(&mut self, index: usize, text: &str) {
        if let Some(AnthropicBlock::Text { buf }) = self.anthropic_blocks.get_mut(index) {
            buf.push_str(text);
        }
    }

    /// 对 index 的思考块做一次修改。
    fn anthropic_block_mut_thinking<F: FnOnce(&mut AnthropicBlock)>(
        &mut self,
        index: usize,
        edit: F,
    ) {
        if let Some(block @ AnthropicBlock::Thinking { .. }) = self.anthropic_blocks.get_mut(index) {
            edit(block);
        }
    }

    /// 找 index 对应的工具调用（Anthropic 的块序与 tool_calls 序一致）。
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
                    // 参数可能是本帧就给全，也可能是后续 .delta 逐片给；原样起步，逐片追加
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
                    // 最终 output[] 是权威内容序列（含 reasoning 等原生条目）
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
                        // 参数拼好后写回原生对象，回给客户端时原样可用
                        let arguments = call.arguments_json();
                        if let Some(obj) = call.native.as_object_mut() {
                            obj.insert("arguments".into(), Value::String(arguments));
                        }
                        call
                    })
                    .collect();
            }
            Protocol::Anthropic => {
                // 工具调用的增量参数收尾（写回原生块）
                for call in &mut self.output.tool_calls {
                    let raw = call.arguments_json();
                    let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = call.native.as_object_mut() {
                        obj.insert("input".into(), parsed.clone());
                    }
                    call.arguments = parsed;
                }
                // 按 content_block 的顺序生成给客户端的内容序列
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

/// Chat 的 SSE 工具调用拼好后写回原生对象（`arguments` 是字符串）。
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

/// 浅合并两个 usage 对象（后者覆盖同名字段；数字相加由调用方语义决定，这里保原样优先）。
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

// ---------------------------------------------------------------- 恢复判定

/// 客户端这一发请求是不是"工具结果回填"：返回**尾部**工具结果的 id 列表。
///
/// 规则（与 legacy 一致）：从消息末尾往前扫，遇到第一个不是工具结果的条目就停。
/// 空 = 不是恢复请求。
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
        // Anthropic：role=user 且 content 全是 tool_result 块
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

/// 找到当前 continuation 的工具结果后，返回从该结果开始到客户端请求末尾的原生条目。
///
/// 这个 helper 只负责定位“本次恢复新增了哪一段客户端历史”，不会删除、过滤或改写
/// 工具结果之后的 reminder / 普通 user 消息。
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

/// 返回客户端这次真正新回填的尾部工具结果条目（保留协议原生 JSON）。
///
/// 兼容旧调用；新的 continuation 恢复主链路使用 [`tool_result_items_for_ids`]。
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

/// 把一轮模型输出转换成下一轮请求要接回去的协议原生 assistant 记录。
///
/// 这是服务端 phase transcript 的一部分：模型说过什么、调用过什么工具，由 pigs
/// 自己保存；客户端只负责把工具结果送回来。
pub fn model_output_transcript_items(protocol: Protocol, output: &ModelOutput) -> Vec<Value> {
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
            if !reasoning.is_empty() {
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

/// 从尾部连续的工具结果条目收集 id。
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
        // 原生对象原样保留
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

    /// 流式：文本与工具调用参数都是增量到达，必须拼得完整（含参数被切成多片）。
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
        // usage 合并了 message_start 与 message_delta
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

    /// 思考块必须原样留给客户端（Anthropic 非流式）。
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

    /// 流式思考：thinking_delta 与 signature_delta 要拼回一个原生思考块。
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

    /// Chat 的思考字段（reasoning_content / reasoning）不能丢。
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

    /// Responses 的 reasoning 条目（含加密内容）原样透传。
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

        // 流式：以 response.completed 的 output[] 为准
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
        // Chat：末条 role=tool
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
        // 末条是普通 user → 不是恢复请求
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

        // Anthropic：role=user 且 content 全 tool_result
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "x"}]}
        ]});
        assert_eq!(trailing_tool_result_ids(Protocol::Anthropic, &body), vec!["toolu_1"]);
        assert_eq!(
            trailing_tool_result_items(Protocol::Anthropic, &body),
            vec![json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "x"}]})]
        );
        // 混杂文本块 → 不算
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

        // Responses：尾部 function_call_output
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
}
