//! pigs-orchestrator —— 编排引擎。
//!
//! 三只 pig 组成一头 pigs：Pre（规划/分流）→ Executor（执行）→ Post（验收），
//! 由 PIGEND / PIGNEXT / PIGFAIL 控制标记与预算驱动的状态机串起来。
//!
//! **一只 pig 是一段对话区间**：模型要工具 → 客户端执行 → 结果回填 → 模型继续，
//! 直到模型这一轮不再要工具，相位才算产出（跨等待的现场见 [`state::Continuation`]）。
//! 模型要工具时，这一轮就**暂停**，把调用原样交给客户端；客户端带结果回来时接着同一只 pig 继续。
//!
//! 红线（见仓库根 `AGENTS.md`）：相对上游只允许改**模型名**与**提示词尾部追加**；
//! 不认识 axum（网络走 [`Transport`] 注入）、不做重试、不知道上游是谁。

pub mod lang;
pub mod markers;
pub mod prompts;
pub mod state;
pub mod transport;

use markers::{detect_marker, detect_terminal_marker, strip_markers, Marker, MarkerFilter};
use pigs_protocol as proto;
use proto::{ModelOutput, Part, ToolCall};
use state::{Continuation, ContinuationStore, TurnState};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use transport::{LiveSink, SubRequest, SubResponse, Transport, TransportError};

/// 会话头名：编排产生的稳定会话标识，所有子请求共用（mini-proxy 见已带就不覆盖）。
pub const SESSION_HEADER: &str = "x-opencode-session";

/// 回环内部令牌头（proxy 验证后跳过 -pigs 分流，防递归）。
pub const LOOPBACK_TOKEN_HEADER: &str = "x-pigs-loopback";

/// 编排次数常量（刻意不进配置——它们是编排语义的一部分）。
/// - `MAX_EXECUTOR_RUNS`：一轮任务最多允许多少次高层 Executor 执行；工具暂停/恢复不重复计数。
///   达到上限的那次 Executor 完成后直接结束，不再进入 Post。
/// - `MAX_POST_PROTOCOL_RETRIES`：Post 没有给控制标记时的协议重试上限；它不属于任务执行次数。
const MAX_EXECUTOR_RUNS: u32 = 4;
const MAX_POST_PROTOCOL_RETRIES: u32 = 3;

/// 单只 pig（一个相位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pig {
    /// 规划 / 分流 / GOAL 声明。
    Pre,
    /// 信息收集 + 起草答复（工具调用多发生在这个相位）。
    Executor,
    /// 纯核验 + 路由：接受、回 Executor 继续修补、或回 Pre 重规划。
    Post,
}

impl Pig {
    pub fn as_str(self) -> &'static str {
        match self {
            Pig::Pre => "pre",
            Pig::Executor => "executor",
            Pig::Post => "post",
        }
    }
}

/// 一只 pig 的生命周期事件（proxy 用它边编排边推 SSE 帧）。
#[derive(Debug, Clone)]
pub enum PigEvent {
    /// 该 pig 的子请求已发出。
    Start(Pig),
    /// 一段可安全转发的可见文本（控制标记已过滤）。
    Delta(String),
    /// 一段思考文本（原样转发，不过滤）。
    Thought(String),
    /// Responses 的思考摘要增量（带 item_id）。
    ThoughtSummary { item_id: String, text: String },
    /// 思考块的签名（Anthropic：思考块末尾的签名，必须跟着一起给客户端）。
    ThoughtSignature(String),
    /// 该 pig 的文本流结束。
    End(Pig),
}

/// 进度回调：整个编排过程中按顺序收到 [`PigEvent`]。
pub type ProgressSink = Arc<dyn Fn(PigEvent) + Send + Sync>;

/// 编排输入：proxy 已解析好的现场（每次客户端请求一份）。
#[derive(Debug, Clone)]
pub struct TurnInput {
    /// 协议（由请求路径判定）。
    pub protocol: proto::Protocol,
    /// 输出模式：A 逐阶段暴露正文；B 只提交最终被接受的业务正文。
    pub mode: proto::PigsMode,
    /// 原始请求 body（JSON）。客户端请求什么就是什么，只允许在尾部追加。
    pub body: serde_json::Value,
    /// 协议路径（子请求原样使用）。
    pub path: String,
    /// 原查询串（有就原样带上）。
    pub query: Option<String>,
    /// 需要随行的客户端端到端头（鉴权、会话头等，原样透传）。
    pub base_headers: Vec<(String, String)>,
    /// 客户端自带的会话头值（没有则为 None，编排生成）。
    pub client_session: Option<String>,
}

