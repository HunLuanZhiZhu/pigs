//! 协议原生的 HTTP 相位运行时。
//! Protocol-native HTTP phased runtime.
//!
//! 本模块把完整的协议原生客户端请求（`HttpRequestEnvelope`）送过
//! Pre → Executor → Post 状态机。与 `phased_runtime` 不同，这里**不解构**
//! 请求体：方法、path、query、headers 和三协议原生 JSON 全部原样保留，
//! 只定点修改 model、当前 user 文本和相位对话记录。工具调用由上游
//! Agent 执行，通过有界内存 continuation 恢复。
//!
//! This module drives a complete protocol-native client request
//! (`HttpRequestEnvelope`) through the Pre → Executor → Post state machine.
//! Unlike `phased_runtime`, it **never deconstructs** the request body:
//! method, path, query, headers, and the three protocols' native JSON are
//! all preserved; only the model, current user text, and phase transcript
//! are mutated in place. Tool calls are executed by the upstream Agent and
//! resumed via a bounded in-memory continuation.

use std::sync::{Arc, Mutex};

use pigs_config::Language;
use serde_json::{Map, Number, Value};
use tracing::{debug, info};

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

/// 流式相位运行期间发出的、已去除控制标记的进度事件。
/// Marker-free progress emitted while a streaming phase is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseProgress {
    /// 一次相位模型调用已开始。
    /// A phase model call has started.
    PhaseStart,
    /// 一段可安全转发的可见文本增量。
    /// A visible text delta is safe to forward.
    TextDelta(String),
    /// 一次相位模型调用已结束。
    /// The phase model call has ended.
    PhaseEnd,
}

/// 按执行顺序接收无标记相位进度的回调类型。
/// Receives marker-free phase progress in execution order.
pub type PhaseProgressSink = Arc<dyn Fn(PhaseProgress) + Send + Sync>;

/// 协议原生 HTTP 相位执行的配置。
/// Configuration for protocol-native HTTP phase execution.
#[derive(Debug, Clone, Default)]
pub struct HttpRuntimeConfig {
    /// 相位 user payload 使用的语言。
    /// Language used for phase user payloads.
    pub language: Language,
    /// 纯编排预算（Post 重试、Pre 重规划上限）。
    /// Pure orchestration budgets.
    pub orchestration: OrchestrationLimits,
    /// 内存 continuation 的边界（容量与 TTL）。
    /// In-memory continuation bounds.
    pub continuation: ContinuationConfig,
}

/// 一次 HTTP 相位轮次的终态：完成或暂停。
/// Terminal or paused state of an HTTP phased turn.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpTurnStatus {
    /// 本轮到达了合法的 `PIGEND` 标记。
    /// The turn reached a valid `PIGEND` marker.
    Complete,
    /// 上游 Agent 必须执行这些工具并返回其原生结果。
    /// The upstream agent must execute these tools and return their native results.
    ToolPause {
        /// 用于诊断的不透明内存 continuation 标识符。
        /// Opaque in-memory continuation identifier for diagnostics.
        continuation_id: String,
        /// 需要通过入口协议返回给上游的原生工具调用。
        /// Native tool calls to return through the entry protocol.
        tool_calls: Vec<NativeToolCall>,
    },
}

/// 截至完成或工具暂停为止累积的完整结果。
/// Complete result accumulated up to completion or a tool pause.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpTurnResult {
    /// 所有相位的可见文本（按执行顺序，已去除控制标记）。
    /// All visible phase text in execution order, with control markers removed.
    pub visible_text: String,
    /// 跨相位子请求聚合的原生 usage 计数。
    /// Aggregated native usage counters across phase subrequests.
    pub usage: Value,
    /// 完成或工具暂停状态。
    /// Completion or tool-pause state.
    pub status: HttpTurnStatus,
    /// 最近一次原生模型输出，用于保留协议原生的工具调用。
    /// Latest native model output, used to preserve protocol-native tool calls.
    pub latest_output: NormalizedModelOutput,
}

