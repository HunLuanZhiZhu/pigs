//! pigs-orchestrator：分阶段编排引擎。 / Phased orchestration engine.
//!
//! Pre 负责规划与路由，Exec 执行，Post 核验。 / Pre plans/routes, Exec performs work, and Post verifies.
//! 控制标记和执行预算驱动状态机转换。 / Control markers and execution budgets drive phase transitions.
//!
//! 每个相位可跨多轮工具调用、执行和回填。 / Each phase may span multiple tool-call and tool-result exchanges.
//! 只有模型停止请求工具时，相位才有完整产出。 / A phase yields its output only when the model stops requesting tools.
//! 工具调用暂停当前相位；客户端回填后续接。 / Tool calls pause the phase, resumed when the client returns results.
//!
//! 约束：相对上游只改变模型名和末尾阶段提示。 / Constraint: only rewrite the model name and append phase instructions.
//! 网络通过 [`Transport`] 注入，本层不负责 HTTP/重试。 / Networking is injected via [`Transport`]; this layer owns neither HTTP nor retries.

pub mod lang;
pub mod markers;
pub mod prompts;
pub mod state;
pub mod transport;

use markers::{detect_terminal_marker, split_pre_decision, strip_markers, Marker, MarkerFilter};
use pigs_protocol as proto;
use proto::{ModelOutput, Part, ToolCall};
use state::{Continuation, ContinuationStore, TurnState};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use transport::{LiveSink, SubRequest, SubResponse, Transport, TransportError};

/// 所有子请求共用的会话头名称。 / Session header name shared across all subrequests.
pub const SESSION_HEADER: &str = "x-opencode-session";

/// 防止回环递归的内部令牌头。 / Internal token header to prevent loopback recursion.
pub const LOOPBACK_TOKEN_HEADER: &str = "x-pigs-loopback";

/// 默认 Exec 次数上限，可由配置覆盖。 / Default Exec run limit, overridable by configuration.
/// 工具暂停/恢复不增加 Exec 次数。 / Tool pauses/resumes do not increment Exec runs.
pub const DEFAULT_MAX_EXEC_RUNS: u32 = 3;
pub const DEFAULT_MAX_POST_PROTOCOL_RETRIES: u32 = 1;

/// 下游 usage 汇报模式。 / Downstream usage reporting mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageMode {
    /// max：返回 token 最多的单次上游 usage。 / Max: return usage from the highest-token upstream call.
    Max,
    /// sum：累加本次客户端请求全部上游 usage 数值。 / Sum: aggregate numeric usage across this client's upstream calls.
    Sum,
}

impl Default for UsageMode {
    fn default() -> Self {
        Self::Max
    }
}

/// 单个 pig 相位。 / Individual pig phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pig {
    /// 规划和任务路由。 / Planning and task routing.
    Pre,
    /// 获取信息并执行任务（常包含工具调用）。 / Information gathering and task execution, often using tools.
    Exec,
    /// 结果核验并选择接受、修补或重规划。 / Verify and choose acceptance, repair, or replanning.
    Post,
}

impl Pig {
    pub fn as_str(self) -> &'static str {
        match self {
            Pig::Pre => "pre",
            Pig::Exec => "exec",
            Pig::Post => "post",
        }
    }
}

/// 单相位生命周期事件，供代理流式响应使用。 / Phase lifecycle event for proxy streaming.
#[derive(Debug, Clone)]
pub enum PigEvent {
    /// 子请求已发送。 / Subrequest sent.
    Start(Pig),
    /// 已过滤标记的可见文本增量。 / Visible text delta with markers removed.
    Delta(String),
    /// 原样转发的思考增量。 / Reasoning delta forwarded unchanged.
    Thought(String),
    /// 带条目标识的 Responses 思考摘要。 / Responses reasoning summary with item ID.
    ThoughtSummary { item_id: String, text: String },
    /// Anthropic 思考块签名。 / Anthropic thinking-block signature.
    ThoughtSignature(String),
    /// 当前相位文本流结束。 / End of the phase text stream.
    End(Pig),
}

