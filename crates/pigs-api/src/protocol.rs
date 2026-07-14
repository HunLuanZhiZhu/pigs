//! Protocol-native request preservation, phase mutation, and response extraction.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// API protocol used by the client and upstream model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// OpenAI Chat Completions.
    OpenAiChat,
    /// Anthropic Messages.
    AnthropicMessages,
    /// OpenAI Responses.
    OpenAiResponses,
}

impl fmt::Display for Protocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OpenAiChat => "openai_chat",
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenAiResponses => "openai_responses",
        })
    }
}

/// A transport-neutral HTTP header name and value pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPair {
    /// Header name as received from the transport.
    pub name: String,
    /// Raw header value bytes as received from the transport.
    pub value: Vec<u8>,
}

impl HeaderPair {
    /// Creates a header pair without depending on an HTTP crate.
    pub fn new(name: impl Into<String>, value: impl AsRef<[u8]>) -> Self {
        Self {
            name: name.into(),
            value: value.as_ref().to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CurrentUserLocation {
    ResponsesString,
    CollectionItem {
        collection: &'static str,
        index: usize,
    },
}

/// A validated client request with its complete protocol-native JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequestEnvelope {
    /// HTTP method without an `http::Method` dependency.
    pub method: String,
    /// Request path including its optional query string.
    pub path_and_query: String,
    /// Header pairs in transport-provided order.
    pub headers: Vec<HeaderPair>,
    /// Complete request body. Unknown fields and native blocks remain intact.
    pub body: Value,
    /// Protocol used by the request.
    pub protocol: Protocol,
    /// Model identifier supplied by the client.
    pub client_model: String,
    /// Client model with exactly one trailing `-pig` removed.
    pub real_model: String,
    /// Whether the request body asks for a streaming response.
    pub stream: bool,
    current_user: CurrentUserLocation,
    tool_result_ids: Vec<String>,
    tool_result_groups: Vec<NativeToolResultGroup>,
}

impl HttpRequestEnvelope {
    /// Returns the concatenated textual portion of the current user input.
    pub fn current_user_text(&self) -> Result<String, CodecError> {
        match self.current_content()? {
            Value::String(text) => Ok(text.clone()),
            Value::Array(parts) => Ok(text_from_parts(parts, self.protocol)),
            _ => Err(CodecError::InvalidField {
                field: "current user content".into(),
                expected: "a string or content-part array",
            }),
        }
    }

    /// Returns tool result identifiers found in the incoming native request.
    pub fn tool_result_ids(&self) -> &[String] {
        &self.tool_result_ids
    }

    /// Whether this request resumes a paused native tool call.
    pub fn is_continuation(&self) -> bool {
        !self.tool_result_groups.is_empty()
    }

    /// Returns complete trailing native tool-result groups for continuation matching.
    pub fn tool_result_groups(&self) -> &[NativeToolResultGroup] {
        &self.tool_result_groups
    }

    /// Clones this request for Pre and appends `suffix` to the current user text.
    pub fn for_pre(
        &self,
        suffix: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        self.with_user_suffix(suffix, overrides)
    }

    /// Clones this request for Executor and appends `suffix` to the current user text.
    pub fn for_executor(
        &self,
        suffix: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        self.with_user_suffix(suffix, overrides)
    }

    /// Builds a Post request from original history, native transcript items, and a new prompt.
    pub fn for_post(
        &self,
        transcript: &[NativeTranscriptItem],
        review_prompt: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
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
        phase.apply_overrides(overrides)?;
        let transcript_values = transcript.iter().map(|item| item.value.clone());

        phase.current_user = match self.current_user {
            CurrentUserLocation::ResponsesString => {
                let mut input: Vec<Value> = transcript_values.collect();
                input.push(review_message(self.protocol, review_prompt));
                object_mut(&mut phase.body)?.insert("input".into(), Value::Array(input));
                CurrentUserLocation::CollectionItem {
                    collection: "input",
                    index: transcript.len(),
                }
            }
            CurrentUserLocation::CollectionItem { collection, index } => {
                let items = collection_mut(&mut phase.body, collection)?;
                if index >= items.len() {
                    return Err(CodecError::InvalidField {
                        field: collection.into(),
                        expected: "the validated current user item",
                    });
                }
                items.remove(index);
                items.extend(transcript_values);
                items.push(review_message(self.protocol, review_prompt));
                CurrentUserLocation::CollectionItem {
                    collection,
                    index: items.len() - 1,
                }
            }
        };
        phase.tool_result_ids = collect_tool_result_ids(self.protocol, &phase.body);
        phase.tool_result_groups = trailing_tool_result_groups(self.protocol, &phase.body);
        Ok(phase)
    }

    /// Appends native assistant/tool transcript items after this phase's user input.
    pub fn with_appended_transcript(
        &self,
        transcript: &[NativeTranscriptItem],
    ) -> Result<Self, CodecError> {
        for item in transcript {
            if item.protocol != self.protocol {
                return Err(CodecError::TranscriptProtocolMismatch {
                    expected: self.protocol,
                    actual: item.protocol,
                });
            }
        }
        if transcript.is_empty() {
            return Ok(self.clone());
        }

        let mut request = self.clone();
        let values = transcript.iter().map(|item| item.value.clone());
        match request.current_user {
            CurrentUserLocation::ResponsesString => {
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
                request.current_user = CurrentUserLocation::CollectionItem {
                    collection: "input",
                    index: 0,
                };
            }
            CurrentUserLocation::CollectionItem { collection, .. } => {
                collection_mut(&mut request.body, collection)?.extend(values);
            }
        }
        request.tool_result_ids = collect_tool_result_ids(self.protocol, &request.body);
        request.tool_result_groups = trailing_tool_result_groups(self.protocol, &request.body);
        Ok(request)
    }

    /// Extracts a non-streaming model response and associates incoming tool results.
    pub fn extract_response(&self, response: &Value) -> Result<NormalizedModelOutput, CodecError> {
        ProtocolCodec::new(self.protocol).extract_response_for_request(self, response)
    }

    fn current_content(&self) -> Result<&Value, CodecError> {
        match self.current_user {
            CurrentUserLocation::ResponsesString => object(&self.body)?
                .get("input")
                .ok_or_else(|| CodecError::MissingField("input".into())),
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

    fn with_user_suffix(
        &self,
        suffix: &str,
        overrides: &InternalTransportOverrides,
    ) -> Result<Self, CodecError> {
        let mut phase = self.clone();
        phase.apply_overrides(overrides)?;
        let addition = format!("\n\n---\n\n{suffix}");

        match phase.current_user {
            CurrentUserLocation::ResponsesString => {
                let input = object_mut(&mut phase.body)?
                    .get_mut("input")
                    .ok_or_else(|| CodecError::MissingField("input".into()))?;
                append_to_text_content(input, &addition, phase.protocol)?;
            }
            CurrentUserLocation::CollectionItem { collection, index } => {
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

    fn apply_overrides(
        &mut self,
        overrides: &InternalTransportOverrides,
    ) -> Result<(), CodecError> {
        let body = object_mut(&mut self.body)?;
        if let Some(model) = &overrides.model {
            body.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(stream) = overrides.stream {
            body.insert("stream".into(), Value::Bool(stream));
            self.stream = stream;
        }
        Ok(())
    }
}

/// Model and stream changes explicitly required by internal transport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InternalTransportOverrides {
    /// Optional upstream model identifier.
    pub model: Option<String>,
    /// Optional upstream streaming mode.
    pub stream: Option<bool>,
}

/// Semantic category of a protocol-native transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTranscriptKind {
    /// Assistant message or output item.
    Assistant,
    /// Model reasoning or thinking item.
    Reasoning,
    /// Tool call item.
    ToolCall,
    /// Tool result item.
    ToolResult,
    /// A native item with a currently unknown semantic category.
    Other,
}

/// A complete protocol-native item ready to be retained in a phase transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTranscriptItem {
    /// Protocol whose request history accepts this item.
    pub protocol: Protocol,
    /// Semantic category used by orchestration without altering native JSON.
    pub kind: NativeTranscriptKind,
    /// Complete native message, content-block container, or output item.
    pub value: Value,
}

impl NativeTranscriptItem {
    /// Creates a transcript item while retaining the complete native JSON value.
    pub fn new(protocol: Protocol, kind: NativeTranscriptKind, value: Value) -> Self {
        Self {
            protocol,
            kind,
            value,
        }
    }
}

/// Complete native tool-result history item and the IDs it satisfies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolResultGroup {
    /// Tool-call identifiers contained in this native history item.
    pub ids: Vec<String>,
    /// Complete native item suitable for appending to a phase transcript.
    pub item: NativeTranscriptItem,
}

/// Protocol-neutral view of a native tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolCall {
    /// Identifier used to correlate a later tool result.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Native arguments value. String arguments remain strings.
    pub arguments: Value,
    /// Complete native tool-call object or block.
    pub native: Value,
}

/// Non-streaming output normalized only as far as phase orchestration requires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedModelOutput {
    /// Concatenated user-visible response text.
    pub visible_text: String,
    /// Complete protocol-native assistant/output items.
    pub items: Vec<NativeTranscriptItem>,
    /// Tool calls extracted without discarding their native representation.
    pub tool_calls: Vec<NativeToolCall>,
    /// Tool result identifiers supplied by the associated incoming request.
    pub tool_result_ids: Vec<String>,
    /// Native stop reason, or an incomplete reason when applicable.
    pub stop_reason: Option<String>,
    /// Native response status when the protocol provides one.
    pub status: Option<String>,
    /// Complete native usage value.
    pub usage: Option<Value>,
}

/// Errors produced while validating, mutating, or extracting protocol JSON.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The complete request or response body is not an object.
    #[error("body must be a JSON object")]
    BodyNotObject,
    /// A required field is absent.
    #[error("missing required field `{0}`")]
    MissingField(String),
    /// A field has the wrong JSON shape.
    #[error("field `{field}` must be {expected}")]
    InvalidField {
        /// Field path.
        field: String,
        /// Expected shape.
        expected: &'static str,
    },
    /// No conversational message could be located.
    #[error("{protocol} request has no conversational input")]
    MissingConversationalInput {
        /// Protocol being parsed.
        protocol: Protocol,
    },
    /// The final conversational input is not a user message.
    #[error("{protocol} request's last conversational input has role `{role}`, not `user`")]
    LastInputNotUser {
        /// Protocol being parsed.
        protocol: Protocol,
        /// Last conversational role.
        role: String,
    },
    /// A transcript item belongs to another protocol.
    #[error("native transcript protocol mismatch: expected {expected}, got {actual}")]
    TranscriptProtocolMismatch {
        /// Request protocol.
        expected: Protocol,
        /// Transcript item protocol.
        actual: Protocol,
    },
    /// A response was associated with a request from another protocol.
    #[error("request protocol mismatch: expected {expected}, got {actual}")]
    RequestProtocolMismatch {
        /// Codec protocol.
        expected: Protocol,
        /// Request protocol.
        actual: Protocol,
    },
}

/// Parser and extractor for one protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolCodec {
    protocol: Protocol,
}

