//! 协议原生请求保留、相位变更与响应抽取。
//! Protocol-native request preservation, phase mutation, and response extraction.
//!
//! 本模块是 HTTP 相位运行时的"数据面"核心。它把一次完整的客户端请求
//! （方法、path、headers、原生 JSON body）封装为 `HttpRequestEnvelope`，
//! 并提供三类操作：
//! 1. **解析**（`ProtocolCodec::parse_request`）：校验 body 并定位当前 user 输入；
//! 2. **相位变更**（`for_pre` / `for_executor` / `for_post` / `with_appended_transcript`）：
//!    clone 原请求，只修改 model、当前 user 文本和相位对话记录；
//! 3. **响应抽取**（`extract_response`）：把上游非流式响应解析为
//!    `NormalizedModelOutput`（可见文本 + 原生条目 + 工具调用 + usage）。
//!
//! This module is the "data plane" core of the HTTP phase runtime. It wraps a
//! complete client request (method, path, headers, native JSON body) into an
//! `HttpRequestEnvelope`, and provides three operations:
//! 1. **Parse** (`ProtocolCodec::parse_request`): validate the body and locate
//!    the current user input;
//! 2. **Phase mutation** (`for_pre`/`for_executor`/`for_post`/`with_appended_transcript`):
//!    clone the original request, mutating only the model, current user text,
//!    and phase transcript;
//! 3. **Response extraction** (`extract_response`): parse an upstream
//!    non-streaming response into `NormalizedModelOutput` (visible text +
//!    native items + tool calls + usage).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// 客户端与上游模型供应商使用的 API 协议。
/// API protocol used by the client and upstream model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// OpenAI Chat Completions（`/chat/completions`）。
    /// OpenAI Chat Completions.
    OpenAiChat,
    /// Anthropic Messages（`/v1/messages`）。
    /// Anthropic Messages.
    AnthropicMessages,
    /// OpenAI Responses（`/responses`）。
    /// OpenAI Responses.
    OpenAiResponses,
}

impl fmt::Display for Protocol {
    /// 协议的稳定字符串标识（用于日志与错误信息）。
    /// Stable string identifier for logs and errors.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OpenAiChat => "openai_chat",
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenAiResponses => "openai_responses",
        })
    }
}

/// 传输中立的 HTTP 头部名值对。
/// A transport-neutral HTTP header name and value pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPair {
    /// 头部名称（按传输层原样保留）/ header name as received from the transport.
    pub name: String,
    /// 头部原始值字节（按传输层原样保留）/ raw header value bytes as received from the transport.
    pub value: Vec<u8>,
}

impl HeaderPair {
    /// 不依赖任何 HTTP crate 构造一个头部对。
    /// Creates a header pair without depending on an HTTP crate.
    pub fn new(name: impl Into<String>, value: impl AsRef<[u8]>) -> Self {
        Self {
            name: name.into(),
            value: value.as_ref().to_vec(),
        }
    }
}

/// 当前 user 输入在请求体中的位置（内部类型）。
/// Where the current user input lives in the request body (internal type).
#[derive(Debug, Clone, PartialEq, Eq)]
enum CurrentUserLocation {
    /// Responses 协议的顶层 `input` 是字符串（而非数组）。
    /// Responses protocol's top-level `input` is a string (not an array).
    ResponsesString,
    /// 某个集合（`messages` 或 `input`）中的第 `index` 个条目。
    /// An item at `index` inside a collection (`messages` or `input`).
    CollectionItem {
        /// 集合字段名（"messages" 或 "input"）/ collection field name.
        collection: &'static str,
        /// 当前 user 条目的下标 / index of the current user item.
        index: usize,
    },
}

/// 已校验的客户端请求，含完整的协议原生 JSON body。
/// A validated client request with its complete protocol-native JSON body.
///
/// 这是相位运行时数据面的"信封"。它保留请求的全部原生结构（未知字段、
/// 原生块都原样保留），只额外记录解析后得到的语义信息：协议、模型名、
/// 当前 user 位置、工具结果 ID 等。
///
/// This is the phase runtime data-plane "envelope". It preserves the
/// request's full native structure (unknown fields and native blocks remain
/// intact) and only adds parsed semantic metadata: protocol, model name,
/// current-user location, tool-result IDs, etc.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequestEnvelope {
    /// HTTP 方法（不依赖 `http::Method`）/ HTTP method (no `http::Method` dependency).
    pub method: String,
    /// 请求路径（含可选 query）/ request path including optional query string.
    pub path_and_query: String,
    /// 按传输层顺序排列的头部对 / header pairs in transport-provided order.
    pub headers: Vec<HeaderPair>,
    /// 完整请求体。未知字段与原生块原样保留 / complete request body; unknown fields and native blocks remain intact.
    pub body: Value,
    /// 请求使用的协议 / protocol used by the request.
    pub protocol: Protocol,
    /// 客户端提供的模型标识符（可能带 `-pig` 后缀）/ model identifier supplied by the client.
    pub client_model: String,
    /// 客户端模型去掉恰好一个尾部 `-pig` 后的结果。
    /// Client model with exactly one trailing `-pig` removed.
    pub real_model: String,
    /// 请求体是否要求流式响应 / whether the request body asks for a streaming response.
    pub stream: bool,
    /// 当前 user 输入在 body 中的位置（内部）/ location of the current user input (internal).
    current_user: CurrentUserLocation,
    /// 请求中找到的工具结果 ID 列表 / tool result identifiers found in the request.
    tool_result_ids: Vec<String>,
    /// 尾部的原生工具结果组（用于 continuation 匹配）/ trailing native tool-result groups (for continuation matching).
    tool_result_groups: Vec<NativeToolResultGroup>,
}

impl HttpRequestEnvelope {
    /// 返回当前 user 输入的文本部分（拼接所有 text part）。
    /// Returns the concatenated textual portion of the current user input.
    pub fn current_user_text(&self) -> Result<String, CodecError> {
        match self.current_content()? {
            // 字符串内容直接返回 / String content returned directly.
            Value::String(text) => Ok(text.clone()),
            // 数组内容拼接所有 text part / Array content: concatenate all text parts.
            Value::Array(parts) => Ok(text_from_parts(parts, self.protocol)),
            // 其它类型 → 错误 / Other types → error.
            _ => Err(CodecError::InvalidField {
                field: "current user content".into(),
                expected: "a string or content-part array",
            }),
        }
    }