/// 按顺序报告编排生命周期事件。 / Callback emitting ordered orchestration events.
pub type ProgressSink = Arc<dyn Fn(PigEvent) + Send + Sync>;

/// 代理解析后的编排输入。 / Parsed orchestration input from the proxy.
#[derive(Debug, Clone)]
pub struct TurnInput {
    /// 请求路径对应的协议。 / Protocol determined from the request path.
    pub protocol: proto::Protocol,
    /// 输出模式 A 展示阶段正文，B 只提交最终业务正文。 / Mode A exposes phases; B commits only the final business text.
    pub mode: proto::PigsMode,
    /// 强制完整路径开关：Pre 仅规划，之后进入 Exec。 / Forced-full switch: planning-only Pre followed by Exec.
    pub force_full: bool,
    /// 原始客户端 JSON 请求体。 / Original client JSON request body.
    pub body: serde_json::Value,
    /// 子请求使用的协议路径。 / Protocol path preserved for subrequests.
    pub path: String,
    /// 原始查询串。 / Original query string.
    pub query: Option<String>,
    /// 原样携带的端到端请求头。 / End-to-end headers preserved in subrequests.
    pub base_headers: Vec<(String, String)>,
    /// 客户端会话标识；缺失时由编排生成。 / Client session ID, generated if absent.
    pub client_session: Option<String>,
}

/// 一轮编排的外部依赖。 / External dependencies for one orchestration turn.
pub struct Runtime {
    /// 子请求传输接口（生产为回环）。 / Subrequest transport, using loopback in production.
    pub transport: Arc<dyn Transport>,
    /// 工具暂停状态存储。 / Storage for tool-paused state.
    pub store: Arc<Mutex<ContinuationStore>>,
    /// 提供回调时按流式传输。 / A callback enables streaming subrequests and deltas.
    pub progress: Option<ProgressSink>,
}

/// 单轮编排结果：完成或工具暂停。 / Turn outcome: completed or paused for tools.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// 已完成的编排。 / Completed orchestration.
    Completed(TurnResult),
    /// 等待客户端工具结果的暂停状态。 / Paused state awaiting client tool results.
    Paused(PausedTurn),
}

/// 完成结果。 / Completed result.
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// 最终答复：模式 A 拼阶段文本，B 提交业务正文。 / Final answer: Mode A joins phases, B returns committed business text.
    pub text: String,
    /// 按模式选择的可见业务正文段。 / Visible business text segments selected by mode.
    pub visible: Vec<String>,
    /// 按序输出最终原生内容，不重发已消费工具调用。 / Final ordered native parts without consumed tool calls.
    pub parts: Vec<Part>,
    /// 结束原因，供日志诊断。 / Completion reason for diagnostics.
    pub ended_with: EndedWith,
    /// 使用的会话标识。 / Effective session ID.
    pub session: String,
    /// 实际进入的相位历史。 / History of visited phases.
    pub path: Vec<Pig>,
    /// 根据策略选出的 usage 原对象。 / Usage object selected by the reporting policy.
    pub usage: Option<serde_json::Value>,
    /// 最后一相位的停止原因。 / Stop reason from the final phase.
    pub stop_reason: Option<String>,
}

/// 工具调用导致的暂停产出。 / Output produced when paused for a tool.
#[derive(Debug, Clone)]
pub struct PausedTurn {
    /// 交付客户端的原生工具调用。 / Native tool calls delivered to the client.
    pub tool_calls: Vec<ToolCall>,
    /// 暂停时已可见的正文；模式 B 未验收候选不在其中。 / Visible text on pause; Mode B excludes unaccepted candidates.
    pub text: String,
    /// 暂停响应按序组合持久内容与本轮工具调用。 / Paused output combines persistent parts and current tool calls in order.
    pub parts: Vec<Part>,
    /// 暂停前聚合的请求 usage。 / Request usage aggregated through this pause.
    pub usage: Option<serde_json::Value>,
    /// 暂停轮次的原生停止原因。 / Native stop reason for the paused turn.
    pub stop_reason: Option<String>,
    /// 代理内部保存的续接句柄。 / Continuation handle kept internally by the proxy.
    pub continuation_id: String,
}