impl ProtocolCodec {
    /// Creates a codec for `protocol`.
    pub fn new(protocol: Protocol) -> Self {
        Self { protocol }
    }

    /// Returns the protocol handled by this codec.
    pub fn protocol(self) -> Protocol {
        self.protocol
    }

    /// Validates a complete body and wraps it with transport-neutral HTTP metadata.
    pub fn parse_request(
        &self,
        method: impl Into<String>,
        path_and_query: impl Into<String>,
        headers: Vec<HeaderPair>,
        body: Value,
    ) -> Result<HttpRequestEnvelope, CodecError> {
        let body_object = object(&body)?;
        let client_model = required_string(body_object, "model")?.to_owned();
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
        let (current_user, tool_result_groups) = match self.protocol {
            Protocol::OpenAiChat => {
                validate_message_request(&body, self.protocol, "messages", true)?
            }
            Protocol::AnthropicMessages => {
                validate_message_request(&body, self.protocol, "messages", false)?
            }
            Protocol::OpenAiResponses => validate_responses_request(&body)?,
        };
        let real_model = client_model
            .strip_suffix("-pig")
            .unwrap_or(client_model.as_str())
            .to_owned();
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

    /// Extracts a non-streaming JSON response into orchestration-ready native output.
    pub fn extract_response(&self, response: &Value) -> Result<NormalizedModelOutput, CodecError> {
        match self.protocol {
            Protocol::OpenAiChat => extract_chat_response(response),
            Protocol::AnthropicMessages => extract_anthropic_response(response),
            Protocol::OpenAiResponses => extract_responses_response(response),
        }
    }

    /// Extracts a response and carries forward tool result IDs from its incoming request.
    pub fn extract_response_for_request(
        &self,
        request: &HttpRequestEnvelope,
        response: &Value,
    ) -> Result<NormalizedModelOutput, CodecError> {
        if request.protocol != self.protocol {
            return Err(CodecError::RequestProtocolMismatch {
                expected: self.protocol,
                actual: request.protocol,
            });
        }
        let mut output = self.extract_response(response)?;
        output.tool_result_ids.clone_from(&request.tool_result_ids);
        Ok(output)
    }
}

fn object(value: &Value) -> Result<&Map<String, Value>, CodecError> {
    value.as_object().ok_or(CodecError::BodyNotObject)
}

fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>, CodecError> {
    value.as_object_mut().ok_or(CodecError::BodyNotObject)
}

fn required_string<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, CodecError> {
    required_string_at(object, field, field)
}

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

fn validate_message_request(
    body: &Value,
    protocol: Protocol,
    field: &'static str,
    allow_null_content: bool,
) -> Result<(CurrentUserLocation, Vec<NativeToolResultGroup>), CodecError> {
    let messages = collection(body, field)?;
    if messages.is_empty() {
        return Err(CodecError::MissingConversationalInput { protocol });
    }

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
            None if role == "user" => return Err(CodecError::MissingField(content_field)),
            None => {}
            Some(content) => validate_content(content, &content_field, allow_null_content)?,
        }
    }

    let trailing_results = trailing_message_tool_results(messages, protocol);
    let current_index = if trailing_results.is_empty() {
        messages.len() - 1
    } else {
        let search_end = messages.len() - trailing_results.len();
        messages[..search_end]
            .iter()
            .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .ok_or(CodecError::MissingConversationalInput { protocol })?
    };
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