/// 返回给 HTTP 代理的类型化失败。
/// Typed failures returned to the HTTP proxy.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// 协议原生请求或响应校验失败。
    /// Protocol-native request or response validation failed.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// 相位传输失败。
    /// The phase transport failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// 相位预算耗尽且未成功完成。
    /// A phase budget was exhausted without successful completion.
    #[error(transparent)]
    Orchestration(#[from] OrchestrationError),
    /// 工具结果请求未匹配到任何活动的暂停轮次。
    /// A tool-result request did not match an active paused turn.
    #[error("{0}")]
    UnknownContinuation(ContinuationError),
    /// 并行工具结果不完整；暂停的轮次仍然可用。
    /// Parallel tool results were incomplete; the paused turn remains available.
    #[error("continuation is waiting for tool result id(s): {missing_ids:?}")]
    MissingToolResults {
        /// 本次请求中缺失的待处理 ID。
        /// Pending IDs absent from this request.
        missing_ids: Vec<String>,
    },
    /// continuation 互斥锁被毒化。
    /// The continuation mutex was poisoned.
    #[error("continuation store is unavailable")]
    ContinuationStoreUnavailable,
}

/// 将完整的原生请求送过 Pre -> Executor -> Post。
/// Runs complete native requests through Pre -> Executor -> Post.
pub struct HttpPhasedRuntime {
    /// 底层相位传输（通常指向 pigs-proxy 的 loopback）。
    /// Underlying phase transport (typically pigs-proxy's loopback).
    transport: Arc<dyn PhaseTransport>,
    /// 运行时配置（语言、预算、continuation 边界）。
    /// Runtime configuration (language, budgets, continuation bounds).
    config: HttpRuntimeConfig,
    /// 有界内存 continuation 存储，用 Mutex 保护以支持并发请求。
    /// Bounded in-memory continuation store, Mutex-guarded for concurrent requests.
    continuations: Mutex<ContinuationStore>,
}

impl HttpPhasedRuntime {
    /// 创建一个运行时，内部含空的内存 continuation 存储。
    /// Creates a runtime with an empty in-memory continuation store.
    pub fn new(transport: Arc<dyn PhaseTransport>, config: HttpRuntimeConfig) -> Self {
        // 用配置中的 continuation 边界初始化存储 / Initialize the store with configured bounds.
        let continuations = Mutex::new(ContinuationStore::new(config.continuation));
        Self {
            transport,
            config,
            continuations,
        }
    }

    /// 运行或恢复一次协议原生客户端请求（无进度回调）。
    /// Runs or resumes one protocol-native client request.
    pub async fn run(&self, request: HttpRequestEnvelope) -> Result<HttpTurnResult, RuntimeError> {
        self.run_internal(request, None).await
    }

    /// 运行或恢复一次请求，并上报每个无标记相位的输出。
    /// Runs or resumes a request and reports each marker-free phase output.
    pub async fn run_with_progress(
        &self,
        request: HttpRequestEnvelope,
        progress: PhaseProgressSink,
    ) -> Result<HttpTurnResult, RuntimeError> {
        self.run_internal(request, Some(progress)).await
    }

    /// 内部：判断是首次请求还是 continuation 恢复，分流到 execute/resume。
    /// Internal: decide first-request vs continuation-resume, dispatch accordingly.
    async fn run_internal(
        &self,
        request: HttpRequestEnvelope,
        progress: Option<PhaseProgressSink>,
    ) -> Result<HttpTurnResult, RuntimeError> {
        // 是否携带工具结果（continuation 恢复）/ Does the request carry tool results (resume)?
        let is_continuation = request.is_continuation();
        info!(
            is_continuation,
            protocol = %request.protocol,
            model = %request.client_model,
            real_model = %request.real_model,
            stream = request.stream,
            "pigs phase runtime: starting turn"
        );
        if is_continuation {
            // 恢复暂停的轮次 / Resume a paused turn.
            self.resume(request, progress).await
        } else {
            // 首次请求：新建 Session 并执行 / First request: create a Session and execute.
            self.execute(
                Session::new(request, OrchestrationState::new(self.config.orchestration)),
                progress,
            )
            .await
        }
    }

