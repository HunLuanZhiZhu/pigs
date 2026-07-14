//! Protocol-native HTTP phased runtime.

use std::sync::{Arc, Mutex};

use pigs_config::Language;
use serde_json::{Map, Number, Value};

use crate::continuation::{
    Continuation, ContinuationConfig, ContinuationError, ContinuationStore, Lookup,
};
use crate::orchestration::{Advance, OrchestrationError, OrchestrationLimits, OrchestrationState};
use crate::phased_markers::{is_control_marker_line, strip_markers};
use crate::phased_phase::Phase;
use crate::protocol::{
    CodecError, HttpRequestEnvelope, InternalTransportOverrides, NativeToolCall,
    NormalizedModelOutput,
};
use crate::transport::{PhaseTransport, TransportError, TransportTextSink};

/// Marker-free progress emitted while a streaming phase is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseProgress {
    /// A phase model call has started.
    PhaseStart,
    /// A visible text delta is safe to forward.
    TextDelta(String),
    /// The phase model call has ended.
    PhaseEnd,
}

/// Receives marker-free phase progress in execution order.
pub type PhaseProgressSink = Arc<dyn Fn(PhaseProgress) + Send + Sync>;

/// Configuration for protocol-native HTTP phase execution.
#[derive(Debug, Clone, Default)]
pub struct HttpRuntimeConfig {
    /// Language used for phase user payloads.
    pub language: Language,
    /// Pure orchestration budgets.
    pub orchestration: OrchestrationLimits,
    /// In-memory continuation bounds.
    pub continuation: ContinuationConfig,
}

/// Terminal or paused state of an HTTP phased turn.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpTurnStatus {
    /// The turn reached a valid `PIGEND` marker.
    Complete,
    /// The upstream agent must execute these tools and return their native results.
    ToolPause {
        /// Opaque in-memory continuation identifier for diagnostics.
        continuation_id: String,
        /// Native tool calls to return through the entry protocol.
        tool_calls: Vec<NativeToolCall>,
    },
}

/// Complete result accumulated up to completion or a tool pause.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpTurnResult {
    /// All visible phase text in execution order, with control markers removed.
    pub visible_text: String,
    /// Aggregated native usage counters across phase subrequests.
    pub usage: Value,
    /// Completion or tool-pause state.
    pub status: HttpTurnStatus,
    /// Latest native model output, used to preserve protocol-native tool calls.
    pub latest_output: NormalizedModelOutput,
}

/// Typed failures returned to the HTTP proxy.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Protocol-native request or response validation failed.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// The phase transport failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A phase budget was exhausted without successful completion.
    #[error(transparent)]
    Orchestration(#[from] OrchestrationError),
    /// A tool-result request did not match an active paused turn.
    #[error("{0}")]
    UnknownContinuation(ContinuationError),
    /// Parallel tool results were incomplete; the paused turn remains available.
    #[error("continuation is waiting for tool result id(s): {missing_ids:?}")]
    MissingToolResults {
        /// Pending IDs absent from this request.
        missing_ids: Vec<String>,
    },
    /// The continuation mutex was poisoned.
    #[error("continuation store is unavailable")]
    ContinuationStoreUnavailable,
}

/// Runs complete native requests through Pre -> Executor -> Post.
pub struct HttpPhasedRuntime {
    transport: Arc<dyn PhaseTransport>,
    config: HttpRuntimeConfig,
    continuations: Mutex<ContinuationStore>,
}

impl HttpPhasedRuntime {
    /// Creates a runtime with an empty in-memory continuation store.
    pub fn new(transport: Arc<dyn PhaseTransport>, config: HttpRuntimeConfig) -> Self {
        let continuations = Mutex::new(ContinuationStore::new(config.continuation));
        Self {
            transport,
            config,
            continuations,
        }
    }

    /// Runs or resumes one protocol-native client request.
    pub async fn run(&self, request: HttpRequestEnvelope) -> Result<HttpTurnResult, RuntimeError> {
        self.run_internal(request, None).await
    }

