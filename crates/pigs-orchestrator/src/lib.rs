//! pigs-orchestrator —— 编排引擎。
//!
//! 三只 pig 组成一头 pigs：Pre（规划/分流）→ Executor（执行）→ Post（验收），
//! 由 PIGEND / PIGFAIL 控制标记与预算驱动的状态机串起来。
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

use markers::{detect_marker, strip_markers, Marker, MarkerFilter};
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

/// 预算常量（legacy 默认值；刻意不进配置——它们是编排语义的一部分）。
/// - `MAX_PRE_REPLANS`：PIGFAIL 回到 Pre 重规划的次数上限；
/// - `MAX_POST_ITERATIONS`：Post 无标记输出的连续重试次数上限。
/// 超预算一律判为本轮失败（绝不假装成功）。
const MAX_PRE_REPLANS: u32 = 2;
const MAX_POST_ITERATIONS: u32 = 3;

/// 单只 pig（一个相位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pig {
    /// 规划 / 分流 / GOAL 声明。
    Pre,
    /// 信息收集 + 起草答复（工具调用多发生在这个相位）。
    Executor,
    /// 审阅 + GOAL 验收 + 失败/重规划。
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
    /// 本轮完成（正常结束；预算耗尽仍然是 `Err`）。
    Completed(TurnResult),
    /// 模型要工具：调用已交给客户端，等它执行完带结果回来。
    Paused(PausedTurn),
}

/// 完成后的结果。
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// 最终答复 = 各只 pig 可见文本按顺序空行拼接（已剥离控制标记）。
    pub text: String,
    /// 每段可见文本（按执行顺序，诊断/测试用）。
    pub visible: Vec<String>,
    /// 给客户端的内容序列（文本 + 思考 + 工具调用 + 其它原生块，按顺序）。
    pub parts: Vec<Part>,
    /// 结束方式（诊断/日志用）。
    pub ended_with: EndedWith,
    /// 实际使用的会话头值。
    pub session: String,
    /// 完整走过的 pig 序列（含工具往返后的重复相位，诊断用）。
    pub path: Vec<Pig>,
    /// 跨相位累加的上游 usage（原样对象；上游没给就是 None）。
    pub usage: Option<serde_json::Value>,
    /// 最后一个相位给的停止原因（原样回传）。
    pub stop_reason: Option<String>,
}

/// 被工具调用打断时的产出。
#[derive(Debug, Clone)]
pub struct PausedTurn {
    /// 交给客户端执行的工具调用（协议原生，原样）。
    pub tool_calls: Vec<ToolCall>,
    /// 到目前为止的可见文本（流式客户端已经收到了；非流式用它拼响应）。
    pub text: String,
    /// 到目前为止的内容序列（给客户端的内容，顺序权威）。
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
}

impl EndedWith {
    pub fn as_str(self) -> &'static str {
        match self {
            EndedWith::SimplePath => "SIMPLE_PATH",
            EndedWith::PigEnd => "PIGEND",
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
        let state = TurnState::new(lang, session);
        Self::drive(input, rt, state).await
    }

    /// 接着被工具调用打断的那只 pig 继续（客户端带工具结果回来了）。
    pub async fn resume(
        &self,
        input: TurnInput,
        rt: Runtime,
        continuation: Continuation,
    ) -> Result<Outcome> {
        tracing::info!(
            continuation = %continuation.id,
            phase = continuation.state.phase.as_str(),
            "恢复相位（工具结果已回填）"
        );
        Self::drive(input, rt, continuation.state).await
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
            }
            tracing::info!(pig = phase.as_str(), first_entry, "pig round");

            // 相位指令：Pre 带失败路径、Executor 带 Pre 分析、Post 是模板本身
            let instruction = match phase {
                Pig::Pre => prompts::pre_instruction(state.lang, &state.failure_paths),
                Pig::Executor => prompts::executor_instruction(state.lang, &state.pre_output),
                Pig::Post => prompts::post_instruction(state.lang),
            };
            // 产物接回对话的只有 Post（Pre/Executor 的输入由指令模板承载）
            let transcript: &[String] = if phase == Pig::Post {
                &state.transcript
            } else {
                &[]
            };

            let output = ctx
                .call_pig(phase, &instruction, transcript, &state.session)
                .await?;
            state.record_round(&output.text, &output);