    /// 恢复一个暂停的轮次：用工具结果匹配 continuation，重建 Session 后执行。
    /// Resume a paused turn: match the continuation by tool results, rebuild a Session, execute.
    async fn resume(
        &self,
        incoming: HttpRequestEnvelope,
        progress: Option<PhaseProgressSink>,
    ) -> Result<HttpTurnResult, RuntimeError> {
        // 在 continuation 存储中查找匹配的暂停轮次 / Look up the matching paused turn.
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
        // 只有 Ready 才能继续；Waiting 表示并行结果不完整 / Only Ready can proceed; Waiting means partial results.
        let Lookup::Ready(continuation) = lookup else {
            let Lookup::Waiting { missing_ids } = lookup else {
                unreachable!();
            };
            return Err(RuntimeError::MissingToolResults { missing_ids });
        };
        let mut continuation = *continuation;

        // 把已收到的工具结果作为对话记录追加到 phase_transcript / Append received tool results to the phase transcript.
        for group in &continuation.received_results {
            continuation.phase_transcript.push(group.item.clone());
        }
        // 清空已消费的结果，避免重复追加 / Clear consumed results to avoid double-append.
        continuation.received_results.clear();
        // 用本次请求的 headers 覆盖（鉴权可能刷新）/ Refresh headers from the incoming request.
        continuation.original_request.headers = incoming.headers;
        // 从 continuation 重建 Session 并执行 / Rebuild a Session from the continuation and execute.
        self.execute(Session::from_continuation(continuation), progress)
            .await
    }