    /// 返回请求中找到的工具结果 ID。
    /// Returns tool result identifiers found in the incoming native request.
    pub fn tool_result_ids(&self) -> &[String] {
        &self.tool_result_ids
    }

    /// 判断本请求是否在恢复一个暂停的原生工具调用。
    /// Whether this request resumes a paused native tool call.
    pub fn is_continuation(&self) -> bool {
        !self.tool_result_groups.is_empty()
    }

    /// 返回用于 continuation 匹配的完整尾部工具结果组。
    /// Returns complete trailing native tool-result groups for continuation matching.
    pub fn tool_result_groups(&self) -> &[NativeToolResultGroup] {
        &self.tool_result_groups
    }

    /// 为 Pre 相位 clone 本请求，并在当前 user 文本后追加 `suffix`。
    /// Clones this request for Pre and appends `suffix` to the current user text.
    pub fn for_pre(
        &self,
        suffix: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        self.with_user_suffix(suffix, overrides)
    }

    /// 为 Executor 相位 clone 本请求，并在当前 user 文本后追加 `suffix`。
    /// Clones this request for Executor and appends `suffix` to the current user text.
    pub fn for_executor(
        &self,
        suffix: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        self.with_user_suffix(suffix, overrides)
    }

    /// 为 Post 相位构造请求：用原生对话记录替换当前 user，再追加审阅提示词。
    ///
    /// Post 相位是特殊的：它不只是在原 user 后追加后缀，而是把当前 user
    /// 条目移除，替换为完整的相位对话记录（assistant/tool_use/tool_result）
    /// 加上一个新的 user 审阅消息。这让 Post 能看到本相位之前的全部上下文。
    ///
    /// Builds a Post request from original history, native transcript items, and a new prompt.
    pub fn for_post(
        &self,
        transcript: &[NativeTranscriptItem],
        review_prompt: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        // 校验所有 transcript 条目协议一致且为对象 / Validate all transcript items share the protocol and are objects.
        for item in transcript {
            if item.protocol != self.protocol {
                return Err(CodecError::TranscriptProtocolMismatch {
                    expected: self.protocol,
                    actual: item.protocol,
                });
            }
            if !item.value.is_object() {
                return Err(CodecError::InvalidField {
                    field: "native transcript item".into(),
                    expected: "an object",
                });
            }
        }

        let mut phase = self.clone();
        // 应用 model/stream 覆盖 / Apply model/stream overrides.
        phase.apply_overrides(overrides)?;
        // 收集 transcript 的原生 JSON 值 / Collect the transcript's native JSON values.
        let transcript_values = transcript.iter().map(|item| item.value.clone());

        // 根据当前 user 位置决定如何替换 / Decide how to replace based on current-user location.
        phase.current_user = match self.current_user {
            CurrentUserLocation::ResponsesString => {
                // Responses 字符串 input：把字符串转成 message 条目，再追加 transcript + review_message
                // Responses string input: convert the string to a message item, then append transcript + review_message.
                let original = object(&self.body)?
                    .get("input")
                    .cloned()
                    .ok_or_else(|| CodecError::MissingField("input".into()))?;
                let mut input: Vec<Value> = Vec::new();
                input.push(review_message(
                    self.protocol,
                    original.as_str().unwrap_or(""),
                ));
                input.extend(transcript_values);
                input.push(review_message(self.protocol, review_prompt));
                let review_index = input.len() - 1;
                object_mut(&mut phase.body)?.insert("input".into(), Value::Array(input));
                CurrentUserLocation::CollectionItem {
                    collection: "input",
                    index: review_index,
                }
            }
            CurrentUserLocation::CollectionItem { collection, index } => {
                // 数组集合：保留原 user 条目，追加 transcript + review_message
                // Array collection: keep the old user item, append transcript + review_message.
                let items = collection_mut(&mut phase.body, collection)?;
                if index >= items.len() {
                    return Err(CodecError::InvalidField {
                        field: collection.into(),
                        expected: "the validated current user item",
                    });
                }
                items.extend(transcript_values);
                items.push(review_message(self.protocol, review_prompt));
                CurrentUserLocation::CollectionItem {
                    collection,
                    index: items.len() - 1,
                }
            }
        };
        // 重新解析工具结果 ID 与组（body 已变更）/ Re-parse tool-result IDs and groups (body changed).
        phase.tool_result_ids = collect_tool_result_ids(self.protocol, &phase.body);
        phase.tool_result_groups = trailing_tool_result_groups(self.protocol, &phase.body);
        Ok(phase)
    }

    /// 在本相位的 user 输入之后追加原生 assistant/tool 对话记录。
    ///
    /// 与 `for_post` 不同，此方法不移除当前 user，而是在其后追加条目。
    /// 用于在同一相位内把上一轮 LLM 的 assistant 输出和工具结果接回去。
    ///
    /// Appends native assistant/tool transcript items after this phase's user input.
    pub fn with_appended_transcript(
        &self,
        transcript: &[NativeTranscriptItem],
    ) -> Result<Self, CodecError> {
        // 校验协议一致 / Validate protocol consistency.
        for item in transcript {
            if item.protocol != self.protocol {
                return Err(CodecError::TranscriptProtocolMismatch {
                    expected: self.protocol,
                    actual: item.protocol,
                });
            }
        }
        // 空 transcript → 直接 clone / Empty transcript → just clone.
        if transcript.is_empty() {
            return Ok(self.clone());
        }

        let mut request = self.clone();
        let values = transcript.iter().map(|item| item.value.clone());
        match request.current_user {
            CurrentUserLocation::ResponsesString => {
                // Responses 字符串 input：把字符串转成 message 条目，再追加 transcript
                // Responses string input: convert the string to a message item, then append transcript.
                let original = object(&request.body)?
                    .get("input")
                    .cloned()
                    .ok_or_else(|| CodecError::MissingField("input".into()))?;
                let mut input = vec![review_message(
                    self.protocol,
                    original.as_str().unwrap_or(""),
                )];
                input.extend(values);
                object_mut(&mut request.body)?.insert("input".into(), Value::Array(input));
                // 位置变为 input 数组的第 0 项 / Location becomes input[0].
                request.current_user = CurrentUserLocation::CollectionItem {
                    collection: "input",
                    index: 0,
                };
            }
            CurrentUserLocation::CollectionItem { collection, .. } => {
                // 数组集合：直接追加 / Array collection: just append.
                collection_mut(&mut request.body, collection)?.extend(values);
            }
        }
        // 重新解析工具结果 / Re-parse tool results.
        request.tool_result_ids = collect_tool_result_ids(self.protocol, &request.body);
        request.tool_result_groups = trailing_tool_result_groups(self.protocol, &request.body);
        Ok(request)
    }