fn validate_responses_request(
    body: &Value,
) -> Result<(CurrentUserLocation, Vec<NativeToolResultGroup>), CodecError> {
    let input = object(body)?
        .get("input")
        .ok_or_else(|| CodecError::MissingField("input".into()))?;
    match input {
        Value::String(_) => Ok((CurrentUserLocation::ResponsesString, Vec::new())),
        Value::Array(items) => {
            if items.is_empty() {
                return Err(CodecError::MissingConversationalInput {
                    protocol: Protocol::OpenAiResponses,
                });
            }
            let trailing_results = trailing_responses_tool_results(items);
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

fn trailing_message_tool_results(
    messages: &[Value],
    protocol: Protocol,
) -> Vec<NativeToolResultGroup> {
    let mut groups = Vec::new();
    for message in messages.iter().rev() {
        let ids = match protocol {
            Protocol::OpenAiChat if message.get("role").and_then(Value::as_str) == Some("tool") => {
                message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(|id| vec![id.to_owned()])
            }
            Protocol::AnthropicMessages
                if message.get("role").and_then(Value::as_str) == Some("user") =>
            {
                let parts = message.get("content").and_then(Value::as_array);
                parts.and_then(|parts| {
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
    groups.reverse();
    groups
}

fn trailing_responses_tool_results(items: &[Value]) -> Vec<NativeToolResultGroup> {
    let mut groups = Vec::new();
    for item in items.iter().rev() {
        if item.get("type").and_then(Value::as_str) != Some("function_call_output") {
            break;
        }
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

fn is_text_part(part: &Value, protocol: Protocol) -> bool {
    let part_type = part.get("type").and_then(Value::as_str);
    match protocol {
        Protocol::OpenAiChat => matches!(part_type, Some("text") | Some("input_text")),
        Protocol::AnthropicMessages => part_type == Some("text"),
        Protocol::OpenAiResponses => matches!(part_type, Some("input_text") | Some("output_text")),
    }
}

fn append_to_text_content(
    content: &mut Value,
    addition: &str,
    protocol: Protocol,
) -> Result<(), CodecError> {
    match content {
        Value::String(text) => text.push_str(addition),
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

fn review_message(protocol: Protocol, prompt: &str) -> Value {
    match protocol {
        Protocol::OpenAiChat | Protocol::AnthropicMessages => {
            json!({"role": "user", "content": prompt})
        }
        Protocol::OpenAiResponses => json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}]
        }),
    }
}

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

fn collect_tool_result_ids(protocol: Protocol, body: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    match protocol {
        Protocol::OpenAiChat => {
            if let Ok(messages) = collection(body, "messages") {
                for message in messages {
                    if message.get("role").and_then(Value::as_str) == Some("tool") {
                        push_string_field(&mut ids, message, "tool_call_id");
                    }
                }
            }
        }
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

fn push_string_field(target: &mut Vec<String>, value: &Value, field: &str) {
    if let Some(id) = value.get(field).and_then(Value::as_str) {
        target.push(id.to_owned());
    }
}

fn extract_chat_response(response: &Value) -> Result<NormalizedModelOutput, CodecError> {
    let choices = response
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| CodecError::InvalidField {
            field: "choices".into(),
            expected: "a non-empty array",
        })?;
    let choice =
        choices
            .first()
            .and_then(Value::as_object)
            .ok_or_else(|| CodecError::InvalidField {
                field: "choices[0]".into(),
                expected: "an object",
            })?;
    let message = choice
        .get("message")
        .filter(|value| value.is_object())
        .ok_or_else(|| CodecError::InvalidField {
            field: "choices[0].message".into(),
            expected: "an object",
        })?;
    let visible_text = content_text(message.get("content"), Protocol::OpenAiChat);
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls") {
        let calls = calls.as_array().ok_or_else(|| CodecError::InvalidField {
            field: "choices[0].message.tool_calls".into(),
            expected: "an array",
        })?;
        for (index, call) in calls.iter().enumerate() {
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
        stop_reason: optional_string(choice.get("finish_reason"), "choices[0].finish_reason")?,
        status: None,
        usage: response.get("usage").cloned(),
    })
}

fn extract_anthropic_response(response: &Value) -> Result<NormalizedModelOutput, CodecError> {
    let content = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| CodecError::InvalidField {
            field: "content".into(),
            expected: "an array",
        })?;
    let visible_text = text_from_parts(content, Protocol::AnthropicMessages);
    let mut tool_calls = Vec::new();
    for (index, block) in content.iter().enumerate() {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let id = value_string(block, "id", &format!("content[{index}].id"))?;
        let name = value_string(block, "name", &format!("content[{index}].name"))?;
        let arguments = block
            .get("input")
            .cloned()
            .ok_or_else(|| CodecError::MissingField(format!("content[{index}].input")))?;
        tool_calls.push(NativeToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            native: block.clone(),
        });
    }

    Ok(NormalizedModelOutput {
        visible_text,
        items: vec![NativeTranscriptItem::new(
            Protocol::AnthropicMessages,
            NativeTranscriptKind::Assistant,
            json!({"role": "assistant", "content": content}),
        )],
        tool_calls,
        tool_result_ids: Vec::new(),
        stop_reason: optional_string(response.get("stop_reason"), "stop_reason")?,
        status: optional_string(response.get("status"), "status")?,
        usage: response.get("usage").cloned(),
    })
}

fn extract_responses_response(response: &Value) -> Result<NormalizedModelOutput, CodecError> {
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

    for (index, item) in output.iter().enumerate() {
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let kind = match item_type {
            "message" => {
                visible_text.push_str(&content_text(
                    item.get("content"),
                    Protocol::OpenAiResponses,
                ));
                NativeTranscriptKind::Assistant
            }
            "reasoning" => NativeTranscriptKind::Reasoning,
            "function_call" => {
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
                    native: item.clone(),
                });
                NativeTranscriptKind::ToolCall
            }
            "function_call_output" => NativeTranscriptKind::ToolResult,
            _ => NativeTranscriptKind::Other,
        };
        items.push(NativeTranscriptItem::new(
            Protocol::OpenAiResponses,
            kind,
            item.clone(),
        ));
    }

    let status = optional_string(response.get("status"), "status")?;
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

fn content_text(content: Option<&Value>, protocol: Protocol) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => text_from_parts(parts, protocol),
        _ => String::new(),
    }
}

fn value_string<'a>(value: &'a Value, field: &str, path: &str) -> Result<&'a str, CodecError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| CodecError::InvalidField {
            field: path.into(),
            expected: "a string",
        })
}

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