    /// 核心执行循环：反复发送相位子请求，直到完成或遇到工具调用暂停。
    /// Core execution loop: send phase subrequests until completion or a tool-call pause.
    async fn execute(
        &self,
        mut session: Session,
        progress: Option<PhaseProgressSink>,
    ) -> Result<HttpTurnResult, RuntimeError> {
        // 循环计数器（仅用于日志）/ loop counter (logging only).
        let mut loop_count: u32 = 0;
        loop {
            loop_count += 1;
            // 从编排状态取出当前相位并同步到 Session / Read the current phase from orchestration state.
            let phase = session.orchestration.phase();
            session.phase = phase;
            info!(
                loop_count,
                phase = phase.as_str(),
                pre_replans = session.orchestration.pre_replan_count(),
                post_iterations = session.orchestration.post_iteration_count(),
                "pigs phase: entering phase"
            );
            // 构建本相位的请求（含相位 user payload + 覆盖）/ Build this phase's request.
            let mut phase_request = self.phase_request(&session, progress.is_some())?;
            // 追加本相位已累积的原生对话记录 / Append the accumulated native transcript.
            phase_request = phase_request.with_appended_transcript(&session.phase_transcript)?;
            // 是否流式 = 是否提供了进度回调 / Streaming iff a progress sink was provided.
            let is_streaming = progress.is_some();
            debug!(
                phase = phase.as_str(),
                is_streaming,
                transcript_items = session.phase_transcript.len(),
                "pigs phase: sending request to upstream"
            );
            // 发送请求：流式或非流式 / Send the request: streaming or non-streaming.
            let response = if let Some(progress) = &progress {
                // 流式：通知 PhaseStart，用 MarkerLineBuffer 过滤控制标记
                // Streaming: emit PhaseStart, filter control markers via MarkerLineBuffer.
                progress(PhaseProgress::PhaseStart);
                let buffer = Arc::new(Mutex::new(MarkerLineBuffer::new(Arc::clone(progress))));
                let text_buffer = Arc::clone(&buffer);
                // text_sink 把上游增量喂给 MarkerLineBuffer / text_sink feeds upstream deltas to the buffer.
                let text_sink: TransportTextSink = Arc::new(move |delta| {
                    if let Ok(mut buffer) = text_buffer.lock() {
                        buffer.push(&delta);
                    }
                });
                let response = self
                    .transport
                    .send_streaming(phase_request.clone(), text_sink)
                    .await?;
                // 完成时 flush 缓冲区剩余文本 / Flush any buffered remainder on completion.
                if let Ok(mut buffer) = buffer.lock() {
                    buffer.finish();
                }
                progress(PhaseProgress::PhaseEnd);
                response
            } else {
                // 非流式：直接发送 / Non-streaming: send directly.
                self.transport.send(phase_request.clone()).await?
            };
            // 从响应体解析出归一化输出 / Extract the normalized output from the response body.
            let output = phase_request.extract_response(&response.body)?;
            // 把输出累积到 Session / Accumulate the output into the Session.
            session.push_output(&output);

            // 拼接本相位的原始文本（含标记）用于路由判断 / Join this phase's raw text (with markers) for routing.
            let phase_raw_output = join_visible(&session.phase_raw_parts);
            let tool_call_count = output.tool_calls.len();
            let visible_len = output.visible_text.len();
            let stop_reason = output.stop_reason.as_deref().unwrap_or("none");
            info!(
                phase = phase.as_str(),
                tool_call_count,
                visible_len,
                stop_reason,
                raw_output_len = phase_raw_output.len(),
                raw_output_last_line = phase_raw_output.lines().last().unwrap_or("(empty)"),
                "pigs phase: upstream response received"
            );

            // 有工具调用 → 暂停轮次，存入 continuation / Tool calls present → pause and store a continuation.
            if !output.tool_calls.is_empty() {
                let visible_text = join_visible(&session.visible_parts);
                let usage = aggregate_usage(&session.usage_values);
                let tool_ids: Vec<&str> = output
                    .tool_calls
                    .iter()
                    .map(|call| call.id.as_str())
                    .collect();
                info!(
                    phase = phase.as_str(),
                    tool_ids = ?tool_ids,
                    "pigs phase: pausing for external tool execution"
                );
                // 把 Session 打包成 continuation / Package the Session into a continuation.
                let continuation = session.into_continuation(output.tool_calls.clone());
                // 插入存储并取得 continuation_id / Insert into the store and get the continuation_id.
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

            // Executor/Post 完成后，把相位记录并入审阅记录 / Merge phase transcript into review transcript for Executor/Post.
            if matches!(phase, Phase::Executor | Phase::Post) {
                session
                    .review_transcript
                    .extend(session.phase_transcript.iter().cloned());
            }
            // 清空本相位临时记录 / Clear this phase's temporary records.
            session.phase_transcript.clear();
            session.phase_raw_parts.clear();
            // 在原始文本上检测控制标记 / Detect control markers on the raw text.
            let detected_marker = crate::phased_markers::detect_marker(&phase_raw_output);
            info!(
                phase = phase.as_str(),
                detected_marker = ?detected_marker,
                "pigs phase: checking control marker in raw output"
            );
            // 推进编排状态机 / Advance the orchestration state machine.
            match session.orchestration.advance(&phase_raw_output)? {
                Advance::Complete => {
                    // 整轮完成 / Turn complete.
                    info!(
                        phase = phase.as_str(),
                        loop_count, "pigs phase: turn complete (Advance::Complete)"
                    );
                    return Ok(HttpTurnResult {
                        visible_text: join_visible(&session.visible_parts),
                        usage: aggregate_usage(&session.usage_values),
                        status: HttpTurnStatus::Complete,
                        latest_output: output,
                    });
                }
                Advance::Continue(next_phase) => {
                    // 进入下一相位，继续循环 / Transition to the next phase, keep looping.
                    info!(
                        from = phase.as_str(),
                        to = next_phase.as_str(),
                        "pigs phase: transitioning to next phase"
                    );
                }
            }
        }
    }

    /// 根据当前相位构造相位请求（注入相位 user payload + 传输覆盖）。
    /// Build the phase request for the current phase (inject phase payload + overrides).
    fn phase_request(
        &self,
        session: &Session,
        stream: bool,
    ) -> Result<HttpRequestEnvelope, CodecError> {
        // 内部传输覆盖：真实模型 + 流式标志 / Internal overrides: real model + stream flag.
        let overrides = InternalTransportOverrides {
            model: Some(session.original_request.real_model.clone()),
            stream: Some(stream),
        };
        match session.orchestration.phase() {
            // Pre：用户原文 + Pre 指令（含失败路径）/ Pre: user text + Pre instructions (with failure paths).
            Phase::Pre => session.original_request.for_pre(
                &pigs_prompts::pre_user_payload(
                    self.config.language,
                    session.orchestration.failure_outputs(),
                ),
                &overrides,
            ),
            // Executor：用户原文 + Executor 指令 + Pre 产物 / Executor: user text + Executor instructions + Pre output.
            Phase::Executor => session.original_request.for_executor(
                &pigs_prompts::executor_user_payload(
                    self.config.language,
                    session.orchestration.pre_output(),
                    "",
                ),
                &overrides,
            ),
            // Post：审阅记录 + Post 指令 / Post: review transcript + Post instructions.
            Phase::Post => session.original_request.for_post(
                &session.review_transcript,
                &pigs_prompts::post_user_payload(self.config.language, "", ""),
                &overrides,
            ),
        }
    }
}

/// 一次 HTTP 相位轮次的可变工作状态（内部类型）。
/// Mutable working state for one HTTP phased turn (internal type).
#[derive(Debug, Clone)]
struct Session {
    /// 原始客户端请求信封（会被相位修改）/ original client request envelope (mutated per phase).
    original_request: HttpRequestEnvelope,
    /// 纯编排状态机 / pure orchestration state machine.
    orchestration: OrchestrationState,
    /// 当前相位（缓存自 orchestration）/ current phase (cached from orchestration).
    phase: Phase,
    /// 本相位累积的原生对话记录（assistant/tool_use/tool_result）/ native transcript accumulated this phase.
    phase_transcript: Vec<crate::protocol::NativeTranscriptItem>,
    /// 跨相位保留供 Post 审阅的原生记录 / native items retained across phases for Post review.
    review_transcript: Vec<crate::protocol::NativeTranscriptItem>,
    /// 跨相位累积的可见文本片段（已去标记）/ visible text fragments (markers stripped).
    visible_parts: Vec<String>,
    /// 本相位累积的原始文本片段（含标记，用于路由）/ raw text fragments this phase (with markers, for routing).
    phase_raw_parts: Vec<String>,
    /// 跨相位累积的原生 usage 值 / native usage values accumulated across phases.
    usage_values: Vec<Value>,
}

impl Session {
    /// 从原始请求和编排状态创建一个新 Session（从 Pre 开始）。
    /// Create a new Session from the original request and orchestration state (starts at Pre).
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