    /// 从非流式模型响应中抽取归一化输出，并关联请求中的工具结果 ID。
    /// Extracts a non-streaming model response and associates incoming tool results.
    pub fn extract_response(&self, response: &Value) -> Result<NormalizedModelOutput, CodecError> {
        ProtocolCodec::new(self.protocol).extract_response_for_request(self, response)
    }

    /// 内部：返回当前 user 输入的 content 值引用。
    /// Internal: return a reference to the current user's content value.
    fn current_content(&self) -> Result<&Value, CodecError> {
        match self.current_user {
            // Responses 字符串：input 字段本身就是内容 / Responses string: the input field is the content.
            CurrentUserLocation::ResponsesString => object(&self.body)?
                .get("input")
                .ok_or_else(|| CodecError::MissingField("input".into())),
            // 数组集合：定位到 messages[index].content / Array collection: locate messages[index].content.
            CurrentUserLocation::CollectionItem {
                collection: collection_name,
                index,
            } => {
                let items = collection(&self.body, collection_name)?;
                let item = items.get(index).ok_or_else(|| CodecError::InvalidField {
                    field: collection_name.into(),
                    expected: "the validated current user item",
                })?;
                object(item)?.get("content").ok_or_else(|| {
                    CodecError::MissingField(format!("{collection_name}[{index}].content"))
                })
            }
        }
    }

    /// 内部：clone 本请求，应用覆盖，并在当前 user 文本后追加分隔符 + suffix。
    /// Internal: clone, apply overrides, append a separator + suffix to the current user text.
    fn with_user_suffix(
        &self,
        suffix: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        let mut phase = self.clone();
        phase.apply_overrides(overrides)?;
        // 追加格式：空行 + 分隔线 + 空行 + suffix / Append format: blank + separator + blank + suffix.
        let addition = format!("\n\n---\n\n{suffix}");

        match phase.current_user {
            CurrentUserLocation::ResponsesString => {
                // Responses 字符串 input：在字符串后追加 / Responses string input: append to the string.
                let input = object_mut(&mut phase.body)?
                    .get_mut("input")
                    .ok_or_else(|| CodecError::MissingField("input".into()))?;
                append_to_text_content(input, &addition, phase.protocol)?;
            }
            CurrentUserLocation::CollectionItem { collection, index } => {
                // 数组集合：在 messages[index].content 后追加 / Array collection: append to messages[index].content.
                let items = collection_mut(&mut phase.body, collection)?;
                let item = items
                    .get_mut(index)
                    .ok_or_else(|| CodecError::InvalidField {
                        field: collection.into(),
                        expected: "the validated current user item",
                    })?;
                let content = object_mut(item)?.get_mut("content").ok_or_else(|| {
                    CodecError::MissingField(format!("{collection}[{index}].content"))
                })?;
                append_to_text_content(content, &addition, phase.protocol)?;
            }
        }
        Ok(phase)
    }

    /// 内部：应用内部传输覆盖（model 和/或 stream）。
    /// Internal: apply internal transport overrides (model and/or stream).
    fn apply_overrides(
        &mut self,
        overrides: &InternalTransportOverrides,
    ) -> Result<(), CodecError> {
        let body = object_mut(&mut self.body)?;
        // 覆盖 model / Override model.
        if let Some(model) = &overrides.model {
            body.insert("model".into(), Value::String(model.clone()));
        }
        // 覆盖 stream（同时更新 self.stream）/ Override stream (also update self.stream).
        if let Some(stream) = overrides.stream {
            body.insert("stream".into(), Value::Bool(stream));
            self.stream = stream;
        }
        Ok(())
    }
}

/// 内部传输显式要求的 model 和 stream 变更。
/// Model and stream changes explicitly required by internal transport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InternalTransportOverrides {
    /// 可选的上游模型标识符（真实模型名，不带 -pig）。
    /// Optional upstream model identifier.
    pub model: Option<String>,
    /// 可选的上游流式模式。
    /// Optional upstream streaming mode.
    pub stream: Option<bool>,
}

/// 原生对话记录条目的语义类别。
/// Semantic category of a protocol-native transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTranscriptKind {
    /// assistant 消息或 output item。
    /// Assistant message or output item.
    Assistant,
    /// 模型推理/thinking 条目。
    /// Model reasoning or thinking item.
    Reasoning,
    /// 工具调用条目。
    /// Tool call item.
    ToolCall,
    /// 工具结果条目。
    /// Tool result item.
    ToolResult,
    /// 当前语义类别未知的原生条目。
    /// A native item with a currently unknown semantic category.
    Other,
}

/// 一个完整的、可保留在相位对话记录中的原生条目。
/// A complete protocol-native item ready to be retained in a phase transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTranscriptItem {
    /// 接受此条目的请求历史所属协议 / protocol whose request history accepts this item.
    pub protocol: Protocol,
    /// 编排用的语义类别（不修改原生 JSON）/ semantic category used by orchestration without altering native JSON.
    pub kind: NativeTranscriptKind,
    /// 完整的原生 message / content-block 容器 / output item。
    /// Complete native message, content-block container, or output item.
    pub value: Value,
}

impl NativeTranscriptItem {
    /// 创建一个 transcript 条目，保留完整的原生 JSON 值。
    /// Creates a transcript item while retaining the complete native JSON value.
    pub fn new(protocol: Protocol, kind: NativeTranscriptKind, value: Value) -> Self {
        Self {
            protocol,
            kind,
            value,
        }
    }
}

/// 完整的原生工具结果历史条目及其满足的工具调用 ID。
/// Complete native tool-result history item and the IDs it satisfies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolResultGroup {
    /// 本原生历史条目包含的工具调用 ID / tool-call identifiers contained in this native history item.
    pub ids: Vec<String>,
    /// 可追加到相位对话记录的完整原生条目 / complete native item suitable for appending to a phase transcript.
    pub item: NativeTranscriptItem,
}

/// 原生工具调用的协议中立视图。
/// Protocol-neutral view of a native tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolCall {
    /// 用于关联后续工具结果的标识符 / identifier used to correlate a later tool result.
    pub id: String,
    /// 工具名 / tool name.
    pub name: String,
    /// 原生参数值。字符串参数保持为字符串 / native arguments value; string arguments stay strings.
    pub arguments: Value,
    /// 完整的原生 tool-call 对象或块 / complete native tool-call object or block.
    pub native: Value,
}