            // 模型要工具 → 暂停，把调用原样交给客户端（相位不结束）
            if !output.tool_calls.is_empty() {
                let pending: Vec<String> =
                    output.tool_calls.iter().map(|call| call.id.clone()).collect();
                tracing::info!(
                    pig = phase.as_str(),
                    calls = pending.len(),
                    "模型请求工具调用，暂停相位等待客户端执行"
                );
                let text = state.final_text();
                let parts = state.parts.clone();
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
                        return Ok(Outcome::Completed(state.complete(EndedWith::SimplePath)))
                    }
                    // 路径失败 → 记录，回 Pre 重规划（预算内）
                    Some(Marker::Failed) => {
                        if state.pre_replans >= MAX_PRE_REPLANS {
                            return Err(Error::Budget(format!(
                                "Pre 重规划次数超过 {MAX_PRE_REPLANS} 次"
                            )));
                        }
                        state.failure_paths.push(strip_markers(&raw));
                        state.pre_replans += 1;
                        state.pre_output.clear();
                        state.post_iterations = 0;
                        state.phase_raw.clear();
                    }
                    // 正常计划 → 交给 Executor
                    None => {
                        state.pre_output = strip_markers(&raw);
                        state.phase_raw.clear();
                        state.phase = Pig::Executor;
                    }
                },
                // ---------------- Executor：执行 ----------------
                Pig::Executor => {
                    // 不解析标记：Executor 之后总是进 Post 验收
                    state.transcript.push(strip_markers(&raw));
                    state.phase_raw.clear();
                    state.phase = Pig::Post;
                }
                // ---------------- Post：验收 / 路由 ----------------
                Pig::Post => match detect_marker(&raw) {
                    // 验收通过 → 整轮结束
                    Some(Marker::End) => {
                        return Ok(Outcome::Completed(state.complete(EndedWith::PigEnd)))
                    }
                    // 执行走偏 → 清空产物，回 Pre 重规划（预算内）
                    Some(Marker::Failed) => {
                        if state.pre_replans >= MAX_PRE_REPLANS {
                            return Err(Error::Budget(format!(
                                "Pre 重规划次数超过 {MAX_PRE_REPLANS} 次"
                            )));
                        }
                        let text = strip_markers(&raw);
                        state.failure_paths.push(text.clone());
                        state.transcript.push(text);
                        state.pre_replans += 1;
                        state.pre_output.clear();
                        state.post_iterations = 0;
                        state.phase_raw.clear();
                        state.phase = Pig::Pre;
                    }
                    // 推进了但没完成 → 提示词要求它继续执行任务，所以再走一次 Post
                    None => {
                        if state.post_iterations >= MAX_POST_ITERATIONS {
                            return Err(Error::Budget(format!(
                                "Post 无标记重试次数超过 {MAX_POST_ITERATIONS} 次"
                            )));
                        }
                        state.transcript.push(strip_markers(&raw));
                        state.post_iterations += 1;
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
    /// 体只做两件事：把产物作为 assistant 消息接回对话、把相位指令追加到尾部。
    /// **不改任何字段**（`tools`、`stream`、`temperature`… 原样透传给上游）。
    async fn call_pig(
        &self,
        pig: Pig,
        instruction: &str,
        transcript: &[String],
        session: &str,
    ) -> Result<ModelOutput> {
        let mut body = self.input.body.clone();
        for text in transcript {
            proto::push_assistant_message(&mut body, self.input.protocol, text)?;
        }
        proto::append_instruction(&mut body, self.input.protocol, instruction)?;

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
                let sink: LiveSink = {
                    let filter = Arc::clone(&filter);
                    let progress = Arc::clone(progress);
                    Arc::new(move |event: proto::LiveEvent| {
                        match event {
                            // 文本要过控制标记过滤；思考原样转发
                            proto::LiveEvent::Text(delta) => {
                                let visible =
                                    filter.lock().map(|mut f| f.push(&delta)).unwrap_or_default();
                                if !visible.is_empty() {
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
                if !tail.is_empty() {
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
                let snippet: String =
                    String::from_utf8_lossy(&resp.body).chars().take(300).collect();
                return Err(Error::Protocol(proto::Error::InvalidJsonWithBody {
                    reason: e.to_string(),
                    content_type: resp.content_type.clone().unwrap_or_default(),
                    snippet,
                }));
            }
        }
    };
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