    /// Runs or resumes a request and reports each marker-free phase output.
    pub async fn run_with_progress(
        &self,
        request: HttpRequestEnvelope,
        progress: PhaseProgressSink,
    ) -> Result<HttpTurnResult, RuntimeError> {
        self.run_internal(request, Some(progress)).await
    }

    async fn run_internal(
        &self,
        request: HttpRequestEnvelope,
        progress: Option<PhaseProgressSink>,
    ) -> Result<HttpTurnResult, RuntimeError> {
        if request.is_continuation() {
            self.resume(request, progress).await
        } else {
            self.execute(
                Session::new(request, OrchestrationState::new(self.config.orchestration)),
                progress,
            )
            .await
        }
    }

    async fn resume(
        &self,
        incoming: HttpRequestEnvelope,
        progress: Option<PhaseProgressSink>,
    ) -> Result<HttpTurnResult, RuntimeError> {
        let lookup = self
            .continuations
            .lock()
            .map_err(|_| RuntimeError::ContinuationStoreUnavailable)?
            .take_ready(
                incoming.protocol,
                &incoming.real_model,
                incoming.tool_result_groups(),
            )
            .map_err(RuntimeError::UnknownContinuation)?;
        let Lookup::Ready(continuation) = lookup else {
            let Lookup::Waiting { missing_ids } = lookup else {
                unreachable!();
            };
            return Err(RuntimeError::MissingToolResults { missing_ids });
        };
        let mut continuation = *continuation;

        for group in &continuation.received_results {
            continuation.phase_transcript.push(group.item.clone());
        }
        continuation.received_results.clear();
        continuation.original_request.headers = incoming.headers;
        self.execute(Session::from_continuation(continuation), progress)
            .await
    }

    async fn execute(
        &self,
        mut session: Session,
        progress: Option<PhaseProgressSink>,
    ) -> Result<HttpTurnResult, RuntimeError> {
        loop {
            let phase = session.orchestration.phase();
            session.phase = phase;
            let mut phase_request = self.phase_request(&session, progress.is_some())?;
            phase_request = phase_request.with_appended_transcript(&session.phase_transcript)?;
            let response = if let Some(progress) = &progress {
                progress(PhaseProgress::PhaseStart);
                let buffer = Arc::new(Mutex::new(MarkerLineBuffer::new(Arc::clone(progress))));
                let text_buffer = Arc::clone(&buffer);
                let text_sink: TransportTextSink = Arc::new(move |delta| {
                    if let Ok(mut buffer) = text_buffer.lock() {
                        buffer.push(&delta);
                    }
                });
                let response = self
                    .transport
                    .send_streaming(phase_request.clone(), text_sink)
                    .await?;
                if let Ok(mut buffer) = buffer.lock() {
                    buffer.finish();
                }
                progress(PhaseProgress::PhaseEnd);
                response
            } else {
                self.transport.send(phase_request.clone()).await?
            };
            let output = phase_request.extract_response(&response.body)?;
            session.push_output(&output);

            if !output.tool_calls.is_empty() {
                let visible_text = join_visible(&session.visible_parts);
                let usage = aggregate_usage(&session.usage_values);
                let continuation = session.into_continuation(output.tool_calls.clone());
                let continuation_id = self
                    .continuations
                    .lock()
                    .map_err(|_| RuntimeError::ContinuationStoreUnavailable)?
                    .insert(continuation);
                return Ok(HttpTurnResult {
                    visible_text,
                    usage,
                    status: HttpTurnStatus::ToolPause {
                        continuation_id,
                        tool_calls: output.tool_calls.clone(),
                    },
                    latest_output: output,
                });
            }

            if matches!(phase, Phase::Executor | Phase::Post) {
                session
                    .review_transcript
                    .extend(session.phase_transcript.iter().cloned());
            }
            session.phase_transcript.clear();
            let phase_raw_output = join_visible(&session.phase_raw_parts);
            session.phase_raw_parts.clear();
            match session.orchestration.advance(&phase_raw_output)? {
                Advance::Complete => {
                    return Ok(HttpTurnResult {
                        visible_text: join_visible(&session.visible_parts),
                        usage: aggregate_usage(&session.usage_values),
                        status: HttpTurnStatus::Complete,
                        latest_output: output,
                    });
                }
                Advance::Continue(_) => {}
            }
        }
    }