/// 非流式输出，仅归一化到相位编排所需程度。
/// Non-streaming output normalized only as far as phase orchestration requires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedModelOutput {
    /// 拼接后的用户可见响应文本 / concatenated user-visible response text.
    pub visible_text: String,
    /// 完整的协议原生 assistant/output 条目 / complete protocol-native assistant/output items.
    pub items: Vec<NativeTranscriptItem>,
    /// 抽取出的工具调用（保留其原生表示）/ tool calls extracted without discarding their native representation.
    pub tool_calls: Vec<NativeToolCall>,
    /// 关联请求中提供的工具结果 ID / tool result identifiers supplied by the associated incoming request.
    pub tool_result_ids: Vec<String>,
    /// 原生 stop_reason（或 incomplete 时的原因）/ native stop reason, or an incomplete reason when applicable.
    pub stop_reason: Option<String>,
    /// 协议提供的原生响应状态 / native response status when the protocol provides one.
    pub status: Option<String>,
    /// 完整的原生 usage 值 / complete native usage value.
    pub usage: Option<Value>,
}

/// 校验、变更或抽取协议 JSON 时产生的错误。
/// Errors produced while validating, mutating, or extracting protocol JSON.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// 完整的请求或响应体不是对象。
    /// The complete request or response body is not an object.
    #[error("body must be a JSON object")]
    BodyNotObject,
    /// 缺少必填字段。
    /// A required field is absent.
    #[error("missing required field `{0}`")]
    MissingField(String),
    /// 字段的 JSON 形状错误。
    /// A field has the wrong JSON shape.
    #[error("field `{field}` must be {expected}")]
    InvalidField {
        /// 字段路径 / field path.
        field: String,
        /// 期望的形状 / expected shape.
        expected: &'static str,
    },
    /// 找不到任何对话消息。
    /// No conversational message could be located.
    #[error("{protocol} request has no conversational input")]
    MissingConversationalInput {
        /// 正在解析的协议 / protocol being parsed.
        protocol: Protocol,
    },
    /// 最后一条对话输入不是 user 消息。
    /// The final conversational input is not a user message.
    #[error("{protocol} request's last conversational input has role `{role}`, not `user`")]
    LastInputNotUser {
        /// 正在解析的协议 / protocol being parsed.
        protocol: Protocol,
        /// 最后一条对话角色 / last conversational role.
        role: String,
    },
    /// transcript 条目属于另一个协议。
    /// A transcript item belongs to another protocol.
    #[error("native transcript protocol mismatch: expected {expected}, got {actual}")]
    TranscriptProtocolMismatch {
        /// 请求协议 / request protocol.
        expected: Protocol,
        /// transcript 条目协议 / transcript item protocol.
        actual: Protocol,
    },
    /// 响应与另一协议的请求关联。
    /// A response was associated with a request from another protocol.
    #[error("request protocol mismatch: expected {expected}, got {actual}")]
    RequestProtocolMismatch {
        /// codec 协议 / codec protocol.
        expected: Protocol,
        /// 请求协议 / request protocol.
        actual: Protocol,
    },
}

/// 单一协议的解析器与抽取器。
/// Parser and extractor for one protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolCodec {
    /// 本 codec 处理的协议 / protocol handled by this codec.
    protocol: Protocol,
}

impl ProtocolCodec {
    /// 为指定协议创建 codec / create a codec for `protocol`.
    pub fn new(protocol: Protocol) -> Self {
        Self { protocol }
    }

    /// 返回本 codec 处理的协议 / return the protocol handled by this codec.
    pub fn protocol(self) -> Protocol {
        self.protocol
    }

    /// 校验完整的 body 并用传输中立的 HTTP 元数据包装。
    ///
    /// 解析流程：
    /// 1. body 必须是对象；
    /// 2. 取必填的 `model` 字符串；
    /// 3. 取可选的 `stream` 布尔；
    /// 4. 按协议校验 messages/input，定位当前 user 与尾部工具结果；
    /// 5. 从 `client_model` 去掉 `-pig` 得到 `real_model`。
    ///
    /// Validates a complete body and wraps it with transport-neutral HTTP metadata.
    pub fn parse_request(
        &self,
        method: impl Into<String>,
        path_and_query: impl Into<String>,
        headers: Vec<HeaderPair>,
        body: Value,
    ) -> Result<HttpRequestEnvelope, CodecError> {
        let body_object = object(&body)?;
        // model 必填 / model is required.
        let client_model = required_string(body_object, "model")?.to_owned();
        // stream 可选，缺省 false / stream is optional, defaults to false.
        let stream = match body_object.get("stream") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(CodecError::InvalidField {
                    field: "stream".into(),
                    expected: "a boolean",
                });
            }
        };
        // 按协议校验并定位当前 user / Validate and locate current user per protocol.
        let (current_user, tool_result_groups) = match self.protocol {
            // Chat：允许 user content 为 null（兼容某些客户端）
            // Chat: allow null user content (some clients send null).
            Protocol::OpenAiChat => {
                validate_message_request(&body, self.protocol, "messages", true)?
            }
            // Anthropic：不允许 null content / Anthropic: disallow null content.
            Protocol::AnthropicMessages => {
                validate_message_request(&body, self.protocol, "messages", false)?
            }
            // Responses：input 可以是字符串或数组 / Responses: input may be a string or array.
            Protocol::OpenAiResponses => validate_responses_request(&body)?,
        };
        // 去掉一个 -pig 后缀得到真实模型 / Strip one -pig suffix to get the real model.
        let real_model = client_model
            .strip_suffix("-pig")
            .unwrap_or(client_model.as_str())
            .to_owned();
        // 收集所有工具结果 ID（不只是尾部的）/ Collect all tool-result IDs (not just trailing).
        let tool_result_ids = collect_tool_result_ids(self.protocol, &body);

        Ok(HttpRequestEnvelope {
            method: method.into(),
            path_and_query: path_and_query.into(),
            headers,
            body,
            protocol: self.protocol,
            client_model,
            real_model,
            stream,
            current_user,
            tool_result_ids,
            tool_result_groups,
        })
    }

    /// 把非流式 JSON 响应抽取为编排就绪的原生输出。
    /// Extracts a non-streaming JSON response into orchestration-ready native output.
    pub fn extract_response(&self, response: &Value) -> Result<NormalizedModelOutput, CodecError> {
        match self.protocol {
            Protocol::OpenAiChat => extract_chat_response(response),
            Protocol::AnthropicMessages => extract_anthropic_response(response),
            Protocol::OpenAiResponses => extract_responses_response(response),
        }
    }

    /// 抽取响应并从关联请求中继承工具结果 ID。
    /// Extracts a response and carries forward tool result IDs from its incoming request.
    pub fn extract_response_for_request(
        &self,
        request: &HttpRequestEnvelope,
        response: &Value,
    ) -> Result<NormalizedModelOutput, CodecError> {
        // 协议必须一致 / Protocol must match.
        if request.protocol != self.protocol {
            return Err(CodecError::RequestProtocolMismatch {
                expected: self.protocol,
                actual: request.protocol,
            });
        }
        let mut output = self.extract_response(response)?;
        // 继承请求中的工具结果 ID / Carry forward the request's tool-result IDs.
        output.tool_result_ids.clone_from(&request.tool_result_ids);
        Ok(output)
    }
}