    /// 从 continuation 重建 Session（恢复暂停的轮次）。
    /// Rebuild a Session from a continuation (resuming a paused turn).
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

    /// 把一次模型输出累积到 Session（原始文本、可见文本、usage、对话记录）。
    /// Accumulate one model output into the Session.
    fn push_output(&mut self, output: &NormalizedModelOutput) {
        // 原始文本（含标记）用于路由 / raw text (with markers) for routing.
        self.phase_raw_parts.push(output.visible_text.clone());
        // 可见文本（去标记）用于最终输出 / visible text (stripped) for final output.
        let visible = strip_markers(&output.visible_text);
        if !visible.is_empty() {
            self.visible_parts.push(visible);
        }
        // 累积 usage / accumulate usage.
        if let Some(usage) = &output.usage {
            self.usage_values.push(usage.clone());
        }
        // 累积原生对话记录 / accumulate native transcript items.
        self.phase_transcript.extend(output.items.clone());
    }

    /// 把 Session 转成 continuation（清空 headers 后存入存储）。
    /// Convert the Session into a continuation (headers cleared before storage).
    fn into_continuation(mut self, pending_calls: Vec<NativeToolCall>) -> Continuation {
        // 清空 headers：不保留鉴权信息 / Clear headers: do not retain auth.
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

/// 流式缓冲区：按行缓存上游增量，仅在确认不是控制标记后才转发。
/// Streaming buffer: caches upstream deltas line-by-line, forwarding only
/// once a line is confirmed not to be a control marker.
///
/// 控制标记（PIGEND/PIGFAIL）可能跨多个 SSE chunk 到达，因此必须缓存
/// "尚未结束的最后一行"，直到看到换行或流结束才能决定是否转发。
///
/// Control markers (PIGEND/PIGFAIL) may arrive split across SSE chunks, so
/// the "unfinished last line" must be buffered until a newline or stream end
/// confirms whether it is safe to forward.
struct MarkerLineBuffer {
    /// 尚未决定是否转发的待处理文本 / pending text not yet decided to forward.
    pending: String,
    /// 是否已经发出过至少一次增量（用于决定是否补前导换行）/ whether any delta was already emitted.
    emitted: bool,
    /// 进度回调 / progress sink.
    progress: PhaseProgressSink,
}

impl MarkerLineBuffer {
    /// 创建一个空缓冲区 / create an empty buffer.
    fn new(progress: PhaseProgressSink) -> Self {
        Self {
            pending: String::new(),
            emitted: false,
            progress,
        }
    }

    /// 喂入一段增量：尽量转发已确认安全的行，保留最后未结束的行。
    /// Feed one delta: forward confirmed-safe lines, retain the unfinished last line.
    fn push(&mut self, delta: &str) {
        self.pending.push_str(delta);
        // 找到最后一个非空行的起始位置 / Find the start of the last non-empty line.
        let Some(last_start) = last_nonempty_line_start(&self.pending) else {
            return;
        };
        // 只转发到"最后一个非空行"之前的内容，保留该行 / Forward only up to the last non-empty line, keep it.
        let keep_from = self.pending[..last_start].rfind('\n').unwrap_or(last_start);
        if keep_from == 0 {
            // 没有可转发的已完成行 / No completed line to forward yet.
            return;
        }
        // 取出可转发的部分 / Drain the forwardable prefix.
        let prefix: String = self.pending.drain(..keep_from).collect();
        // 去掉其中的控制标记行（保留布局）/ Strip control-marker lines (preserve layout).
        let visible = strip_control_lines_preserving_layout(&prefix);
        if !visible.trim().is_empty() {
            self.emitted = true;
            (self.progress)(PhaseProgress::TextDelta(visible));
        }
    }

    /// 流结束：把剩余待处理文本去标记后一次性发出。
    /// Stream end: strip markers from the remainder and emit it in one shot.
    fn finish(&mut self) {
        let visible = strip_markers(&self.pending);
        if !visible.is_empty() {
            // 若之前已发过且剩余以换行开头，补一个前导换行保持排版
            // If we already emitted and the remainder starts with a newline,
            // prepend one to preserve layout.
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

/// 从文本中删除整行的控制标记，但保留其它行和换行布局。
/// Remove whole control-marker lines from text, preserving other lines and layout.
fn strip_control_lines_preserving_layout(text: &str) -> String {
    text.split('\n')
        .filter(|line| !is_control_marker_line(line.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 返回最后一个非空行的起始字节偏移（用于决定保留多少待处理文本）。
/// Return the byte offset where the last non-empty line starts.
fn last_nonempty_line_start(text: &str) -> Option<usize> {
    let mut offset = 0usize;
    let mut last = None;
    // 逐段（含换行符）扫描 / Scan segment by segment (including newlines).
    for segment in text.split_inclusive('\n') {
        if !segment.trim().is_empty() {
            last = Some(offset);
        }
        offset += segment.len();
    }
    // 处理末尾没有换行的最后一段 / Handle a trailing segment without a newline.
    if offset < text.len() {
        let segment = &text[offset..];
        if !segment.trim().is_empty() {
            last = Some(offset);
        }
    }
    last
}

/// 把多段可见文本用空行连接成一个字符串。
/// Join visible text fragments with blank lines.
fn join_visible(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 聚合多个原生 usage 值（递归求和数值字段）。
/// Aggregate multiple native usage values (recursively summing numeric fields).
fn aggregate_usage(values: &[Value]) -> Value {
    let mut aggregate = Value::Object(Map::new());
    for value in values {
        merge_usage(&mut aggregate, value);
    }
    aggregate
}

/// 把 `source` 的数值字段递归合并到 `target`（相加），对象字段递归合并，其它字段首次写入。
/// Recursively merge `source` into `target`: numbers sum, objects recurse, others insert-once.
fn merge_usage(target: &mut Value, source: &Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };
    for (key, value) in source {
        match value {
            // 数值 → 饱和相加 / Number → saturating add.
            Value::Number(number) => {
                let sum = target
                    .get(key)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .saturating_add(number.as_u64().unwrap_or(0));
                target.insert(key.clone(), Value::Number(Number::from(sum)));
            }
            // 对象 → 递归合并 / Object → recurse.
            Value::Object(_) => {
                let entry = target
                    .entry(key.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                merge_usage(entry, value);
            }
            // 其它 → 仅在不存在时写入 / Other → insert only if absent.
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
        // 跨 chunk 的 "PIG"+"END" 不应泄露给用户 / A split "PIG"+"END" must not leak.
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
        // 中间的 PIGEND 行被删除，换行不重复 / Mid-stream PIGEND removed, no duplicated newlines.
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
        // 多行文本跨 chunk 应保持排版 / Multiline text across chunks keeps layout.
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
        // 句子中出现的 PIGEND 不应被误删 / PIGEND inside prose must not be stripped.
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