/// 结束类型。 / Completion type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedWith {
    /// Pre 简单路径直接完成。 / Simple path completed within Pre.
    SimplePath,
    /// Post 验收通过。 / Accepted by Post.
    PigEnd,
    /// Exec 达到执行上限后直接结束。 / Finish when the Exec run limit is reached.
    ExecLimit,
}

impl EndedWith {
    pub fn as_str(self) -> &'static str {
        match self {
            EndedWith::SimplePath => "SIMPLE_PATH",
            EndedWith::PigEnd => "PIGEND",
            EndedWith::ExecLimit => "EXECUTOR_LIMIT",
        }
    }
}

/// 编排错误。 / Orchestration error.
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
    #[error("Pre 控制协议错误: {0}")]
    PreProtocol(String),
    #[error("编排预算耗尽: {0}")]
    Budget(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 可复用的编排器，持有执行预算配置。 / Reusable orchestrator holding execution budget settings.
#[derive(Debug, Clone)]
pub struct Orchestrator {
    max_exec_runs: u32,
    max_post_protocol_retries: u32,
    usage_mode: UsageMode,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self {
            max_exec_runs: DEFAULT_MAX_EXEC_RUNS,
            max_post_protocol_retries: DEFAULT_MAX_POST_PROTOCOL_RETRIES,
            usage_mode: UsageMode::Max,
        }
    }
}

