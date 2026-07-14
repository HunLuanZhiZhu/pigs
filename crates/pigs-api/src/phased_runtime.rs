//! 相位化轮次运行时：Pre → Executor → Post，带标记路由。
//! Phased turn runtime: Pre -> Executor -> Post with marker routing.
//!
//! 设计要点 / Key design points:
//! - 调用方的 system 消息被提取到 `system_prompt` 字段，透传给 LLM，三相位不变。
//! - 每相位 clone base_history + 追加相位特定 user 消息。
//! - 相位结束条件：工具循环空闲（无 tool_use）。
//! - 路由标记 PIGEND/PIGFAILED 在 PRE 和 POST 中解析。

use std::sync::Arc;

use pigs_config::{ApiFormat, AppConfig, Language, ResolvedModel};
use pigs_core::{
    ApiClient, ApiRequest, ContentBlock, Message, StreamCallback, StreamEvent, ToolRegistry,
    ToolResult,
};
use pigs_llm::{create_client_for_endpoint, Provider as LlmProvider};
use tracing::{debug, info, warn};

use crate::phased_markers::{detect_marker, strip_markers, Marker};
use crate::phased_phase::Phase;
use crate::phased_prompts::{executor_user_payload, post_user_payload, pre_user_payload};
use crate::phased_tools::info_tool_registry;

/// 运行时限制参数。
/// Runtime limit parameters.
#[derive(Debug, Clone)]
pub struct RuntimeLimits {
    /// 每相位最大工具调用轮次。
    /// Max tool-call rounds per phase.
    pub max_tool_rounds_per_phase: u32,
    /// Executor ← Post 回环最大次数。
    /// Max Executor ← Post loop count.
    pub max_executor_loops: u32,
    /// PRE 重规划最大次数（PIGFAILED 后回到 PRE）。
    /// Max PRE replan count (after PIGFAILED back to PRE).
    pub max_pre_replans: u32,
    /// LLM 最大输出 token 数。
    /// Max output tokens for LLM.
    pub max_tokens: u32,
    /// LLM 温度参数。
    /// LLM temperature.
    pub temperature: f32,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_tool_rounds_per_phase: 4,
            max_executor_loops: 3,
            max_pre_replans: 2,
            max_tokens: 4096,
            temperature: 0.2,
        }
    }
}

/// 轮次进度事件（用于 API SSE 推送 / 日志）。
/// High-level turn progress for API SSE / logging.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TurnProgress {
    /// 相位开始 / Phase started.
    PhaseStart { phase: String },
    /// 相位输出完成 / Phase output complete.
    PhaseOutput { phase: String, text: String },
    /// 文本增量（流式）/ Text delta (streaming).
    TextDelta { phase: String, text: String },
    /// 工具调用开始 / Tool call started.
    ToolStart { phase: String, name: String },
    /// 工具调用结束 / Tool call ended.
    ToolEnd {
        phase: String,
        name: String,
        is_error: bool,
    },
    /// 整轮结束 / Turn ended.
    TurnEnd {
        ended_with: String,
        final_text: String,
    },
}

/// 进度回调类型：Arc 包装的闭包。
/// Progress callback type: Arc-wrapped closure.
pub type ProgressSink = Arc<dyn Fn(TurnProgress) + Send + Sync>;

/// 轮次事件记录（用于结果回溯）。
/// Turn event record (for result tracing).
#[derive(Debug, Clone)]
pub struct TurnEvent {
    /// 事件类型（如 "phase_start", "phase_output", "turn_end"）。
    /// Event kind (e.g. "phase_start", "phase_output", "turn_end").
    pub kind: String,
    /// 所属相位（如 "pre", "executor", "post"）。
    /// Associated phase (e.g. "pre", "executor", "post").
    pub phase: Option<String>,
    /// 事件文本（如相位输出内容）。
    /// Event text (e.g. phase output content).
    #[allow(dead_code)]
    pub text: Option<String>,
}