/// 跑一轮编排需要的外部依赖。
pub struct Runtime {
    /// 子请求传输（生产实现 = proxy 的 loopback）。
    pub transport: Arc<dyn Transport>,
    /// 工具调用暂停的现场存储。
    pub store: Arc<Mutex<ContinuationStore>>,
    /// 有回调 = 客户端要流式：子请求按流式读、增量边到边转发。
    pub progress: Option<ProgressSink>,
}

/// 一轮编排的结果：跑完，或者被工具调用打断。
#[derive(Debug, Clone)]
pub enum Outcome {
    /// 本轮完成（正常验收，或最后一次 Executor 直接完成）。
    Completed(TurnResult),
    /// 模型要工具：调用已交给客户端，等它执行完带结果回来。
    Paused(PausedTurn),
}

/// 完成后的结果。
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// 最终答复。模式 A = 各只 pig 可见文本拼接；模式 B = 最终 committed business text。
    pub text: String,
    /// 客户端业务正文片段：模式 A 按执行顺序；模式 B 只有最终提交的一段。
    pub visible: Vec<String>,
    /// 最终内容序列（文本 + 思考 + 其它原生块，按顺序）；已消费的工具调用不会再次出现。
    pub parts: Vec<Part>,
    /// 结束方式（诊断/日志用）。
    pub ended_with: EndedWith,
    /// 实际使用的会话头值。
    pub session: String,
    /// 完整走过的 pig 序列（含工具往返后的重复相位，诊断用）。
    pub path: Vec<Pig>,
    /// 当前策略选中的上游 usage 原对象（上游没给就是 None）。
    pub usage: Option<serde_json::Value>,
    /// 最后一个相位给的停止原因（原样回传）。
    pub stop_reason: Option<String>,
}

/// 被工具调用打断时的产出。
#[derive(Debug, Clone)]
pub struct PausedTurn {
    /// 交给客户端执行的工具调用（协议原生，原样）。
    pub tool_calls: Vec<ToolCall>,
    /// 到目前为止的客户端业务正文。模式 B 在工具暂停时通常为空，因为候选尚未验收。
    pub text: String,
    /// 暂停响应的内容序列：持久内容 + 本轮一次性的工具调用，顺序权威。
    pub parts: Vec<Part>,
    /// 这一轮的停止原因（上游原话，通常是 tool_calls / tool_use）。
    pub stop_reason: Option<String>,
    /// 恢复句柄（proxy 存自己手里，客户端不感知）。
    pub continuation_id: String,
}

/// 结束方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedWith {
    /// Pre 判定为简单问题，直接作答。
    SimplePath,
    /// Post 验收通过。
    PigEnd,
    /// 已达到 Executor 执行次数上限；最后一次 Executor 完成后直接结束，未再进入 Post。
    ExecutorLimit,
}

impl EndedWith {
    pub fn as_str(self) -> &'static str {
        match self {
            EndedWith::SimplePath => "SIMPLE_PATH",
            EndedWith::PigEnd => "PIGEND",
            EndedWith::ExecutorLimit => "EXECUTOR_LIMIT",
        }
    }
}