impl Orchestrator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(max_exec_runs: u32, max_post_protocol_retries: u32) -> Self {
        Self {
            max_exec_runs,
            max_post_protocol_retries,
            usage_mode: UsageMode::Max,
        }
    }

    pub fn with_limits_and_usage(
        max_exec_runs: u32,
        max_post_protocol_retries: u32,
        usage_mode: UsageMode,
    ) -> Self {
        Self {
            max_exec_runs,
            max_post_protocol_retries,
            usage_mode,
        }
    }

    /// 启动新编排轮次。 / Start a new orchestration turn.
    pub async fn run(&self, input: TurnInput, rt: Runtime) -> Result<Outcome> {
        let session = input
            .client_session
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let lang = lang::detect_lang(&proto::extract_last_user_text(&input.body, input.protocol));
        let mut state = TurnState::new_with_mode(lang, session, input.body.clone(), input.mode);
        state.force_full = input.force_full;
        self.drive(input, rt, state).await
    }

    /// 从客户端工具结果恢复暂停相位。 / Resume a tool-paused phase with client results.
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
        if input.mode != continuation.state.mode
            || input.force_full != continuation.state.force_full
        {
            return Err(Error::Budget(
                "continuation 的 PIGS 模式与恢复请求不一致".into(),
            ));
        }
        // 续接只用于判断请求是否匹配工具恢复。 / Continuation determines whether the request resumes tool calls.
        // 恢复后保留工具结果及之后的原生消息。 / Preserve tool results and all subsequent native messages on resume.
        let resume_items =
            proto::continuation_resume_items(input.protocol, &input.body, &continuation.pending);
        continuation.state.phase_transcript.extend(resume_items);
        // usage 以本次客户端请求为边界。 / Usage is scoped to each client API request.
        // 恢复时重置 usage，防止重复计费或沿用旧快照。 / Reset usage on resume to avoid duplicate charges or stale context usage.
        continuation.state.usage = None;
        self.drive(input, rt, continuation.state).await
    }

    /// 进入相位时创建基础请求并只注入一次提示。 / Create the phase base request and inject its prompt once.
    fn build_phase_base_body(
        input: &TurnInput,
        state: &TurnState,
        phase: Pig,
    ) -> Result<serde_json::Value> {
        let mut body = state.root_body.clone();
        match phase {
            Pig::Pre => {
                let instruction = if state.force_full {
                    prompts::pre_analysis_instruction(state.lang, &state.failure_paths)
                } else {
                    prompts::pre_instruction_for_model(
                        state.lang,
                        &state.failure_paths,
                        proto::get_model(&state.root_body).unwrap_or_default(),
                    )
                };
                proto::append_to_last_user_text(&mut body, input.protocol, &instruction)?;
            }
            Pig::Exec => {
                let instruction = prompts::exec_instruction(state.lang, &state.pre_output);
                proto::append_to_last_user_text(&mut body, input.protocol, &instruction)?;
            }
            Pig::Post => {
                return Err(Error::Budget(
                    "Post 必须继承 Exec 的完整上下文，不能从 root_body 重建".into(),
                ));
            }
        }
        Ok(body)
    }

    /// 保存 Exec 完成但尚未进入 Post 的快照。 / Snapshot the completed Exec context before Post.
    fn build_exec_checkpoint(
        input: &TurnInput,
        state: &TurnState,
    ) -> Result<serde_json::Value> {
        let mut body = state
            .phase_base_body
            .clone()
            .ok_or_else(|| Error::Budget("Exec 缺少 phase_base_body，无法建立 checkpoint".into()))?;
        proto::append_transcript_items(&mut body, input.protocol, &state.phase_transcript)?;
        Ok(body)
    }

    /// 从 Exec 快照构造 Post，只追加新核验指令。 / Build Post from the Exec checkpoint with one new verification message.
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

    /// Post 要求修补时恢复最近的 Exec 快照。 / Restore the last Exec checkpoint when Post requests repair.
    /// 复用 Exec 模板并注入 Post 反馈。 / Reuse the Exec template with Post feedback.
    fn build_exec_base_from_checkpoint(
        input: &TurnInput,
        state: &TurnState,
        feedback: &str,
    ) -> Result<serde_json::Value> {
        let mut body = state
            .exec_checkpoint_body
            .clone()
            .ok_or_else(|| Error::Budget("Post 缺少 Exec checkpoint，无法 PIGNEXT".into()))?;
        proto::push_user_message(
            &mut body,
            input.protocol,
            &prompts::exec_instruction(state.lang, feedback),
        )?;
        Ok(body)
    }

    /// 状态机按工具调用与控制结果推进。 / State machine advances according to tool calls and routing results.
    async fn drive(&self, input: TurnInput, rt: Runtime, mut state: TurnState) -> Result<Outcome> {
        let ctx = Ctx {
            input,
            transport: Arc::clone(&rt.transport),
            progress: rt.progress.clone(),
            store: Arc::clone(&rt.store),
        };
        loop {
            let phase = state.phase;
            // 相位历史记录新进入的相位，不重复记录工具续接。 / Phase history records entries, not repeated tool rounds.
            let first_entry = state.path.last() != Some(&phase);
            if first_entry {
                state.path.push(phase);
                if phase == Pig::Exec {
                    state.exec_runs += 1;
                }
            }
            tracing::info!(
                pig = phase.as_str(),
                first_entry,
                exec_runs = state.exec_runs,
                "pig round"
            );

            // 每个相位提示只注入一次。 / Inject each phase prompt exactly once.
            // 工具续接只追加对话历史。 / Tool resumes append transcript history without reinjecting prompts.
            if state.phase_base_body.is_none() {
                state.phase_base_body =
                    Some(Self::build_phase_base_body(&ctx.input, &state, phase)?);
            }
            let base_body = state
                .phase_base_body
                .as_ref()
                .expect("phase base body must exist");
            let output = ctx
                .call_pig(
                    phase,
                    base_body,
                    &state.phase_transcript,
                    &state.session,
                    state.pre_simple_decided,
                )
                .await?;
            // 自适应 Pre 必须回答五问并输出路由标记。 / Adaptive Pre must answer five questions and emit a routing marker.
            // 首次 PIGEND 之后简单任务仍在 Pre 完成。 / After the first PIGEND, simple execution remains in Pre.
            // 确认 PIGEND 边界前不发送工具调用。 / Never expose Pre tool calls before the PIGEND boundary.
            if phase == Pig::Pre && !state.force_full && !state.pre_simple_decided {
                let (marker, _, after) = split_pre_decision(&output.text).ok_or_else(|| {
                    Error::PreProtocol("必须先输出五问分析，再单独一行输出 PIGEND 或 PIGNEXT".into())
                })?;
                match marker {
                    Marker::End => state.pre_simple_decided = true,
                    Marker::Next if !after.is_empty() || !output.tool_calls.is_empty() => {
                        return Err(Error::PreProtocol(
                            "PIGNEXT 必须是最后一行，且不能同时请求工具调用".into(),
                        ));
                    }
                    Marker::Next => {}
                    Marker::Failed => {
                        return Err(Error::PreProtocol("Pre 不允许使用 PIGFAIL".into()));
                    }
                }
            }
            let round_transcript =
                proto::model_output_transcript_items(ctx.input.protocol, &output);
            // 工具调用只存在于本次暂停响应。 / Tool calls belong only to the current paused response.
            // 只持久化其它内容，完成响应不得重发工具调用。 / Persist other parts, never consumed tool calls in completed output.
            let paused_parts = if output.tool_calls.is_empty() {
                None
            } else {
                let mut parts = state.parts.clone();
                parts.extend(state.client_parts_for_round(&output));
                Some(parts)
            };
            state.record_round(&output.text, &output, self.usage_mode);
            state.phase_transcript.extend(round_transcript);

            // 工具请求暂停相位并交付原生调用。 / Tool requests pause, delivering native calls without ending the phase.
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
                let usage = state.usage.clone();
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
                    usage,
                    stop_reason: output.stop_reason,
                    continuation_id,
                }));
            }

            // 无工具调用则根据控制标记推进。 / Without tool calls, route according to control markers.
            let raw = state.phase_raw_text();
            match phase {
                // ---------------- Pre：规划与分流 / Planning and routing ----------------
                Pig::Pre => {
                    // 强制完整模式使用共享分析 Pre，不解析 Pre 控制标记。 / Forced-full modes use shared analysis-only Pre and ignore Pre markers.
                    // 简单路径在 PIGEND 后执行，完成工具轮次才提交。 / Simple paths execute after PIGEND, committing when tool rounds finish.
                    // -pig 隐藏分析，-pigs 保留阶段正文。 / -pig hides analysis while -pigs preserves phase text.
                    if !state.force_full && state.pre_simple_decided {
                        let (_, _, business_text) = split_pre_decision(&raw).ok_or_else(|| {
                            Error::PreProtocol("简单路径的 PIGEND 分界线丢失".into())
                        })?;
                        let business_text = strip_markers(&business_text);
                        if business_text.is_empty() {
                            return Err(Error::PreProtocol(
                                "简单路径 PIGEND 后没有实际任务回答".into(),
                            ));
                        }
                        state.commit_text(business_text);
                        return Ok(Outcome::Completed(state.complete(EndedWith::SimplePath)));
                    }
                    // 复杂任务以 PIGNEXT 结束 Pre 并进入 Exec。 / Complex tasks end Pre with PIGNEXT and enter Exec.
                    // 已验证首轮无工具调用和非法后续文本。 / The initial decision disallows tool calls and extra text after PIGNEXT.
                    state.pre_output = strip_markers(&raw);
                    state.post_protocol_retries = 0;
                    state.exec_checkpoint_body = None;
                    state.discard_candidate();
                    state.phase_raw.clear();
                    state.phase = Pig::Exec;
                    state.reset_phase_conversation();
                },
                // ---------------- Exec：执行 / Execution ----------------
                Pig::Exec => {
                    // Exec 不解析标记；模式 B 暂存正文候选。 / Exec does not parse markers; Mode B stores a candidate.
                    state.set_candidate(strip_markers(&raw));

                    // 执行预算按高层 Exec 次数计算。 / Budget counts high-level Exec runs.
                    // 预算耗尽后直接提交，避免无修补机会的 Post。 / On budget exhaustion, finish without a Post that cannot trigger repair.
                    // 强制完整模式正常成功路径必须经过三阶段。 / Forced-full successful paths pass through all three phases.
                    // 即使上限为一次也要让 Post 验收首次 Exec。 / Even with one allowed Exec, run Post once to verify it.
                    if state.exec_runs >= self.max_exec_runs
                        && !(state.force_full && state.exec_runs == 1)
                    {
                        state.commit_candidate();
                        return Ok(Outcome::Completed(
                            state.complete(EndedWith::ExecLimit),
                        ));
                    }

                    // 有剩余执行机会时保存快照并进入 Post。 / Save a checkpoint and enter Post while budget remains.
                    let checkpoint = Self::build_exec_checkpoint(&ctx.input, &state)?;
                    let post_base =
                        Self::build_post_base_from_checkpoint(&ctx.input, &state, &checkpoint)?;
                    state.exec_checkpoint_body = Some(checkpoint);
                    state.post_protocol_retries = 0;
                    state.phase_raw.clear();
                    state.phase = Pig::Post;
                    state.phase_base_body = Some(post_base);
                    state.phase_transcript.clear();
                }
                // ---------------- Post：核验与路由 / Verification and routing ----------------
                Pig::Post => match detect_terminal_marker(&raw) {
                    // Post 验收通过则完成。 / Post acceptance completes the turn.
                    Some(Marker::End) => {
                        state.commit_candidate();
                        return Ok(Outcome::Completed(state.complete(EndedWith::PigEnd)))
                    }
                    // 可修补时 Post 只反馈并交给下一轮 Exec。 / On repair, Post supplies feedback and hands control to Exec.
                    // 后续执行额度由 Exec 次数统一决定。 / Further execution eligibility is determined by Exec run count.
                    Some(Marker::Next) => {
                        let feedback = strip_markers(&raw);
                        let exec_base = Self::build_exec_base_from_checkpoint(
                            &ctx.input,
                            &state,
                            &feedback,
                        )?;
                        state.post_protocol_retries = 0;
                        state.discard_candidate();
                        state.phase_raw.clear();
                        state.phase = Pig::Exec;
                        state.phase_base_body = Some(exec_base);
                        state.phase_transcript.clear();
                    }
                    // 路径失败则保存反馈并回 Pre 重规划。 / On failed strategy, save feedback and return to Pre.
                    // 重规划不单独计次，统一使用 Exec 预算。 / Replanning has no separate budget; Exec runs are the limit.
                    Some(Marker::Failed) => {
                        let text = strip_markers(&raw);
                        state.failure_paths.push(text);
                        state.pre_output.clear();
                        state.pre_simple_decided = false;
                        state.post_protocol_retries = 0;
                        state.exec_checkpoint_body = None;
                        state.discard_candidate();
                        state.phase_raw.clear();
                        state.phase = Pig::Pre;
                        state.reset_phase_conversation();
                    }
                    // Post 缺少标记时仅重试控制协议，不执行任务。 / Missing Post markers trigger protocol retry, not task execution.
                    None => {
                        if state.post_protocol_retries >= self.max_post_protocol_retries {
                            return Err(Error::Budget(format!(
                                "Post 无标记协议重试次数超过 {} 次",
                                self.max_post_protocol_retries
                            )));
                        }
                        // 保留 Post 历史并追加独立用户纠错消息。 / Preserve Post history and append a separate user correction.
                        // 只纠正控制协议，不重跑业务任务。 / Correct the control protocol without rerunning prior work.
                        let mut retry_body = state.phase_base_body.clone().ok_or_else(|| {
                            Error::Budget("Post 缺少 phase_base_body，无法协议重试".into())
                        })?;
                        proto::append_transcript_items(
                            &mut retry_body,
                            ctx.input.protocol,
                            &state.phase_transcript,
                        )?;
                        proto::push_user_message(
                            &mut retry_body,
                            ctx.input.protocol,
                            &prompts::control_protocol_retry_instruction(state.lang),
                        )?;
                        state.phase_base_body = Some(retry_body);
                        state.phase_transcript.clear();
                        state.post_protocol_retries += 1;
                        state.phase_raw.clear();
                    }
                },
            }
        }
    }
}