/// 一轮相位对话的最终结果。
/// Final result of a phased turn.
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// 最终面向用户的文本。
    /// Final user-facing text.
    pub final_text: String,
    /// 事件列表（全相位）。
    /// Event list (all phases).
    pub events: Vec<TurnEvent>,
    /// 结束标记（如 "PIGEND", "PIGFAILED_BUDGET"）。
    /// End marker (e.g. "PIGEND", "PIGFAILED_BUDGET").
    pub ended_with: String,
}

/// 相位化 Agent 运行时主结构体。
/// Phased agent runtime main struct.
pub struct PhasedRuntime {
    /// LLM API 客户端（Arc 包装，支持共享）。
    /// LLM API client (Arc-wrapped, supports sharing).
    pub api: Arc<dyn ApiClient>,
    /// 实际后端模型 ID（如 "gpt-4o"）。
    /// Actual backend model ID (e.g. "gpt-4o").
    pub remote_model: String,
    /// 对外暴露的包装模型 ID（如 "gpt-4o-pig"）。
    /// Wrapped model id exposed to API consumers (e.g. `auto-pig`).
    pub wrapped_model: String,
    /// 工具注册表 —— 全量内置工具 + internal_notes。
    /// Tool registry — full built-in set (same as pigs-cli) + internal_notes.
    pub tools: ToolRegistry,
    /// 运行时限制参数。
    /// Runtime limit parameters.
    pub limits: RuntimeLimits,
    /// 相位提示词 / payload 语言（默认 zh）。
    /// Phase prompt / payload language (`zh` default via config).
    pub language: Language,
    /// 是否启用相位编排。
    /// Whether phased orchestration is enabled.
    ///
    /// - `true`: 完整 Pre→Executor→Post 循环（`-pig` 模型）
    /// - `false`: 单次 LLM 调用，不做编排（非 `-pig` 模型）
    ///
    /// When `true`: full Pre→Executor→Post loop (`-pig` models).
    /// When `false`: single LLM call, no orchestration (non-`-pig` models).
    pub is_pig: bool,
}

impl PhasedRuntime {
    /// 从应用配置构建运行时。
    /// Build runtime from app config.
    pub fn from_config(config: &AppConfig) -> anyhow::Result<Self> {
        let resolved = config
            .resolve_model(&config.model)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let is_pig = resolved.is_pig;
        Self::from_resolved(
            resolved,
            RuntimeLimits {
                max_tokens: config.max_tokens,
                temperature: config.temperature,
                ..RuntimeLimits::default()
            },
            config.language_or_default(),
        )
        .map(|mut rt| {
            rt.is_pig = is_pig;
            rt
        })
    }