// =============================================================================
// 辅助函数：JSON 访问与校验 / Helpers: JSON access & validation
// =============================================================================

/// 返回 Value 的对象引用，否则 BodyNotObject 错误。
/// Return the value's object reference or a BodyNotObject error.
fn object(value: &Value) -> Result<&Map<String, Value>, CodecError> {
    value.as_object().ok_or(CodecError::BodyNotObject)
}

/// 返回 Value 的可变对象引用，否则 BodyNotObject 错误。
/// Return the value's mutable object reference or a BodyNotObject error.
fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>, CodecError> {
    value.as_object_mut().ok_or(CodecError::BodyNotObject)
}

/// 取必填字符串字段（path 与 key 同名）。
/// Required string field where path == key.
fn required_string<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, CodecError> {
    required_string_at(object, field, field)
}

/// 取必填字符串字段，允许 path 与 key 不同（用于嵌套路径的错误信息）。
/// Required string field with a distinct path (for nested error messages).
fn required_string_at<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'a str, CodecError> {
    match object.get(key) {
        None => Err(CodecError::MissingField(path.into())),
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(CodecError::InvalidField {
            field: path.into(),
            expected: "a string",
        }),
    }
}

/// 取 body 中名为 `name` 的数组字段引用。
/// Get a reference to the array field named `name` in the body.
fn collection<'a>(body: &'a Value, name: &str) -> Result<&'a Vec<Value>, CodecError> {
    match object(body)?.get(name) {
        None => Err(CodecError::MissingField(name.into())),
        Some(Value::Array(values)) => Ok(values),
        Some(_) => Err(CodecError::InvalidField {
            field: name.into(),
            expected: "an array",
        }),
    }
}

/// 取 body 中名为 `name` 的可变数组字段引用。
/// Get a mutable reference to the array field named `name` in the body.
fn collection_mut<'a>(body: &'a mut Value, name: &str) -> Result<&'a mut Vec<Value>, CodecError> {
    match object_mut(body)?.get_mut(name) {
        None => Err(CodecError::MissingField(name.into())),
        Some(Value::Array(values)) => Ok(values),
        Some(_) => Err(CodecError::InvalidField {
            field: name.into(),
            expected: "an array",
        }),
    }
}

/// 校验 OpenAI Chat / Anthropic Messages 风格的请求，定位当前 user 与尾部工具结果。
/// Validate an OpenAI Chat / Anthropic Messages style request, locating current user and trailing tool results.
fn validate_message_request(
    body: &Value,
    protocol: Protocol,
    field: &'static str,
    allow_null_content: bool,
) -> Result<(CurrentUserLocation, Vec<NativeToolResultGroup>), CodecError> {
    let messages = collection(body, field)?;
    // 空消息数组 → 无对话输入 / Empty messages → no conversational input.
    if messages.is_empty() {
        return Err(CodecError::MissingConversationalInput { protocol });
    }

    // 逐条校验 role 与 content / Validate role and content of each message.
    for (index, message) in messages.iter().enumerate() {
        let message_object = message
            .as_object()
            .ok_or_else(|| CodecError::InvalidField {
                field: format!("{field}[{index}]"),
                expected: "an object",
            })?;
        let role_field = format!("{field}[{index}].role");
        let role = required_string_at(message_object, "role", &role_field)?;
        let content_field = format!("{field}[{index}].content");
        match message_object.get("content") {
            // user 消息必须有 content / user messages must have content.
            None if role == "user" => return Err(CodecError::MissingField(content_field)),
            None => {}
            Some(content) => validate_content(content, &content_field, allow_null_content)?,
        }
    }

    // 收集尾部的工具结果条目 / Collect trailing tool-result messages.
    let trailing_results = trailing_message_tool_results(messages, protocol);
    // 定位当前 user 条目：无工具结果时是最后一条；有时是工具结果之前的最后一条 user
    // Locate the current user: last message if no tool results, else the last user before them.
    let current_index = if trailing_results.is_empty() {
        messages.len() - 1
    } else {
        let search_end = messages.len() - trailing_results.len();
        messages[..search_end]
            .iter()
            .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .ok_or(CodecError::MissingConversationalInput { protocol })?
    };
    // 校验当前条目是 user / Validate the current item is a user message.
    let message = messages[current_index]
        .as_object()
        .ok_or_else(|| CodecError::InvalidField {
            field: format!("{field}[{current_index}]"),
            expected: "an object",
        })?;
    let role = required_string_at(message, "role", &format!("{field}[{current_index}].role"))?;
    if role != "user" {
        return Err(CodecError::LastInputNotUser {
            protocol,
            role: role.into(),
        });
    }

    Ok((
        CurrentUserLocation::CollectionItem {
            collection: field,
            index: current_index,
        },
        trailing_results,
    ))
}