/// 单轮编排上下文：请求、传输、事件与存储。 / Turn context: request, transport, events, and storage.
struct Ctx {
    input: TurnInput,
    transport: Arc<dyn Transport>,
    progress: Option<ProgressSink>,
    store: Arc<Mutex<ContinuationStore>>,
}

impl Ctx {
    /// 发送相位子请求并解析本轮模型输出。 / Send a phase subrequest and parse the model turn.
    ///
    /// 基础请求已包含阶段提示。 / The base request already contains the phase prompt.
    /// 后续只追加累计的原生对话。 / Later calls append only accumulated native assistant/tool history.
    async fn call_pig(
        &self,
        pig: Pig,
        base_body: &serde_json::Value,
        phase_transcript: &[serde_json::Value],
        session: &str,
        pre_simple_decided: bool,
    ) -> Result<ModelOutput> {
        let mut body = base_body.clone();
        proto::append_transcript_items(&mut body, self.input.protocol, phase_transcript)?;

        // 保留头，缺失时补内容类型和会话标识。 / Preserve headers; fill in missing content type or session ID.
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
            // 流式过滤标记并即时转发安全文本。 / Stream safe text immediately after marker filtering.
            Some(progress) => {
                progress(PigEvent::Start(pig));
                let filter = Arc::new(Mutex::new(MarkerFilter::new()));
                let stream_text = self.input.mode == proto::PigsMode::A;
                let filter_pre_thoughts =
                    self.input.mode == proto::PigsMode::B && pig == Pig::Pre && !self.input.force_full;
                let pre_text = Arc::new(Mutex::new(String::new()));
                let allow_thoughts = Arc::new(Mutex::new(!filter_pre_thoughts || pre_simple_decided));
                let sink: LiveSink = {
                    let filter = Arc::clone(&filter);
                    let pre_text = Arc::clone(&pre_text);
                    let allow_thoughts = Arc::clone(&allow_thoughts);
                    let progress = Arc::clone(progress);
                    Arc::new(move |event: proto::LiveEvent| {
                        match event {
                            // 过滤标记并隐藏模式 B 的 Pre 分析。 / Filter markers and suppress Mode B Pre analysis.
                            proto::LiveEvent::Text(delta) => {
                                if filter_pre_thoughts {
                                    if let Ok(mut raw) = pre_text.lock() {
                                        raw.push_str(&delta);
                                        if matches!(split_pre_decision(&raw), Some((Marker::End, _, _))) {
                                            if let Ok(mut allow) = allow_thoughts.lock() {
                                                *allow = true;
                                            }
                                        }
                                    }
                                }
                                let visible = filter
                                    .lock()
                                    .map(|mut f| f.push(&delta))
                                    .unwrap_or_default();
                                if stream_text && !visible.is_empty() {
                                    progress(PigEvent::Delta(visible));
                                }
                            }
                            proto::LiveEvent::Thinking(text) => {
                                if allow_thoughts.lock().map(|a| *a).unwrap_or(false) {
                                    progress(PigEvent::Thought(text));
                                }
                            }
                            proto::LiveEvent::ThinkingSummary { item_id, text } => {
                                if allow_thoughts.lock().map(|a| *a).unwrap_or(false) {
                                    progress(PigEvent::ThoughtSummary { item_id, text });
                                }
                            }
                            proto::LiveEvent::ThinkingSignature(signature) => {
                                if allow_thoughts.lock().map(|a| *a).unwrap_or(false) {
                                    progress(PigEvent::ThoughtSignature(signature));
                                }
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

/// 将 JSON/SSE 子响应转换成模型轮次。 / Parse JSON or aggregate SSE into a model turn.
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
                // 2xx 响应若不是合法 JSON，保留上下文用于诊断。 / Capture context when a successful HTTP status carries invalid JSON.
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

/// 不区分大小写查找客户端会话头。 / Find the client session header case-insensitively.
pub fn find_client_session(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(SESSION_HEADER))
        .map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests;