/// 编排错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("协议层错误: {0}")]
    Protocol(#[from] proto::Error),
    #[error("传输错误: {0}")]
    Transport(#[from] TransportError),
    #[error("上游输出因 token 限制被截断（协议 {0:?}，pig {1}，reason {2}）")]
    OutputTruncated(proto::Protocol, &'static str, String),
    #[error("上游响应既没有文本也没有工具调用（协议 {0:?}，pig {1}）")]
    NoText(proto::Protocol, &'static str),
    #[error("编排预算耗尽: {0}")]
    Budget(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 编排器。无状态，可全局共享。
#[derive(Debug, Clone, Default)]
pub struct Orchestrator;

impl Orchestrator {
    pub fn new() -> Self {
        Self
    }

    /// 开一轮新编排（客户端的第一发请求）。
    pub async fn run(&self, input: TurnInput, rt: Runtime) -> Result<Outcome> {
        let session = input
            .client_session
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let lang = lang::detect_lang(&proto::extract_last_user_text(&input.body, input.protocol));
        let state = TurnState::new_with_mode(lang, session, input.body.clone(), input.mode);
        Self::drive(input, rt, state).await
    }

    /// 接着被工具调用打断的那只 pig 继续（客户端带工具结果回来了）。
    pub async fn resume(
        &self,
        input: TurnInput,
        rt: Runtime,
        mut continuation: Continuation,
    ) -> Result<Outcome> {
        tracing::info!(
            continuation = %continuation.id,
            phase = continuation.state.phase.as_str(),
            "恢复相位（工具结果已回填）"
        );
        if input.mode != continuation.state.mode {
            return Err(Error::Budget("continuation 的 PIGS 模式与恢复请求不一致".into()));
        }
        // continuation 只负责判断“这次请求是不是上一轮工具调用的继续”。
        // 一旦确认恢复，就保留客户端从匹配工具结果开始追加的后续消息原样，不擅自过滤 reminder/user。
        let resume_items =
            proto::continuation_resume_items(input.protocol, &input.body, &continuation.pending);
        continuation.state.phase_transcript.extend(resume_items);
        Self::drive(input, rt, continuation.state).await
    }

    /// 为新进入的 pig 构造一次基础请求。阶段提示在这里注入，之后整个 pig 都复用它。
    fn build_phase_base_body(
        input: &TurnInput,
        state: &TurnState,
        phase: Pig,
    ) -> Result<serde_json::Value> {
        let mut body = state.root_body.clone();
        match phase {
            Pig::Pre => {
                let instruction = prompts::pre_instruction(state.lang, &state.failure_paths);
                proto::append_to_last_user_text(&mut body, input.protocol, &instruction)?;
            }
            Pig::Executor => {
                let instruction = prompts::executor_instruction(state.lang, &state.pre_output);
                proto::append_to_last_user_text(&mut body, input.protocol, &instruction)?;
            }
            Pig::Post => {
                return Err(Error::Budget(
                    "Post 必须继承 Executor 的完整上下文，不能从 root_body 重建".into(),
                ));
            }
        }
        Ok(body)
    }

    /// 物化“Executor 已执行完、尚未进入 Post”的完整上下文截面。
    fn build_executor_checkpoint(
        input: &TurnInput,
        state: &TurnState,
    ) -> Result<serde_json::Value> {
        let mut body = state
            .phase_base_body
            .clone()
            .ok_or_else(|| Error::Budget("Executor 缺少 phase_base_body，无法建立 checkpoint".into()))?;
        proto::append_transcript_items(&mut body, input.protocol, &state.phase_transcript)?;
        Ok(body)
    }

    /// 从 Executor checkpoint 构造 Post：只在 checkpoint 后追加一条 Post 核验 user 消息。
    fn build_post_base_from_checkpoint(
        input: &TurnInput,
        state: &TurnState,
        checkpoint: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut body = checkpoint.clone();
        proto::push_user_message(
            &mut body,
            input.protocol,
            &prompts::post_instruction(state.lang),
        )?;
        Ok(body)
    }

    /// Post 返回 PIGNEXT 时，砍回最近一次 Executor 完成的 checkpoint。
    /// 然后复用原 Executor 指令模板，只把“执行前分析”替换为 Post 的核验反馈。
    fn build_executor_base_from_checkpoint(
        input: &TurnInput,
        state: &TurnState,
        feedback: &str,
    ) -> Result<serde_json::Value> {
        let mut body = state
            .executor_checkpoint_body
            .clone()
            .ok_or_else(|| Error::Budget("Post 缺少 Executor checkpoint，无法 PIGNEXT".into()))?;
        proto::push_user_message(
            &mut body,
            input.protocol,
            &prompts::executor_instruction(state.lang, feedback),
        )?;
        Ok(body)
    }

    /// 状态机主循环：每轮发一次子请求，按"有没有工具调用"决定暂停还是推进相位。
    async fn drive(input: TurnInput, rt: Runtime, mut state: TurnState) -> Result<Outcome> {
        let ctx = Ctx {
            input,
            transport: Arc::clone(&rt.transport),
            progress: rt.progress.clone(),
            store: Arc::clone(&rt.store),
        };
        loop {
            let phase = state.phase;
            // 相位序列只记"进入过哪些 pig"：同一个相位因工具往返而重复的多轮不算新的
            let first_entry = state.path.last() != Some(&phase);
            if first_entry {
                state.path.push(phase);
                if phase == Pig::Executor {
                    state.executor_runs += 1;
                }
            }
            tracing::info!(
                pig = phase.as_str(),
                first_entry,
                executor_runs = state.executor_runs,
                "pig round"
            );

            // 每个 pig 的阶段提示只在第一次进入时注入一次。之后的工具往返只追加
            // phase_transcript，绝不在工具结果尾部重新放一条阶段 user 提示。
            if state.phase_base_body.is_none() {
                state.phase_base_body =
                    Some(Self::build_phase_base_body(&ctx.input, &state, phase)?);
            }
            let base_body = state
                .phase_base_body
                .as_ref()
                .expect("phase base body must exist");
            let output = ctx
                .call_pig(phase, base_body, &state.phase_transcript, &state.session)
                .await?;
            let round_transcript =
                proto::model_output_transcript_items(ctx.input.protocol, &output);
            // ToolCall 只属于这一发 Paused 响应。先按原顺序组好客户端内容，
            // 再把本轮的持久部分写进 state；continuation / Completed 都不能携带已消费的 ToolCall。
            let paused_parts = if output.tool_calls.is_empty() {
                None
            } else {
                let mut parts = state.parts.clone();
                parts.extend(state.client_parts_for_round(&output));
                Some(parts)
            };
            state.record_round(&output.text, &output);
            state.phase_transcript.extend(round_transcript);

            // 模型要工具 → 暂停，把调用原样交给客户端（相位不结束）
            if !output.tool_calls.is_empty() {
                let pending: Vec<String> = output
                    .tool_calls
                    .iter()
                    .map(|call| call.id.clone())
                    .collect();
                tracing::info!(
                    pig = phase.as_str(),
                    calls = pending.len(),
                    "模型请求工具调用，暂停相位等待客户端执行"
                );
                let text = state.final_text();
                let parts = paused_parts.unwrap_or_else(|| state.parts.clone());
                let continuation_id = ctx
                    .store
                    .lock()
                    .map_err(|_| Error::Budget("continuation 存储不可用".into()))?
                    .insert(Continuation {
                        id: String::new(),
                        pending,
                        state: state.clone(),
                        created: Instant::now(),
                    });
                return Ok(Outcome::Paused(PausedTurn {
                    tool_calls: output.tool_calls,
                    text,
                    parts,
                    stop_reason: output.stop_reason,
                    continuation_id,
                }));
            }

            // 没有工具调用 → 这一轮就是该相位的产出，按控制标记路由
            let raw = state.phase_raw_text();
            match phase {
                // ---------------- Pre：规划 / 分流 ----------------
                Pig::Pre => match detect_marker(&raw) {
                    // 简单路径：Pre 直接给出答案，整轮结束
                    Some(Marker::End) => {
                        state.commit_text(strip_markers(&raw));
                        return Ok(Outcome::Completed(state.complete(EndedWith::SimplePath)))
                    }
                    // Pre 只负责给下一次 Executor 形成计划；PIGFAIL/PIGNEXT 在 Pre 没有独立的次数语义。
                    // 除简单路径 PIGEND 外，其余输出都按复杂计划进入 Executor。
                    Some(Marker::Failed) | Some(Marker::Next) | None => {
                        state.pre_output = strip_markers(&raw);
                        state.post_protocol_retries = 0;
                        state.executor_checkpoint_body = None;
                        state.discard_candidate();
                        state.phase_raw.clear();
                        state.phase = Pig::Executor;
                        state.reset_phase_conversation();
                    }
                },
                // ---------------- Executor：执行 ----------------
                Pig::Executor => {
                    // 不解析标记。模式 B 先把本次 Executor 的普通文本保存为候选。
                    state.set_candidate(strip_markers(&raw));

                    // 所有执行预算统一按 Executor 高层执行次数计算。最后一次允许的 Executor
                    // 完成后直接结束；此时再做 Post 已没有任何后续执行机会，因此没有意义。
                    if state.executor_runs >= MAX_EXECUTOR_RUNS {
                        state.commit_candidate();
                        return Ok(Outcome::Completed(
                            state.complete(EndedWith::ExecutorLimit),
                        ));
                    }

                    // 尚有下一次执行机会时才保存 checkpoint 并进入 Post 核验。
                    let checkpoint = Self::build_executor_checkpoint(&ctx.input, &state)?;
                    let post_base =
                        Self::build_post_base_from_checkpoint(&ctx.input, &state, &checkpoint)?;
                    state.executor_checkpoint_body = Some(checkpoint);
                    state.post_protocol_retries = 0;
                    state.phase_raw.clear();
                    state.phase = Pig::Post;
                    state.phase_base_body = Some(post_base);
                    state.phase_transcript.clear();
                }
                // ---------------- Post：验收 / 路由 ----------------
                Pig::Post => match detect_terminal_marker(&raw) {
                    // 验收通过 → 整轮结束
                    Some(Marker::End) => {
                        state.commit_candidate();
                        return Ok(Outcome::Completed(state.complete(EndedWith::PigEnd)))
                    }
                    // 可修补 → Post 只给反馈，不自行执行；把完整现场交回下一次 Executor。
                    // 是否还能继续不在这里单独计数；统一由下一次 Executor 的 executor_runs 判断。
                    Some(Marker::Next) => {
                        let feedback = strip_markers(&raw);
                        let executor_base = Self::build_executor_base_from_checkpoint(
                            &ctx.input,
                            &state,
                            &feedback,
                        )?;
                        state.post_protocol_retries = 0;
                        state.discard_candidate();
                        state.phase_raw.clear();
                        state.phase = Pig::Executor;
                        state.phase_base_body = Some(executor_base);
                        state.phase_transcript.clear();
                    }
                    // 执行路径根本错误 → 记录反馈并回 Pre 形成下一次 Executor 的计划。
                    // 同样不维护独立的“重规划次数”；唯一执行预算是 executor_runs。
                    Some(Marker::Failed) => {
                        let text = strip_markers(&raw);
                        state.failure_paths.push(text);
                        state.pre_output.clear();
                        state.post_protocol_retries = 0;
                        state.executor_checkpoint_body = None;
                        state.discard_candidate();
                        state.phase_raw.clear();
                        state.phase = Pig::Pre;
                        state.reset_phase_conversation();
                    }
                    // 无控制标记 → 视为核验器协议未完成，留在 Post 重试；不让 Post 自行执行任务
                    None => {
                        if state.post_protocol_retries >= MAX_POST_PROTOCOL_RETRIES {
                            return Err(Error::Budget(format!(
                                "Post 无标记协议重试次数超过 {MAX_POST_PROTOCOL_RETRIES} 次"
                            )));
                        }
                        state.post_protocol_retries += 1;
                        state.phase_raw.clear();
                    }
                },
            }
        }
    }
}

/// 一轮编排的内部上下文（本次请求 + 传输 + 进度 + 存储）。
struct Ctx {
    input: TurnInput,
    transport: Arc<dyn Transport>,
    progress: Option<ProgressSink>,
    store: Arc<Mutex<ContinuationStore>>,
}

impl Ctx {
    /// 组装并发送一只 pig 的子请求，返回这一轮的模型输出。
    ///
    /// `base_body` 已经在进入 pig 时注入过一次阶段提示；这里每轮只把当前 pig
    /// 已累积的原生 assistant/tool 对话接在后面。
    async fn call_pig(
        &self,
        pig: Pig,
        base_body: &serde_json::Value,
        phase_transcript: &[serde_json::Value],
        session: &str,
    ) -> Result<ModelOutput> {
        let mut body = base_body.clone();
        proto::append_transcript_items(&mut body, self.input.protocol, phase_transcript)?;

        // 头也照原样走；只在缺失时补 content-type / 会话头（会话头按现行决定保留）
        let mut headers: Vec<(String, String)> = self.input.base_headers.clone();
        let has = |headers: &[(String, String)], key: &str| {
            headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
        };
        if !has(&headers, "content-type") {
            headers.push(("content-type".into(), "application/json".into()));
        }
        if !has(&headers, SESSION_HEADER) {
            headers.push((SESSION_HEADER.into(), session.to_string()));
        }

        let body_bytes = bytes::Bytes::from(
            serde_json::to_vec(&body)
                .map_err(|e| TransportError::Send(format!("body 序列化失败: {e}")))?,
        );
        tracing::debug!(
            pig = pig.as_str(),
            bytes = body_bytes.len(),
            streaming = self.progress.is_some(),
            "sending subrequest"
        );

        let req = SubRequest {
            path: self.input.path.clone(),
            query: self.input.query.clone(),
            headers,
            body: body_bytes,
        };

        let resp = match &self.progress {
            // 流式：增量交给 MarkerFilter，确认安全的文本立刻回调出去
            Some(progress) => {
                progress(PigEvent::Start(pig));
                let filter = Arc::new(Mutex::new(MarkerFilter::new()));
                let stream_text = self.input.mode == proto::PigsMode::A;
                let sink: LiveSink = {
                    let filter = Arc::clone(&filter);
                    let progress = Arc::clone(progress);
                    Arc::new(move |event: proto::LiveEvent| {
                        match event {
                            // 文本要过控制标记过滤；思考原样转发
                            proto::LiveEvent::Text(delta) => {
                                let visible = filter
                                    .lock()
                                    .map(|mut f| f.push(&delta))
                                    .unwrap_or_default();
                                if stream_text && !visible.is_empty() {
                                    progress(PigEvent::Delta(visible));
                                }
                            }
                            proto::LiveEvent::Thinking(text) => {
                                progress(PigEvent::Thought(text));
                            }
                            proto::LiveEvent::ThinkingSummary { item_id, text } => {
                                progress(PigEvent::ThoughtSummary { item_id, text });
                            }
                            proto::LiveEvent::ThinkingSignature(signature) => {
                                progress(PigEvent::ThoughtSignature(signature));
                            }
                        }
                    })
                };
                let resp = self
                    .transport
                    .send_streaming(req, self.input.protocol, sink)
                    .await?;
                let tail = filter.lock().map(|mut f| f.finish()).unwrap_or_default();
                if stream_text && !tail.is_empty() {
                    progress(PigEvent::Delta(tail));
                }
                progress(PigEvent::End(pig));
                resp
            }
            None => self.transport.send(req).await?,
        };

        parse_output(self.input.protocol, pig, &resp)
    }
}

/// 把子请求响应解析成这一轮的模型输出（JSON 走结构解析，SSE 走全文汇总）。
fn parse_output(protocol: proto::Protocol, pig: Pig, resp: &SubResponse) -> Result<ModelOutput> {
    if !(200..300).contains(&resp.status) {
        return Err(Error::Transport(TransportError::Upstream {
            status: resp.status,
            body: String::from_utf8_lossy(&resp.body)
                .chars()
                .take(500)
                .collect(),
        }));
    }
    let output = if proto::is_sse_content_type(resp.content_type.as_deref()) {
        proto::parse_sse_output(protocol, &String::from_utf8_lossy(&resp.body))
    } else {
        match serde_json::from_slice::<serde_json::Value>(&resp.body) {
            Ok(value) => proto::parse_json_output(protocol, &value),
            Err(e) => {
                // 上游回了 2xx 但 body 不是 JSON：把现场带进错误，便于诊断
                let snippet: String = String::from_utf8_lossy(&resp.body)
                    .chars()
                    .take(300)
                    .collect();
                return Err(Error::Protocol(proto::Error::InvalidJsonWithBody {
                    reason: e.to_string(),
                    content_type: resp.content_type.clone().unwrap_or_default(),
                    snippet,
                }));
            }
        }
    };
    if output.is_truncated() {
        return Err(Error::OutputTruncated(
            protocol,
            pig.as_str(),
            output.stop_reason.clone().unwrap_or_default(),
        ));
    }
    if output.is_empty() {
        return Err(Error::NoText(protocol, pig.as_str()));
    }
    Ok(output)
}

/// 从客户端请求头里找会话头（大小写不敏感）。
pub fn find_client_session(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(SESSION_HEADER))
        .map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests;