/// 校验 OpenAI Responses 风格的请求（input 为字符串或数组）。
/// Validate an OpenAI Responses style request (input is a string or array).
fn validate_responses_request(
    body: &Value,
) -> Result<(CurrentUserLocation, Vec<NativeToolResultGroup>), CodecError> {
    let input = object(body)?
        .get("input")
        .ok_or_else(|| CodecError::MissingField("input".into()))?;
    match input {
        // 字符串 input → 整个字符串就是当前 user / String input → the whole string is the current user.
        Value::String(_) => Ok((CurrentUserLocation::ResponsesString, Vec::new())),
        Value::Array(items) => {
            if items.is_empty() {
                return Err(CodecError::MissingConversationalInput {
                    protocol: Protocol::OpenAiResponses,
                });
            }
            // 收集尾部的 function_call_output / Collect trailing function_call_output items.
            let trailing_results = trailing_responses_tool_results(items);
            // 定位当前 user message 条目 / Locate the current user message item.
            let index = if trailing_results.is_empty() {
                items.len() - 1
            } else {
                let search_end = items.len() - trailing_results.len();
                items[..search_end]
                    .iter()
                    .rposition(|item| {
                        item.get("role").and_then(Value::as_str) == Some("user")
                            && matches!(
                                item.get("type").and_then(Value::as_str),
                                None | Some("message")
                            )
                    })
                    .ok_or(CodecError::MissingConversationalInput {
                        protocol: Protocol::OpenAiResponses,
                    })?
            };
            let item_object = items[index]
                .as_object()
                .ok_or_else(|| CodecError::InvalidField {
                    field: format!("input[{index}]"),
                    expected: "an object",
                })?;
            // 解析 type 字段（缺省且有 role 时视为 "message"）/ Parse the type field.
            let item_type = match item_object.get("type") {
                None if item_object.contains_key("role") => "message",
                None => "unknown",
                Some(Value::String(value)) => value.as_str(),
                Some(_) => {
                    return Err(CodecError::InvalidField {
                        field: format!("input[{index}].type"),
                        expected: "a string",
                    });
                }
            };
            if item_type != "message" {
                return Err(CodecError::LastInputNotUser {
                    protocol: Protocol::OpenAiResponses,
                    role: item_type.into(),
                });
            }
            let role = required_string_at(item_object, "role", &format!("input[{index}].role"))?;
            let content = item_object
                .get("content")
                .ok_or_else(|| CodecError::MissingField(format!("input[{index}].content")))?;
            validate_content(content, &format!("input[{index}].content"), false)?;
            if role != "user" {
                return Err(CodecError::LastInputNotUser {
                    protocol: Protocol::OpenAiResponses,
                    role: role.into(),
                });
            }
            Ok((
                CurrentUserLocation::CollectionItem {
                    collection: "input",
                    index,
                },
                trailing_results,
            ))
        }
        _ => Err(CodecError::InvalidField {
            field: "input".into(),
            expected: "a string or array",
        }),
    }
}

/// 从 Chat/Anthropic 的 messages 数组尾部收集工具结果条目。
/// Collect trailing tool-result messages from a Chat/Anthropic messages array.
fn trailing_message_tool_results(
    messages: &[Value],
    protocol: Protocol,
) -> Vec<NativeToolResultGroup> {
    let mut groups = Vec::new();
    // 从后往前扫描，遇到非工具结果即停 / Scan from the end, stop at the first non-result.
    for message in messages.iter().rev() {
        let ids = match protocol {
            // Chat：role=tool 的消息，取 tool_call_id / Chat: role=tool messages, take tool_call_id.
            Protocol::OpenAiChat if message.get("role").and_then(Value::as_str) == Some("tool") => {
                message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(|id| vec![id.to_owned()])
            }
            // Anthropic：role=user 且 content 全是 tool_result 块 / Anthropic: role=user with all-tool_result content.
            Protocol::AnthropicMessages
                if message.get("role").and_then(Value::as_str) == Some("user") =>
            {
                let parts = message.get("content").and_then(Value::as_array);
                parts.and_then(|parts| {
                    // 空数组或含非 tool_result 块 → 不是工具结果 / Empty or has non-tool_result → not a result.
                    if parts.is_empty()
                        || parts.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) != Some("tool_result")
                        })
                    {
                        return None;
                    }
                    let ids: Vec<String> = parts
                        .iter()
                        .filter_map(|part| {
                            part.get("tool_use_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect();
                    // 所有块都必须有 tool_use_id / Every block must have a tool_use_id.
                    (ids.len() == parts.len()).then_some(ids)
                })
            }
            _ => None,
        };
        let Some(ids) = ids else {
            break;
        };
        groups.push(NativeToolResultGroup {
            ids,
            item: NativeTranscriptItem::new(
                protocol,
                NativeTranscriptKind::ToolResult,
                message.clone(),
            ),
        });
    }
    // 反转回原始顺序 / Reverse back to original order.
    groups.reverse();
    groups
}

/// 从 Responses 的 input 数组尾部收集 function_call_output 条目。
/// Collect trailing function_call_output items from a Responses input array.
fn trailing_responses_tool_results(items: &[Value]) -> Vec<NativeToolResultGroup> {
    let mut groups = Vec::new();
    for item in items.iter().rev() {
        // 非 function_call_output 即停 / Stop at the first non-function_call_output.
        if item.get("type").and_then(Value::as_str) != Some("function_call_output") {
            break;
        }
        // 必须有 call_id / Must have a call_id.
        let Some(id) = item.get("call_id").and_then(Value::as_str) else {
            break;
        };
        groups.push(NativeToolResultGroup {
            ids: vec![id.to_owned()],
            item: NativeTranscriptItem::new(
                Protocol::OpenAiResponses,
                NativeTranscriptKind::ToolResult,
                item.clone(),
            ),
        });
    }
    groups.reverse();
    groups
}

/// 校验 content 字段的形状（字符串、数组，可选 null）。
/// Validate the shape of a content field (string, array, optionally null).
fn validate_content(value: &Value, field: &str, allow_null: bool) -> Result<(), CodecError> {
    match value {
        Value::String(_) | Value::Array(_) => Ok(()),
        Value::Null if allow_null => Ok(()),
        _ => Err(CodecError::InvalidField {
            field: field.into(),
            expected: if allow_null {
                "a string, content-part array, or null"
            } else {
                "a string or content-part array"
            },
        }),
    }
}

/// 从 content-part 数组中拼接所有文本部分。
/// Concatenate all text parts from a content-part array.
fn text_from_parts(parts: &[Value], protocol: Protocol) -> String {
    let mut text = String::new();
    for part in parts {
        if is_text_part(part, protocol) {
            if let Some(value) = part.get("text").and_then(Value::as_str) {
                text.push_str(value);
            }
        }
    }
    text
}