    fn phase_request(
        &self,
        session: &Session,
        stream: bool,
    ) -> Result<HttpRequestEnvelope, CodecError> {
        let overrides = InternalTransportOverrides {
            model: Some(session.original_request.real_model.clone()),
            stream: Some(stream),
        };
        match session.orchestration.phase() {
            Phase::Pre => session.original_request.for_pre(
                &pigs_prompts::pre_user_payload(
                    self.config.language,
                    session.orchestration.failure_outputs(),
                ),
                &overrides,
            ),
            Phase::Executor => session.original_request.for_executor(
                &pigs_prompts::executor_user_payload(
                    self.config.language,
                    session.orchestration.pre_output(),
                    "",
                ),
                &overrides,
            ),
            Phase::Post => session.original_request.for_post(
                &session.review_transcript,
                &pigs_prompts::post_user_payload(self.config.language, "", ""),
                &overrides,
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct Session {
    original_request: HttpRequestEnvelope,
    orchestration: OrchestrationState,
    phase: Phase,
    phase_transcript: Vec<crate::protocol::NativeTranscriptItem>,
    review_transcript: Vec<crate::protocol::NativeTranscriptItem>,
    visible_parts: Vec<String>,
    phase_raw_parts: Vec<String>,
    usage_values: Vec<Value>,
}

impl Session {
    fn new(original_request: HttpRequestEnvelope, orchestration: OrchestrationState) -> Self {
        Self {
            original_request,
            orchestration,
            phase: Phase::Pre,
            phase_transcript: Vec::new(),
            review_transcript: Vec::new(),
            visible_parts: Vec::new(),
            phase_raw_parts: Vec::new(),
            usage_values: Vec::new(),
        }
    }

    fn from_continuation(continuation: Continuation) -> Self {
        Self {
            original_request: continuation.original_request,
            orchestration: continuation.orchestration,
            phase: continuation.phase,
            phase_transcript: continuation.phase_transcript,
            review_transcript: continuation.review_transcript,
            visible_parts: continuation.visible_parts,
            phase_raw_parts: continuation.phase_raw_parts,
            usage_values: continuation.usage_values,
        }
    }

    fn push_output(&mut self, output: &NormalizedModelOutput) {
        self.phase_raw_parts.push(output.visible_text.clone());
        let visible = strip_markers(&output.visible_text);
        if !visible.is_empty() {
            self.visible_parts.push(visible);
        }
        if let Some(usage) = &output.usage {
            self.usage_values.push(usage.clone());
        }
        self.phase_transcript.extend(output.items.clone());
    }

    fn into_continuation(mut self, pending_calls: Vec<NativeToolCall>) -> Continuation {
        self.original_request.headers.clear();
        Continuation {
            id: String::new(),
            original_request: self.original_request,
            orchestration: self.orchestration,
            phase: self.phase,
            phase_transcript: self.phase_transcript,
            review_transcript: self.review_transcript,
            visible_parts: self.visible_parts,
            phase_raw_parts: self.phase_raw_parts,
            pending_calls,
            usage_values: self.usage_values,
            received_results: Vec::new(),
        }
    }
}

struct MarkerLineBuffer {
    pending: String,
    emitted: bool,
    progress: PhaseProgressSink,
}

impl MarkerLineBuffer {
    fn new(progress: PhaseProgressSink) -> Self {
        Self {
            pending: String::new(),
            emitted: false,
            progress,
        }
    }

    fn push(&mut self, delta: &str) {
        self.pending.push_str(delta);
        let Some(last_start) = last_nonempty_line_start(&self.pending) else {
            return;
        };
        let keep_from = self.pending[..last_start].rfind('\n').unwrap_or(last_start);
        if keep_from == 0 {
            return;
        }
        let prefix: String = self.pending.drain(..keep_from).collect();
        let visible = strip_control_lines_preserving_layout(&prefix);
        if !visible.trim().is_empty() {
            self.emitted = true;
            (self.progress)(PhaseProgress::TextDelta(visible));
        }
    }

    fn finish(&mut self) {
        let visible = strip_markers(&self.pending);
        if !visible.is_empty() {
            let prefix = if self.emitted && self.pending.starts_with('\n') {
                "\n"
            } else {
                ""
            };
            (self.progress)(PhaseProgress::TextDelta(format!("{prefix}{visible}")));
        }
        self.pending.clear();
    }
}

fn strip_control_lines_preserving_layout(text: &str) -> String {
    text.split('\n')
        .filter(|line| !is_control_marker_line(line.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .join("\n")
}

fn last_nonempty_line_start(text: &str) -> Option<usize> {
    let mut offset = 0usize;
    let mut last = None;
    for segment in text.split_inclusive('\n') {
        if !segment.trim().is_empty() {
            last = Some(offset);
        }
        offset += segment.len();
    }
    if offset < text.len() {
        let segment = &text[offset..];
        if !segment.trim().is_empty() {
            last = Some(offset);
        }
    }
    last
}

fn join_visible(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn aggregate_usage(values: &[Value]) -> Value {
    let mut aggregate = Value::Object(Map::new());
    for value in values {
        merge_usage(&mut aggregate, value);
    }
    aggregate
}

fn merge_usage(target: &mut Value, source: &Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };
    for (key, value) in source {
        match value {
            Value::Number(number) => {
                let sum = target
                    .get(key)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .saturating_add(number.as_u64().unwrap_or(0));
                target.insert(key.clone(), Value::Number(Number::from(sum)));
            }
            Value::Object(_) => {
                let entry = target
                    .entry(key.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                merge_usage(entry, value);
            }
            _ => {
                target.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
}

#[cfg(test)]
mod marker_buffer_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn hides_split_control_lines_even_when_the_model_continues() {
        let output = Arc::new(Mutex::new(String::new()));
        let sink_output = Arc::clone(&output);
        let sink: PhaseProgressSink = Arc::new(move |event| {
            if let PhaseProgress::TextDelta(text) = event {
                sink_output.lock().unwrap().push_str(&text);
            }
        });
        let mut buffer = MarkerLineBuffer::new(sink);
        buffer.push("PIG");
        buffer.push("END\nstill working");
        buffer.finish();

        assert_eq!(*output.lock().unwrap(), "still working");
    }

    #[test]
    fn removes_midstream_control_line_without_duplicating_newlines() {
        let output = Arc::new(Mutex::new(String::new()));
        let sink_output = Arc::clone(&output);
        let sink: PhaseProgressSink = Arc::new(move |event| {
            if let PhaseProgress::TextDelta(text) = event {
                sink_output.lock().unwrap().push_str(&text);
            }
        });
        let mut buffer = MarkerLineBuffer::new(sink);
        buffer.push("reason\nPIGEND\nsti");
        buffer.push("ll working");
        buffer.finish();

        assert_eq!(*output.lock().unwrap(), "reason\nstill working");
    }

    #[test]
    fn preserves_multiline_layout_across_stream_chunks() {
        let output = Arc::new(Mutex::new(String::new()));
        let sink_output = Arc::clone(&output);
        let sink: PhaseProgressSink = Arc::new(move |event| {
            if let PhaseProgress::TextDelta(text) = event {
                sink_output.lock().unwrap().push_str(&text);
            }
        });
        let mut buffer = MarkerLineBuffer::new(sink);
        buffer.push("first\nsecond\nthi");
        buffer.push("rd\nPIGEND");
        buffer.finish();

        assert_eq!(*output.lock().unwrap(), "first\nsecond\nthird");
    }

    #[test]
    fn keeps_marker_words_inside_ordinary_sentences() {
        let output = Arc::new(Mutex::new(String::new()));
        let sink_output = Arc::clone(&output);
        let sink: PhaseProgressSink = Arc::new(move |event| {
            if let PhaseProgress::TextDelta(text) = event {
                sink_output.lock().unwrap().push_str(&text);
            }
        });
        let mut buffer = MarkerLineBuffer::new(sink);
        buffer.push("PIGEND appears in prose\nnext line");
        buffer.finish();

        assert_eq!(
            *output.lock().unwrap(),
            "PIGEND appears in prose\nnext line"
        );
    }
}