    /// 从已解析的模型配置构建运行时（直连上游 LLM）。
    /// Build runtime from a resolved model config (direct upstream connection).
    #[allow(dead_code)]
    pub fn from_resolved(
        resolved: ResolvedModel,
        limits: RuntimeLimits,
        language: Language,
    ) -> anyhow::Result<Self> {
        // 根据 API 格式选择 provider / Select provider based on API format
        let provider = match resolved.api {
            ApiFormat::OpenAI => LlmProvider::OpenAI,
            ApiFormat::OpenAIChat => LlmProvider::OpenAIChat,
            ApiFormat::Anthropic => LlmProvider::Anthropic,
        };
        let api = create_client_for_endpoint(
            provider,
            &resolved.remote_model,
            &resolved.api_key,
            &resolved.base_url,
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        // 包装模型名 = 原名 + "-pig" / Wrapped model name = original + "-pig"
        let wrapped_model = format!("{}-pig", resolved.name);
        let is_pig = resolved.is_pig;
        Ok(Self {
            api,
            remote_model: resolved.remote_model,
            wrapped_model,
            tools: info_tool_registry(),
            limits,
            language,
            is_pig,
        })
    }

    /// 从外部 ApiClient 构建运行时（供 CLI 用，注入 ProxyApiClient）。
    /// Build runtime from an external ApiClient (for CLI, injects ProxyApiClient).
    ///
    /// 与 `from_resolved` 不同，此构造函数**不**内部创建 `pigs-llm` 客户端，
    /// 而是接受调用方传入的 `Arc<dyn ApiClient>`（通常是 `ProxyApiClient`）。
    /// 这样所有 LLM 请求统一经过 `pigs-proxy` 的 `dispatch_in_process` 重试逻辑。
    ///
    /// Unlike `from_resolved`, this constructor does **not** create an internal
    /// `pigs-llm` client. Instead it accepts a caller-provided `Arc<dyn ApiClient>`
    /// (typically `ProxyApiClient`). This routes all LLM requests through
    /// `pigs-proxy`'s `dispatch_in_process` retry logic.
    pub fn from_client(
        api: Arc<dyn ApiClient>,
        remote_model: String,
        is_pig: bool,
        limits: RuntimeLimits,
        language: Language,
    ) -> Self {
        let wrapped_model = if is_pig {
            format!("{remote_model}-pig")
        } else {
            remote_model.clone()
        };
        Self {
            api,
            remote_model,
            wrapped_model,
            tools: info_tool_registry(),
            limits,
            language,
            is_pig,
        }
    }

    /// 运行一轮相位对话（无进度回调）。
    /// Run a phased turn (no progress callback).
    #[allow(dead_code)]
    pub async fn run_turn(&self, messages: &[Message]) -> anyhow::Result<TurnResult> {
        self.run_turn_with_progress(messages, None, None).await
    }

    /// 在调用方的完整消息数组上运行一轮相位对话。
    ///
    /// `messages` 是**未修改的**调用方数组 —— 可能包含开头的
    /// `system` 角色消息、历次对话，末尾是当前用户问题。运行时会：
    /// 1. 提取所有 `system` 角色消息 → `system_prompt` 字段
    ///    （原样透传给 LLM，永不被覆盖）。
    /// 2. 将剩余消息（去掉最后一条 user）作为 base_history。
    /// 3. 每相位 clone base_history 并追加相位特定 user 消息：
    ///    `{用户原问题} + {相位指令 + 产物}`。
    ///
    /// `model_override` 用于覆盖 `self.remote_model`。HTTP 路径需要此参数：
    /// 一个共享的 `PhasedRuntime` 实例可能服务不同的 `-pig` 模型请求
    /// （如 `xopglm52-pig`、`auto-pig`），每次请求的真实模型名不同，
    /// 必须从客户端请求中动态传入。CLI 路径传 `None`，使用启动时配置的模型名。
    ///
    /// Run a phased turn on the caller's complete message array.
    ///
    /// `messages` is the **unmodified** caller array — it may include a
    /// leading `system` role message, prior turns, and ends with the
    /// current user question. The runtime:
    /// 1. Extracts any `system` role message(s) → `system_prompt` field
    ///    (passed through to the LLM as-is, never overwritten).
    /// 2. Treats the remaining messages (minus the last user) as history.
    /// 3. Per phase, clones history and appends a phase-specific user
    ///    message: `{original user question} + {phase instructions + products}`.
    ///
    /// `model_override` overrides `self.remote_model`. Required by the HTTP
    /// path: a single shared `PhasedRuntime` may serve different `-pig` model
    /// requests (e.g. `xopglm52-pig`, `auto-pig`), each with a different real
    /// model name that must come from the client request. The CLI path passes
    /// `None` to use the model configured at startup.
    pub async fn run_turn_with_progress(
        &self,
        messages: &[Message],
        progress: Option<ProgressSink>,
        model_override: Option<&str>,
    ) -> anyhow::Result<TurnResult> {
        // 确定本轮使用的模型名：优先用覆盖值，否则用启动时配置的值
        // Determine the model name for this turn: override takes precedence,
        // otherwise fall back to the configured remote_model.
        let effective_model = model_override.unwrap_or(&self.remote_model);
        // 将调用方消息拆分为：system_prompt, base_history, user_question
        // Split caller messages into: system_prompt, base_history, user_question.
        let mut system_parts: Vec<String> = Vec::new();
        let mut non_system: Vec<Message> = Vec::with_capacity(messages.len());
        for m in messages {
            if m.role == pigs_core::MessageRole::System {
                let t = m.text_content();
                if !t.is_empty() {
                    system_parts.push(t);
                }
            } else {
                non_system.push(m.clone());
            }
        }
        // 多条 system 消息用空行连接 / Join multiple system messages with blank lines
        let system_prompt: String = system_parts.join("\n\n");

        // 最后一条非 system 消息是当前用户问题。
        // 它之前的所有消息是 base_history（历次对话）。
        // The last non-system message is the current user question.
        // Everything before it is base_history (prior turns).
        if non_system.is_empty() {
            return Err(anyhow::anyhow!("no user message found"));
        }
        let (base_history, last_msg) = non_system.split_at(non_system.len() - 1);
        let user_question = last_msg[0].text_content();
        let base_history: Vec<Message> = base_history.to_vec();

        // 进度回调辅助闭包 / Progress callback helper closure
        let emit = |p: TurnProgress| {
            if let Some(sink) = &progress {
                sink(p);
            }
        };
        let mut events = Vec::new();

        // ================================================================
        // 非-pig 模式：单次 LLM 调用，不做相位编排
        // Non-pig mode: single LLM call, no phased orchestration
        // ================================================================
        if !self.is_pig {
            events.push(ev("phase_start", Some("direct"), None));
            emit(TurnProgress::PhaseStart {
                phase: "direct".into(),
            });
            // payload = 纯用户原问题（不追加相位提示词）
            // payload = plain user question (no phase instructions appended)
            let payload = user_question.clone();
            let text = self
                .run_phase(
                    Phase::Pre,
                    &base_history,
                    &payload,
                    &system_prompt,
                    progress.as_ref(),
                    effective_model,
                )
                .await?;
            let final_text = strip_markers(&text);
            events.push(ev("turn_end", Some("direct"), Some(final_text.clone())));
            emit(TurnProgress::TurnEnd {
                ended_with: "DIRECT".into(),
                final_text: final_text.clone(),
            });
            return Ok(TurnResult {
                final_text,
                events,
                ended_with: "DIRECT".into(),
            });
        }

        // ================================================================
        // -pig 模式：完整 Pre→Executor→Post 相位编排
        // -pig mode: full Pre→Executor→Post phased orchestration
        // ================================================================
        // 本轮相位间传递的产物 / Products passed between phases this turn
        let mut pre_output = String::new(); // PRE 计划 / GOAL
        let mut executor_draft = String::new(); // Executor 草稿
        let mut failure_paths: Vec<String> = Vec::new(); // 失败路径记录
        let mut executor_loops: u32 = 0; // Executor ← Post 回环计数
        let mut pre_replans: u32 = 0; // PRE 重规划计数
        let mut phase = Phase::Pre; // 当前相位
        let mut last_post_feedback = String::new(); // POST 给 Executor 的反馈

        loop {
            match phase {
                // ================================================================
                // PRE 相位：规划 / 分流
                // PRE phase: planning / triage
                // ================================================================
                Phase::Pre => {
                    info!(phase = "pre", "starting pre phase");
                    events.push(ev("phase_start", Some("pre"), None));
                    emit(TurnProgress::PhaseStart {
                        phase: "pre".into(),
                    });
                    // payload = 用户原问题 + PRE 相位指令（含失败路径）
                    // payload = user question first, then phase instructions.
                    let payload = format!(
                        "{}\n\n{}",
                        user_question,
                        pre_user_payload(self.language, &failure_paths)
                    );
                    let text = self
                        .run_phase(
                            Phase::Pre,
                            &base_history,
                            &payload,
                            &system_prompt,
                            progress.as_ref(),
                            effective_model,
                        )
                        .await?;
                    events.push(ev("phase_output", Some("pre"), Some(text.clone())));
                    emit(TurnProgress::PhaseOutput {
                        phase: "pre".into(),
                        text: text.clone(),
                    });
                    // 解析控制标记 / Parse control markers
                    match detect_marker(&text) {
                        // PIGEND → 整轮直接结束（简单路径）
                        Some(Marker::End) => {
                            let final_text = strip_markers(&text);
                            events.push(ev("turn_end", Some("pre"), Some(final_text.clone())));
                            emit(TurnProgress::TurnEnd {
                                ended_with: "PIGEND".into(),
                                final_text: final_text.clone(),
                            });
                            return Ok(TurnResult {
                                final_text,
                                events,
                                ended_with: "PIGEND".into(),
                            });
                        }
                        // PIGFAILED → 记录失败路径，重规划
                        Some(Marker::Failed) => {
                            failure_paths.push(strip_markers(&text));
                            pre_replans += 1;
                            if pre_replans > self.limits.max_pre_replans {
                                // 超出重规划预算 → 强制结束
                                return Ok(TurnResult {
                                    final_text: strip_markers(&text),
                                    events,
                                    ended_with: "PIGFAILED_BUDGET".into(),
                                });
                            }
                            // 预算内 → 重新进入 PRE
                        }
                        // 无标记 → 计划交给 Executor
                        None => {
                            pre_output = strip_markers(&text);
                            phase = Phase::Executor;
                        }
                    }
                }
                // ================================================================
                // EXECUTOR 相位：信息收集 + 起草
                // EXECUTOR phase: information gathering + drafting
                // ================================================================
                Phase::Executor => {
                    info!(phase = "executor", "starting executor phase");
                    events.push(ev("phase_start", Some("executor"), None));
                    emit(TurnProgress::PhaseStart {
                        phase: "executor".into(),
                    });
                    // payload = 用户原问题 + Executor 指令 + PRE 产物 + POST 反馈
                    let payload = format!(
                        "{}\n\n{}",
                        user_question,
                        executor_user_payload(self.language, &pre_output, &last_post_feedback,)
                    );
                    let text = self
                        .run_phase(
                            Phase::Executor,
                            &base_history,
                            &payload,
                            &system_prompt,
                            progress.as_ref(),
                            effective_model,
                        )
                        .await?;
                    events.push(ev("phase_output", Some("executor"), Some(text.clone())));
                    emit(TurnProgress::PhaseOutput {
                        phase: "executor".into(),
                        text: text.clone(),
                    });
                    // 不解析标记：PRE 之后总是进入 POST。
                    // No marker parsing: after PRE, always go to POST.
                    executor_draft = text;
                    last_post_feedback.clear();
                    phase = Phase::Post;
                }
                // ================================================================
                // POST 相位：审阅 + GOAL 验收 + 路由
                // POST phase: review + goal check + routing
                // ================================================================
                Phase::Post => {
                    info!(phase = "post", "starting post phase");
                    events.push(ev("phase_start", Some("post"), None));
                    emit(TurnProgress::PhaseStart {
                        phase: "post".into(),
                    });
                    // payload = 用户原问题 + POST 指令 + PRE 产物 + Executor 草稿
                    let payload = format!(
                        "{}\n\n{}",
                        user_question,
                        post_user_payload(self.language, &pre_output, &executor_draft,)
                    );
                    let text = self
                        .run_phase(
                            Phase::Post,
                            &base_history,
                            &payload,
                            &system_prompt,
                            progress.as_ref(),
                            effective_model,
                        )
                        .await?;
                    events.push(ev("phase_output", Some("post"), Some(text.clone())));
                    emit(TurnProgress::PhaseOutput {
                        phase: "post".into(),
                        text: text.clone(),
                    });
                    // 解析控制标记 / Parse control markers
                    match detect_marker(&text) {
                        // PIGEND → 整轮正常结束
                        Some(Marker::End) => {
                            let final_text = strip_markers(&text);
                            events.push(ev("turn_end", Some("post"), Some(final_text.clone())));
                            emit(TurnProgress::TurnEnd {
                                ended_with: "PIGEND".into(),
                                final_text: final_text.clone(),
                            });
                            return Ok(TurnResult {
                                final_text,
                                events,
                                ended_with: "PIGEND".into(),
                            });
                        }
                        // PIGFAILED → 路径失败，清空产物，回到 PRE 重规划
                        Some(Marker::Failed) => {
                            failure_paths.push(strip_markers(&text));
                            pre_output.clear();
                            executor_draft.clear();
                            last_post_feedback.clear();
                            pre_replans += 1;
                            if pre_replans > self.limits.max_pre_replans {
                                return Ok(TurnResult {
                                    final_text: failure_paths
                                        .last()
                                        .cloned()
                                        .unwrap_or_else(|| "failed".into()),
                                    events,
                                    ended_with: "PIGFAILED_BUDGET".into(),
                                });
                            }
                            phase = Phase::Pre;
                        }
                        // 无标记 → POST 反馈给 Executor，回环重试
                        None => {
                            last_post_feedback = strip_markers(&text);
                            executor_loops += 1;
                            if executor_loops > self.limits.max_executor_loops {
                                // 超出回环预算 → 强制结束，返回最新草稿
                                warn!("executor loop budget exceeded; forcing end");
                                return Ok(TurnResult {
                                    final_text: if executor_draft.is_empty() {
                                        last_post_feedback.clone()
                                    } else {
                                        executor_draft.clone()
                                    },
                                    events,
                                    ended_with: "EXECUTOR_LOOP_BUDGET".into(),
                                });
                            }
                            phase = Phase::Executor;
                        }
                    }
                }
            }
        }
    }

    /// 执行单个相位的工具循环。
    ///
    /// Execute a single phase's tool loop.
    ///
    /// 循环逻辑 / Loop logic:
    /// 1. 构建 ApiRequest（system + messages + tools）
    /// 2. 调用 LLM（流式）
    /// 3. 如果有 tool_use → 执行工具 → 追加结果 → 回到步骤 1
    /// 4. 如果无 tool_use → 相位结束，返回文本
    /// 5. 超过最大轮次 → 强制结束，返回最后文本
    async fn run_phase(
        &self,
        phase: Phase,
        base_history: &[Message],
        user_payload: &str,
        system: &str,
        progress: Option<&ProgressSink>,
        model: &str,
    ) -> anyhow::Result<String> {
        // 构建消息：base_history + 本相位 user payload
        // Build messages: base_history + this phase's user payload
        let mut messages = base_history.to_vec();
        messages.push(Message::user(user_payload));
        let tool_defs = self.tools.definitions();
        let mut rounds = 0u32;
        let mut last_text = String::new();
        let phase_name = phase.as_str().to_string();
        // 所有相位都流式输出文本 / All phases stream text
        let stream_visible = true;

        loop {
            rounds += 1;
            // 超过最大工具轮次 → 强制结束
            // Exceeds max tool rounds → force end
            if rounds > self.limits.max_tool_rounds_per_phase + 1 {
                warn!(?phase, rounds, "max tool rounds hit; returning last text");
                break;
            }

            // 构建 LLM 请求 / Build LLM request
            let request = ApiRequest::new(model.to_string(), messages.clone())
                .with_system_prompt(system) // 用户原 system，透传
                .with_tools(tool_defs.clone())
                .with_max_tokens(self.limits.max_tokens)
                .with_temperature(self.limits.temperature);

            debug!(phase = phase.as_str(), round = rounds, "llm call");
            // 流式回调：收集文本 + 转发进度
            // Streaming callback: collect text + forward progress
            let cb = CollectingCallback {
                phase: phase_name.clone(),
                stream_visible,
                progress: progress.cloned(),
                text: std::sync::Mutex::new(String::new()),
            };
            let response = self
                .api
                .send_message_streaming(request, &cb)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;

            // 优先取 response 的文本内容；若为空则取 callback 收集的文本
            // Prefer response text; fall back to callback-collected text
            last_text = response.text_content();
            if last_text.is_empty() {
                if let Ok(guard) = cb.text.lock() {
                    if !guard.is_empty() {
                        last_text = guard.clone();
                    }
                }
            }

            // 提取 tool_use 调用 / Extract tool_use calls
            let tool_uses: Vec<(String, String, serde_json::Value)> = response
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            // 无工具调用 → 相位结束 / No tool calls → phase ends
            if tool_uses.is_empty() {
                break;
            }

            // 有工具调用 → 追加 assistant 消息，执行工具，追加 tool_result
            // Tool calls → append assistant message, execute tools, append tool_result
            messages.push(Message::assistant(response.content.clone()));
            for (id, name, input) in tool_uses {
                info!(phase = phase.as_str(), tool = %name, "tool call");
                // 通知进度：工具开始 / Notify progress: tool start
                if let Some(sink) = progress {
                    sink(TurnProgress::ToolStart {
                        phase: phase_name.clone(),
                        name: name.clone(),
                    });
                }
                // 执行工具 / Execute the tool
                let result = self.tools.execute(&name, input).await;
                let (output, is_error) = match result {
                    Ok(ToolResult {
                        output, is_error, ..
                    }) => (output, is_error),
                    Err(e) => (format!("tool error: {e}"), true),
                };
                // 通知进度：工具结束 / Notify progress: tool end
                if let Some(sink) = progress {
                    sink(TurnProgress::ToolEnd {
                        phase: phase_name.clone(),
                        name: name.clone(),
                        is_error,
                    });
                }
                // 追加 tool_result 到消息列表 / Append tool_result to messages
                messages.push(Message::tool_result(&id, &output, is_error));
            }
        }

        Ok(last_text)
    }
}

/// 流式回调收集器：收集文本增量 + 转发进度事件。
/// Streaming callback collector: collects text deltas + forwards progress events.
struct CollectingCallback {
    /// 相位名称（用于进度事件）。
    /// Phase name (for progress events).
    phase: String,
    /// 是否将文本增量转发给 ProgressSink。
    /// Whether to forward text deltas to ProgressSink.
    stream_visible: bool,
    /// 进度回调（可选）。
    /// Progress callback (optional).
    progress: Option<ProgressSink>,
    /// 收集的文本（线程安全）。
    /// Collected text (thread-safe).
    text: std::sync::Mutex<String>,
}

impl StreamCallback for CollectingCallback {
    fn on_event(&self, event: &StreamEvent) {
        if let StreamEvent::TextDelta(delta) = event {
            // 收集文本增量 / Collect text delta
            if let Ok(mut g) = self.text.lock() {
                g.push_str(delta);
            }
            // 转发给 ProgressSink（如果启用）/ Forward to ProgressSink (if enabled)
            if self.stream_visible {
                if let Some(sink) = &self.progress {
                    if !delta.is_empty() {
                        sink(TurnProgress::TextDelta {
                            phase: self.phase.clone(),
                            text: delta.clone(),
                        });
                    }
                }
            }
        }
    }
}

/// 构造 TurnEvent 的辅助函数。
/// Helper to construct a TurnEvent.
fn ev(kind: &str, phase: Option<&str>, text: Option<String>) -> TurnEvent {
    TurnEvent {
        kind: kind.into(),
        phase: phase.map(|s| s.to_string()),
        text,
    }
}