/// 判断一个 content part 是否是文本类型（按协议）。
/// Whether a content part is a text type (per protocol).
fn is_text_part(part: &Value, protocol: Protocol) -> bool {
    let part_type = part.get("type").and_then(Value::as_str);
    match protocol {
        Protocol::OpenAiChat => matches!(part_type, Some("text") | Some("input_text")),
        Protocol::AnthropicMessages => part_type == Some("text"),
        Protocol::OpenAiResponses => matches!(part_type, Some("input_text") | Some("output_text")),
    }
}

/// 在当前 user 的 content 上追加文本（字符串或数组最后一段）。
/// Append text to the current user's content (string or the last text part of an array).
fn append_to_text_content(
    content: &mut Value,
    addition: &str,
    protocol: Protocol,
) -> Result<(), CodecError> {
    match content {
        // 字符串：直接 push_str / String: push_str directly.
        Value::String(text) => text.push_str(addition),
        // 数组：找最后一个文本 part 追加；没有则新建一个 / Array: append to the last text part, or create one.
        Value::Array(parts) => {
            if let Some(text) = parts.iter_mut().rev().find_map(|part| {
                if !is_text_part(part, protocol) {
                    return None;
                }
                match part.get_mut("text") {
                    Some(Value::String(text)) => Some(text),
                    _ => None,
                }
            }) {
                text.push_str(addition);
            } else {
                // 没有文本 part → 新建一个 / No text part → create one.
                let part_type = match protocol {
                    Protocol::OpenAiChat | Protocol::AnthropicMessages => "text",
                    Protocol::OpenAiResponses => "input_text",
                };
                parts.push(json!({"type": part_type, "text": addition}));
            }
        }
        _ => {
            return Err(CodecError::InvalidField {
                field: "current user content".into(),
                expected: "a string or content-part array",
            });
        }
    }
    Ok(())
}

/// 按协议构造一条 user review 消息（用于 Post 相位）。
/// Build a user review message per protocol (for the Post phase).
fn review_message(protocol: Protocol, prompt: &str) -> Value {
    match protocol {
        // Chat / Anthropic：role=user + content=字符串 / Chat / Anthropic: role=user + content=string.
        Protocol::OpenAiChat | Protocol::AnthropicMessages => {
            json!({"role": "user", "content": prompt})
        }
        // Responses：type=message + content=input_text 数组 / Responses: type=message + content=input_text array.
        Protocol::OpenAiResponses => json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}]
        }),
    }
}

/// 重新解析 body 中的尾部工具结果组（用于 body 变更后）。
/// Re-parse trailing tool-result groups from the body (after mutations).
fn trailing_tool_result_groups(protocol: Protocol, body: &Value) -> Vec<NativeToolResultGroup> {
    match protocol {
        Protocol::OpenAiChat | Protocol::AnthropicMessages => collection(body, "messages")
            .map(|messages| trailing_message_tool_results(messages, protocol))
            .unwrap_or_default(),
        Protocol::OpenAiResponses => body
            .get("input")
            .and_then(Value::as_array)
            .map(|items| trailing_responses_tool_results(items))
            .unwrap_or_default(),
    }
}

/// 收集 body 中所有的工具结果 ID（不只尾部）。
/// Collect all tool-result IDs in the body (not just trailing).
fn collect_tool_result_ids(protocol: Protocol, body: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    match protocol {
        // Chat：role=tool 的 tool_call_id / Chat: tool_call_id from role=tool messages.
        Protocol::OpenAiChat => {
            if let Ok(messages) = collection(body, "messages") {
                for message in messages {
                    if message.get("role").and_then(Value::as_str) == Some("tool") {
                        push_string_field(&mut ids, message, "tool_call_id");
                    }
                }
            }
        }
        // Anthropic：content 中 type=tool_result 的 tool_use_id / Anthropic: tool_use_id from tool_result parts.
        Protocol::AnthropicMessages => {
            if let Ok(messages) = collection(body, "messages") {
                for message in messages {
                    if let Some(parts) = message.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if part.get("type").and_then(Value::as_str) == Some("tool_result") {
                                push_string_field(&mut ids, part, "tool_use_id");
                            }
                        }
                    }
                }
            }
        }
        // Responses：type=function_call_output 的 call_id / Responses: call_id from function_call_output.
        Protocol::OpenAiResponses => {
            if let Some(items) = body.get("input").and_then(Value::as_array) {
                for item in items {
                    if item.get("type").and_then(Value::as_str) == Some("function_call_output") {
                        push_string_field(&mut ids, item, "call_id");
                    }
                }
            }
        }
    }
    ids
}

/// 从 JSON 对象中取出一个字符串字段并推入目标向量。
/// Push a string field from a JSON object into the target vector.
fn push_string_field(target: &mut Vec<String>, value: &Value, field: &str) {
    if let Some(id) = value.get(field).and_then(Value::as_str) {
        target.push(id.to_owned());
    }
}

// =============================================================================
// 响应抽取 / Response extraction
// =============================================================================

/// 抽取 OpenAI Chat 非流式响应 / Extract an OpenAI Chat non-streaming response.
fn extract_chat_response(response: &Value) -> Result<NormalizedModelOutput, CodecError> {
    // choices 必须是非空数组 / choices must be a non-empty array.
    let choices = response
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| CodecError::InvalidField {
            field: "choices".into(),
            expected: "a non-empty array",
        })?;
    // 取第一个 choice / Take the first choice.
    let choice =
        choices
            .first()
            .and_then(Value::as_object)
            .ok_or_else(|| CodecError::InvalidField {
                field: "choices[0]".into(),
                expected: "an object",
            })?;
    // message 必须是对象 / message must be an object.
    let message = choice
        .get("message")
        .filter(|value| value.is_object())
        .ok_or_else(|| CodecError::InvalidField {
            field: "choices[0].message".into(),
            expected: "an object",
        })?;
    // 可见文本 = message.content 的文本部分 / Visible text = the text portion of message.content.
    let visible_text = content_text(message.get("content"), Protocol::OpenAiChat);
    let mut tool_calls = Vec::new();
    // 解析 tool_calls（可选）/ Parse tool_calls (optional).
    if let Some(calls) = message.get("tool_calls") {
        let calls = calls.as_array().ok_or_else(|| CodecError::InvalidField {
            field: "choices[0].message.tool_calls".into(),
            expected: "an array",
        })?;
        for (index, call) in calls.iter().enumerate() {
            // 每个工具调用必须有 function 对象 / Each call must have a function object.
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| CodecError::InvalidField {
                    field: format!("choices[0].message.tool_calls[{index}].function"),
                    expected: "an object",
                })?;
            let id = value_string(call, "id", &format!("tool_calls[{index}].id"))?;
            let name = required_string_at(
                function,
                "name",
                &format!("tool_calls[{index}].function.name"),
            )?;
            let arguments = function.get("arguments").cloned().ok_or_else(|| {
                CodecError::MissingField(format!("tool_calls[{index}].function.arguments"))
            })?;
            tool_calls.push(NativeToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments,
                // 保留完整的原生 call 对象 / Retain the complete native call object.
                native: call.clone(),
            });
        }
    }

    Ok(NormalizedModelOutput {
        visible_text,
        items: vec![NativeTranscriptItem::new(
            Protocol::OpenAiChat,
            NativeTranscriptKind::Assistant,
            message.clone(),
        )],
        tool_calls,
        tool_result_ids: Vec::new(),
        // finish_reason 作为 stop_reason / finish_reason as stop_reason.
        stop_reason: optional_string(choice.get("finish_reason"), "choices[0].finish_reason")?,
        status: None,
        usage: response.get("usage").cloned(),
    })
}

/// 抽取 Anthropic 非流式响应 / Extract an Anthropic non-streaming response.
fn extract_anthropic_response(response: &Value) -> Result<NormalizedModelOutput, CodecError> {
    // content 必须是数组 / content must be an array.
    let content = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| CodecError::InvalidField {
            field: "content".into(),
            expected: "an array",
        })?;
    // 可见文本 = content 中所有 text 块 / Visible text = all text blocks in content.
    let visible_text = text_from_parts(content, Protocol::AnthropicMessages);
    let mut tool_calls = Vec::new();
    // 解析 tool_use 块 / Parse tool_use blocks.
    for (index, block) in content.iter().enumerate() {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let id = value_string(block, "id", &format!("content[{index}].id"))?;
        let name = value_string(block, "name", &format!("content[{index}].name"))?;
        // input 字段是工具参数 / The input field holds the tool arguments.
        let arguments = block
            .get("input")
            .cloned()
            .ok_or_else(|| CodecError::MissingField(format!("content[{index}].input")))?;
        tool_calls.push(NativeToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            // 保留完整的原生 block / Retain the complete native block.
            native: block.clone(),
        });
    }

    Ok(NormalizedModelOutput {
        visible_text,
        items: vec![NativeTranscriptItem::new(
            Protocol::AnthropicMessages,
            NativeTranscriptKind::Assistant,
            // 重新包装为 {role:assistant, content} / Re-wrap as {role:assistant, content}.
            json!({"role": "assistant", "content": content}),
        )],
        tool_calls,
        tool_result_ids: Vec::new(),
        stop_reason: optional_string(response.get("stop_reason"), "stop_reason")?,
        status: optional_string(response.get("status"), "status")?,
        usage: response.get("usage").cloned(),
    })
}

/// 抽取 OpenAI Responses 非流式响应 / Extract an OpenAI Responses non-streaming response.
fn extract_responses_response(response: &Value) -> Result<NormalizedModelOutput, CodecError> {
    // output 必须是数组 / output must be an array.
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| CodecError::InvalidField {
            field: "output".into(),
            expected: "an array",
        })?;
    let mut visible_text = String::new();
    let mut items = Vec::with_capacity(output.len());
    let mut tool_calls = Vec::new();

    // 逐个 output item 解析 / Parse each output item.
    for (index, item) in output.iter().enumerate() {
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let kind = match item_type {
            // message → 拼接可见文本 / message → concatenate visible text.
            "message" => {
                visible_text.push_str(&content_text(
                    item.get("content"),
                    Protocol::OpenAiResponses,
                ));
                NativeTranscriptKind::Assistant
            }
            // reasoning → 保留为推理条目 / reasoning → keep as a reasoning item.
            "reasoning" => NativeTranscriptKind::Reasoning,
            // function_call → 解析工具调用 / function_call → parse a tool call.
            "function_call" => {
                // call_id 优先，回退到 id / Prefer call_id, fall back to id.
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| CodecError::InvalidField {
                        field: format!("output[{index}].call_id"),
                        expected: "a string",
                    })?;
                let name = value_string(item, "name", &format!("output[{index}].name"))?;
                let arguments = item.get("arguments").cloned().ok_or_else(|| {
                    CodecError::MissingField(format!("output[{index}].arguments"))
                })?;
                tool_calls.push(NativeToolCall {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    arguments,
                    // 保留完整的原生 item / Retain the complete native item.
                    native: item.clone(),
                });
                NativeTranscriptKind::ToolCall
            }
            // function_call_output → 工具结果 / function_call_output → tool result.
            "function_call_output" => NativeTranscriptKind::ToolResult,
            // 未知类型 → Other / Unknown type → Other.
            _ => NativeTranscriptKind::Other,
        };
        items.push(NativeTranscriptItem::new(
            Protocol::OpenAiResponses,
            kind,
            item.clone(),
        ));
    }

    let status = optional_string(response.get("status"), "status")?;
    // Responses 的 stop_reason 来自 incomplete_details.reason（若存在）
    // Responses' stop_reason comes from incomplete_details.reason if present.
    let stop_reason = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .map(|reason| {
            reason
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| CodecError::InvalidField {
                    field: "incomplete_details.reason".into(),
                    expected: "a string",
                })
        })
        .transpose()?;

    Ok(NormalizedModelOutput {
        visible_text,
        items,
        tool_calls,
        tool_result_ids: Vec::new(),
        stop_reason,
        status,
        usage: response.get("usage").cloned(),
    })
}

/// 从 content 值（字符串或数组）提取文本。
/// Extract text from a content value (string or array).
fn content_text(content: Option<&Value>, protocol: Protocol) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => text_from_parts(parts, protocol),
        _ => String::new(),
    }
}

/// 取一个必填字符串字段（值必须是字符串，否则错误）。
/// Take a required string field (must be a string or error).
fn value_string<'a>(value: &'a Value, field: &str, path: &str) -> Result<&'a str, CodecError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| CodecError::InvalidField {
            field: path.into(),
            expected: "a string",
        })
}

/// 取一个可选字符串字段（null 或缺失返回 None）。
/// Take an optional string field (null/absent → None).
fn optional_string(value: Option<&Value>, field: &str) -> Result<Option<String>, CodecError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(CodecError::InvalidField {
            field: field.into(),
            expected: "a string or null",
        }),
    }
}
