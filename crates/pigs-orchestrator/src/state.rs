//! 相位状态及工具调用暂停/恢复句柄。 / Phase state and handles for tool-call pause/resume.
//!
//! 一个相位可跨越多次工具调用与结果回填。 / A phase can span multiple tool calls and returned results.
//! 模型不再请求工具时相位才完成；等待现场保存在 [`Continuation`]。 / A phase finishes when no more tools are requested; [`Continuation`] stores its paused state.

use crate::lang::Lang;
use crate::{Pig, UsageMode};
use pigs_protocol::{Part, PigsMode};
use serde_json::Value;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 可跨工具暂停保留的一轮编排状态。 / Turn state persisted across tool pauses.
#[derive(Debug, Clone)]
pub struct TurnState {
    /// 当前相位。 / Current phase.
    pub phase: Pig,
    /// 本轮输出模式。 / Current output mode.
    pub mode: PigsMode,
    /// 强制完整路径：Pre 只规划，不提前结束。 / Forced-full path: planning-only Pre without early completion.
    pub force_full: bool,
    /// 原始客户端请求体；Pre/Exec 基于此构造，Post 继承 Exec 上下文。 / Original client body for Pre/Exec; Post inherits the Exec context.
    pub root_body: Value,
    /// 当前相位初次进入时构造的基础请求。 / Base request created once on phase entry.
    pub phase_base_body: Option<Value>,
    /// 相位内已有的协议原生助手/工具对话。 / Protocol-native assistant/tool history within the phase.
    pub phase_transcript: Vec<Value>,
    /// 最近 Exec 完成时的完整上下文快照（尚无 Post 指令）。 / Complete snapshot after the last Exec, before Post instruction.
    /// Post 返回 PIGNEXT 时从此快照恢复并注入核验反馈。 / Restore this snapshot on Post PIGNEXT and feed back verification notes.
    pub exec_checkpoint_body: Option<Value>,
    /// 已进入的相位历史，供诊断。 / Entered-phase history for diagnostics.
    pub path: Vec<Pig>,
    /// Pre 分析，供 Exec 指令使用。 / Pre analysis consumed by Exec.
    pub pre_output: String,
    /// Pre 已输出 PIGEND 后，工具续接仍留在 Pre。 / After Pre PIGEND, tool continuation stays in Pre without rerouting.
    pub pre_simple_decided: bool,
    /// 失败路径历史，供 Pre 重规划。 / Failure history for Pre replanning.
    pub failure_paths: Vec<String>,
    /// 模式 A 保留阶段文本，模式 B 只保留提交正文。 / Mode A retains phase text; Mode B retains committed business text.
    pub visible: Vec<String>,
    /// 模式 B 的 Exec 候选，在 Post 接受前不可见。 / Mode B Exec candidate, hidden until Post accepts it.
    pub candidate_text: String,
    /// 模式 B 已提交的业务正文。 / Mode B committed business text.
    pub committed_text: String,
    /// 持久内容序列保序并移除控制标记；工具调用不持久化。 / Persistent ordered parts exclude markers and consumed tool calls.
    pub parts: Vec<Part>,
    /// 本相位原始文本轮次，含路由标记。 / Raw per-request phase text including routing markers.
    pub phase_raw: Vec<String>,
    /// Exec 高层执行次数，不重复计算工具续接。 / High-level Exec run count, excluding tool continuation.
    pub exec_runs: u32,
    /// Post 协议重试次数，独立于任务执行预算。 / Post protocol retries, separate from execution budget.
    pub post_protocol_retries: u32,
    /// 按用户问题判定的语言。 / Language inferred from the user request.
    pub lang: Lang,
    /// 所有子请求共用的会话标识。 / Shared session identifier for all subrequests.
    pub session: String,
    /// 本次客户端请求按指定模式聚合的 usage。 / Usage aggregated for this client request.
    pub usage: Option<Value>,
    /// 最后一相位返回的停止原因。 / Stop reason from the last phase.
    pub stop_reason: Option<String>,
}

impl TurnState {
    pub fn new(lang: Lang, session: String, root_body: Value) -> Self {
        Self::new_with_mode(lang, session, root_body, PigsMode::A)
    }

    pub fn new_with_mode(lang: Lang, session: String, root_body: Value, mode: PigsMode) -> Self {
        Self {
            phase: Pig::Pre,
            mode,
            force_full: false,
            root_body,
            phase_base_body: None,
            phase_transcript: Vec::new(),
            exec_checkpoint_body: None,
            path: Vec::new(),
            pre_output: String::new(),
            pre_simple_decided: false,
            failure_paths: Vec::new(),
            visible: Vec::new(),
            candidate_text: String::new(),
            committed_text: String::new(),
            parts: Vec::new(),
            phase_raw: Vec::new(),
            exec_runs: 0,
            post_protocol_retries: 0,
            lang,
            session,
            usage: None,
            stop_reason: None,
        }
    }

    /// 进入新相位或重规划时清理临时对话现场。 / Clear temporary conversation state when entering or replanning a phase.
    pub fn reset_phase_conversation(&mut self) {
        self.phase_base_body = None;
        self.phase_transcript.clear();
    }

    /// 将当前模型输出整理成客户端内容序列。 / Convert the model output to ordered client-facing parts.
    ///
    /// 工具调用只在暂停响应中交付。 / Tool calls are delivered only in paused responses.
    pub(crate) fn client_parts_for_round(&self, output: &crate::proto::ModelOutput) -> Vec<Part> {
        // 首次 Pre 标记前的非文本块属于内部分析。 / Native nontext blocks before the first Pre marker are internal analysis.
        // 标记后的原生内容及工具续接应保留。 / Preserve post-marker parts and tool continuations.
        let first_pre_round = self.mode == PigsMode::B
            && self.phase == Pig::Pre
            && !self.force_full
            && self.phase_raw.is_empty();
        let mut seen_marker = !first_pre_round;
        let mut pre_text = String::new();
        let has_text = self
            .parts
            .iter()
            .any(|part| matches!(part, Part::Text(text) if !text.is_empty()));
        let mut parts = Vec::new();
        let mut marker_filter = crate::markers::MarkerFilter::new();
        let mut text_added = false;
        for part in &output.parts {
            match part {
                Part::Text(text) => {
                    if first_pre_round && !seen_marker {
                        pre_text.push_str(text);
                        seen_marker = crate::markers::split_pre_decision(&pre_text).is_some();
                    }
                    if self.mode == PigsMode::A {
                        // PIGEND 可能跨文本块，须跨块过滤。 / PIGEND may span text parts; filtering must cross boundaries.
                        let cleaned = marker_filter.push(text);
                        if !cleaned.is_empty() {
                            let piece = if has_text && !text_added {
                                format!("\n\n{cleaned}")
                            } else {
                                cleaned
                            };
                            parts.push(Part::Text(piece));
                            text_added = true;
                        }
                    }
                }
                // -pig 隐藏首轮 Pre 的五问和思考。 / -pig hides initial Pre analysis and reasoning.
                // 分界线后的原生块与工具调用正常转发。 / Forward native parts and tool calls after the boundary.
                Part::ToolCall(call) => parts.push(Part::ToolCall(call.clone())),
                other if seen_marker => parts.push(other.clone()),
                _ => {},
            }
        }
        if self.mode == PigsMode::A {
            let tail = marker_filter.finish();
            if !tail.is_empty() {
                let piece = if has_text && !text_added {
                    format!("\n\n{tail}")
                } else {
                    tail
                };
                parts.push(Part::Text(piece));
            }
        }
        parts
    }

    /// 记录模型轮次：持久化文本/思考，不持久化工具调用。 / Record a model turn; persist text/reasoning but not tool calls.
    pub fn record_round(
        &mut self,
        raw_text: &str,
        output: &crate::proto::ModelOutput,
        usage_mode: UsageMode,
    ) {
        // 先处理内容块，再保存原文，以确定控制边界。 / Process parts before storing raw text to establish marker boundaries.
        let client_parts = self.client_parts_for_round(output);
        self.phase_raw.push(raw_text.to_string());
        let visible = crate::markers::strip_markers(raw_text);
        if self.mode == PigsMode::A && !visible.is_empty() {
            self.visible.push(visible);
        }
        // 工具调用只在暂停时交付一次，不得重放。 / Deliver a tool call once on pause; never replay it in final output.
        self.parts.extend(
            client_parts.into_iter().filter(|part| !matches!(part, Part::ToolCall(_))),
        );
        if let Some(usage) = &output.usage {
            match usage_mode {
                UsageMode::Max => {
                    // max 模式按单次调用 token 选 usage，不拼合字段。 / Max mode selects one call's entire usage by token count.
                    let larger = match &self.usage {
                        None => true,
                        Some(current) => usage_total_tokens(usage) > usage_total_tokens(current),
                    };
                    if larger {
                        self.usage = Some(usage.clone());
                    }
                }
                UsageMode::Sum => match &mut self.usage {
                    None => self.usage = Some(usage.clone()),
                    Some(current) => sum_usage_json(current, usage),
                },
            }
        }
        // 停止原因取最后一轮，避免保留过期工具状态。 / Use the final turn's stop reason, not stale tool-call status.
        self.stop_reason = output.stop_reason.clone();
    }

    /// 记录 Exec 正文；模式 B 将其设为待验收候选。 / Record Exec text as a pending candidate in Mode B.
    pub fn set_candidate(&mut self, text: String) {
        if self.mode == PigsMode::B {
            self.candidate_text = text;
        }
    }

    /// 模式 B 丢弃 Exec 候选。 / Discard the Mode B Exec candidate.
    pub fn discard_candidate(&mut self) {
        if self.mode == PigsMode::B {
            self.candidate_text.clear();
        }
    }

    /// 模式 B 提交最终业务正文。 / Commit final business text in Mode B.
    pub fn commit_text(&mut self, text: String) {
        if self.mode == PigsMode::B {
            self.committed_text = text.clone();
            self.visible.clear();
            if !text.is_empty() {
                self.visible.push(text);
            }
        }
    }

    /// 模式 B 接受当前 Exec 候选。 / Accept the current Exec candidate in Mode B.
    pub fn commit_candidate(&mut self) {
        if self.mode == PigsMode::B {
            let text = std::mem::take(&mut self.candidate_text);
            self.commit_text(text);
        }
    }

    /// 返回本相位含标记的原始文本。 / Return raw phase text including routing markers.
    pub fn phase_raw_text(&self) -> String {
        self.phase_raw.join("\n\n")
    }

    /// 按顺序以空行拼接可见答复段。 / Join visible response segments with blank lines.
    pub fn final_text(&self) -> String {
        match self.mode {
            PigsMode::A => self
                .visible
                .iter()
                .filter(|part| !part.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n"),
            PigsMode::B => self.committed_text.clone(),
        }
    }

    /// 将编排状态收尾为一轮结果。 / Finalize the turn state into an outcome.
    pub fn complete(mut self, ended_with: crate::EndedWith) -> crate::TurnResult {
        let text = self.final_text();
        if self.mode == PigsMode::B && !text.is_empty() {
            self.parts.push(Part::Text(text.clone()));
        }
        crate::TurnResult {
            text,
            parts: self.parts,
            visible: self.visible,
            ended_with,
            session: self.session,
            path: self.path,
            usage: self.usage,
            stop_reason: self.stop_reason,
        }
    }
}

/// 单次上游调用 token：优先读取 `total_tokens`。 / Tokens for one upstream call: prefer `total_tokens`.
/// 缺失时回退为输入加输出，避免重复计算缓存/推理子字段。 / Otherwise sum primary input/output counts without double-counting subfields.
fn usage_total_tokens(usage: &Value) -> u64 {
    if let Some(total) = usage.get("total_tokens").and_then(Value::as_u64) {
        return total;
    }
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    input.saturating_add(output)
}

/// sum 模式递归相加数值字段，保留独有字段。 / Sum mode recursively adds numeric fields and preserves unique fields.
/// 相同路径上的非数值字段保留首值。 / Keep the first value for conflicting nonnumeric metadata.
fn sum_usage_json(current: &mut Value, next: &Value) {
    match (current, next) {
        (Value::Object(current), Value::Object(next)) => {
            for (key, next_value) in next {
                match current.get_mut(key) {
                    Some(current_value) => sum_usage_json(current_value, next_value),
                    None => {
                        current.insert(key.clone(), next_value.clone());
                    }
                }
            }
        }
        (Value::Number(current), Value::Number(next)) => {
            if let (Some(a), Some(b)) = (current.as_u64(), next.as_u64()) {
                *current = serde_json::Number::from(a.saturating_add(b));
            } else if let (Some(a), Some(b)) = (current.as_i64(), next.as_i64()) {
                *current = serde_json::Number::from(a.saturating_add(b));
            } else if let (Some(a), Some(b)) = (current.as_f64(), next.as_f64()) {
                if let Some(sum) = serde_json::Number::from_f64(a + b) {
                    *current = sum;
                }
            }
        }
        _ => {}
    }
}

/// 等待客户端工具结果的相位续接现场。 / Paused phase state awaiting client tool results.
#[derive(Debug, Clone)]
pub struct Continuation {
    /// 续接存储标识，供日志诊断。 / Continuation ID for storage and diagnostics.
    pub id: String,
    /// 待回填的工具调用标识。 / Pending tool-call IDs.
    pub pending: Vec<String>,
    /// 相位现场。 / Phase state.
    pub state: TurnState,
    /// 创建时间，供 TTL 清理。 / Creation time for TTL expiration.
    pub created: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationSummary {
    pub id: String,
    pub session: String,
    pub mode: PigsMode,
    pub phase: Pig,
    pub pending: Vec<String>,
    pub age_ms: u128,
    pub in_flight: bool,
}

/// 续接认领的匹配结果。 / Continuation claim result.
#[derive(Debug, Clone)]
pub enum ClaimMatch {
    /// 已原子认领，续接后必须提交或回滚。 / Atomically claimed; caller must commit or roll back after resume.
    Claimed(Continuation),
    /// 匹配现场已被其他恢复请求占用。 / Matched state is in flight for another resume.
    InFlight(String),
    /// 没有匹配现场。 / No matching state.
    NoMatch,
}

/// 内存续接存储：限制容量/TTL，按工具结果标识和输出模式匹配。 / In-memory continuation store bounded by capacity/TTL and matched by tool-result IDs and output mode.
///
/// 匹配需要当前请求包含所有待回填 ID。 / A match requires every pending ID in the current request.
/// 多个现场同时匹配时优先最新项。 / Prefer the newest continuation when several match.
#[derive(Debug)]
pub struct ContinuationStore {
    entries: Vec<Continuation>,
    /// 已认领但尚未提交的续接标识。 / Claimed but not yet consumed continuation IDs.
    in_flight: HashSet<String>,
    max: usize,
    ttl: Duration,
    counter: u64,
}

impl Default for ContinuationStore {
    fn default() -> Self {
        Self::new(64, Duration::from_secs(30 * 60))
    }
}

impl ContinuationStore {
    pub fn new(max: usize, ttl: Duration) -> Self {
        Self {
            entries: Vec::new(),
            in_flight: HashSet::new(),
            max: max.max(1),
            ttl,
            counter: 0,
        }
    }

    /// 保存续接并返回 ID，同时清理超时或超容量记录。 / Store a continuation and return its ID while evicting expired or excess entries.
    pub fn insert(&mut self, mut continuation: Continuation) -> String {
        self.evict_expired();
        self.counter += 1;
        let id = format!("pigc-{}", self.counter);
        continuation.id = id.clone();
        continuation.created = Instant::now();
        self.entries.push(continuation);
        // 正在恢复的现场不得被容量清理。 / Never evict an in-flight continuation.
        // 全部在途时可暂时超容量，随后恢复限制。 / Temporarily exceed capacity if all entries are in flight.
        while self.entries.len() > self.max {
            let Some(index) = self
                .entries
                .iter()
                .position(|entry| !self.in_flight.contains(&entry.id))
            else {
                break;
            };
            self.entries.remove(index);
        }
        id
    }

    /// 原子认领匹配项而不删除，成功后提交。 / Atomically claim without deleting; commit after successful resume.
    /// 失败则回滚，保留现场以便重试。 / Roll back on failure to preserve state for retry.
    pub fn claim_match_for_mode(&mut self, result_ids: &[String], mode: PigsMode) -> ClaimMatch {
        self.claim_match_inner(result_ids, Some(mode))
    }

    fn claim_match_inner(&mut self, result_ids: &[String], mode: Option<PigsMode>) -> ClaimMatch {
        self.evict_expired();
        let Some(index) = self.entries.iter().rposition(|entry| {
            mode.map(|expected| entry.state.mode == expected).unwrap_or(true)
                && !entry.pending.is_empty()
                && entry
                    .pending
                    .iter()
                    .all(|id| result_ids.iter().any(|got| got == id))
        }) else {
            return ClaimMatch::NoMatch;
        };

        let id = self.entries[index].id.clone();
        if self.in_flight.contains(&id) {
            return ClaimMatch::InFlight(id);
        }
        self.in_flight.insert(id);
        ClaimMatch::Claimed(self.entries[index].clone())
    }

    /// 恢复成功后消费认领项。 / Consume the claimed continuation after successful resume.
    pub fn commit_claim(&mut self, id: &str) -> bool {
        if !self.in_flight.remove(id) {
            return false;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.id == id) {
            self.entries.remove(index);
            true
        } else {
            false
        }
    }

    /// 恢复失败只解除认领，允许重试。 / Release the claim on failure, allowing retry.
    pub fn rollback_claim(&mut self, id: &str) -> bool {
        self.in_flight.remove(id)
    }

    /// 兼容接口：认领后立即消费。 / Compatibility API: claim then immediately consume.
    pub fn take_match(&mut self, result_ids: &[String]) -> Option<Continuation> {
        match self.claim_match_inner(result_ids, None) {
            ClaimMatch::Claimed(continuation) => {
                self.commit_claim(&continuation.id);
                Some(continuation)
            }
            ClaimMatch::InFlight(_) | ClaimMatch::NoMatch => None,
        }
    }

    /// 低层兼容接口；正式恢复必须显式认领/提交/回滚。 / Low-level compatibility API; production requires explicit claim/commit/rollback.
    pub fn take_match_for_mode(
        &mut self,
        result_ids: &[String],
        mode: PigsMode,
    ) -> Option<Continuation> {
        match self.claim_match_for_mode(result_ids, mode) {
            ClaimMatch::Claimed(continuation) => {
                self.commit_claim(&continuation.id);
                Some(continuation)
            }
            ClaimMatch::InFlight(_) | ClaimMatch::NoMatch => None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 诊断快照：清理过期记录后返回待恢复项，不消费。 / Diagnostic snapshot: remove expired entries and list pending states without consuming.
    pub fn summaries(&mut self) -> Vec<ContinuationSummary> {
        self.evict_expired();
        self.entries
            .iter()
            .map(|entry| ContinuationSummary {
                id: entry.id.clone(),
                session: entry.state.session.clone(),
                mode: entry.state.mode,
                phase: entry.state.phase,
                pending: entry.pending.clone(),
                age_ms: entry.created.elapsed().as_millis(),
                in_flight: self.in_flight.contains(&entry.id),
            })
            .collect()
    }

    fn evict_expired(&mut self) {
        let ttl = self.ttl;
        let in_flight = &self.in_flight;
        self.entries
            .retain(|entry| in_flight.contains(&entry.id) || entry.created.elapsed() < ttl);
        let live_ids: HashSet<String> = self.entries.iter().map(|entry| entry.id.clone()).collect();
        self.in_flight.retain(|id| live_ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    fn state() -> TurnState {
        TurnState::new(Lang::Zh, "sess".into(), json!({"messages": []}))
    }

    /// 构造仅含 usage 的模型输出。 / Build a model output containing only usage.
    fn output_with_usage(usage: Value) -> crate::proto::ModelOutput {
        crate::proto::ModelOutput {
            text: String::new(),
            tool_calls: Vec::new(),
            parts: Vec::new(),
            stop_reason: Some("end_turn".into()),
            usage: Some(usage),
        }
    }

    #[test]
    fn usage_max_keeps_largest_total_object_verbatim() {
        // max 选择最大 token 调用的完整 usage JSON。 / Max selects the whole usage JSON of the highest-token call.
        let mut state = state();
        state.record_round(
            "第一轮",
            &output_with_usage(json!({
                "prompt_tokens": 20,
                "completion_tokens": 30,
                "total_tokens": 50,
                "prompt_tokens_details": {"cached_tokens": 10}
            })),
            UsageMode::Max,
        );
        state.record_round(
            "第二轮",
            &output_with_usage(json!({
                "prompt_tokens": 55,
                "completion_tokens": 5,
                "total_tokens": 60,
                "prompt_tokens_details": {"cached_tokens": 40, "cache_write_tokens": 7}
            })),
            UsageMode::Max,
        );
        let usage = state.usage.expect("应有 usage");
        assert_eq!(usage["total_tokens"], 60);
        assert_eq!(usage["prompt_tokens"], 55);
        assert_eq!(usage["completion_tokens"], 5);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 40);
        assert_eq!(usage["prompt_tokens_details"]["cache_write_tokens"], 7);
    }

    #[test]
    fn usage_max_falls_back_to_input_plus_output_when_total_is_missing() {
        let mut state = state();
        state.record_round(
            "第一轮",
            &output_with_usage(json!({"input_tokens": 40, "output_tokens": 10})),
            UsageMode::Max,
        );
        state.record_round(
            "第二轮",
            &output_with_usage(json!({"input_tokens": 30, "output_tokens": 30})),
            UsageMode::Max,
        );
        let usage = state.usage.expect("应有 usage");
        assert_eq!(usage["input_tokens"], 30);
        assert_eq!(usage["output_tokens"], 30);
    }

    #[test]
    fn usage_sum_adds_all_numeric_json_fields_recursively() {
        let mut state = state();
        state.record_round(
            "第一轮",
            &output_with_usage(json!({
                "prompt_tokens": 20,
                "completion_tokens": 3,
                "total_tokens": 23,
                "prompt_tokens_details": {"cached_tokens": 12, "cache_write_tokens": 2},
                "completion_tokens_details": {"reasoning_tokens": 1}
            })),
            UsageMode::Sum,
        );
        state.record_round(
            "第二轮",
            &output_with_usage(json!({
                "prompt_tokens": 30,
                "completion_tokens": 7,
                "total_tokens": 37,
                "prompt_tokens_details": {"cached_tokens": 25, "cache_write_tokens": 4},
                "completion_tokens_details": {"reasoning_tokens": 5},
                "provider_extra": 9
            })),
            UsageMode::Sum,
        );
        let usage = state.usage.expect("应有 usage");
        assert_eq!(usage["prompt_tokens"], 50);
        assert_eq!(usage["completion_tokens"], 10);
        assert_eq!(usage["total_tokens"], 60);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 37);
        assert_eq!(usage["prompt_tokens_details"]["cache_write_tokens"], 6);
        assert_eq!(usage["completion_tokens_details"]["reasoning_tokens"], 6);
        assert_eq!(usage["provider_extra"], 9);
    }

    #[test]
    fn store_matches_only_when_all_pending_results_arrive() {
        let mut store = ContinuationStore::new(4, Duration::from_secs(60));
        let mut cont = Continuation {
            id: String::new(),
            pending: vec!["call_1".into(), "call_2".into()],
            state: state(),
            created: Instant::now(),
        };
        cont.state.phase = Pig::Exec;
        let id = store.insert(cont);
        assert_eq!(store.len(), 1);

        // 工具结果未全部到达，不匹配。 / No match until all tool results arrive.
        assert!(store.take_match(&["call_1".into()]).is_none());
        // 全部工具结果到达后完成匹配。 / Match once all tool results arrive.
        let taken = store
            .take_match(&["call_1".into(), "call_2".into()])
            .unwrap();
        assert_eq!(taken.id, id);
        assert_eq!(taken.state.phase, Pig::Exec);
        assert!(store
            .take_match(&["call_1".into(), "call_2".into()])
            .is_none());
    }

    #[test]
    fn claim_keeps_continuation_until_commit_and_rollback_restores_it() {
        let mut store = ContinuationStore::new(4, Duration::from_secs(60));
        let id = store.insert(Continuation {
            id: String::new(),
            pending: vec!["call_1".into()],
            state: state(),
            created: Instant::now(),
        });

        let claimed = match store.claim_match_for_mode(&["call_1".into()], PigsMode::A) {
            ClaimMatch::Claimed(continuation) => continuation,
            other => panic!("首次应 claim 成功，实际 {other:?}"),
        };
        assert_eq!(claimed.id, id);
        assert_eq!(store.len(), 1, "claim 只占用，不得提前删除现场");
        assert!(matches!(
            store.claim_match_for_mode(&["call_1".into()], PigsMode::A),
            ClaimMatch::InFlight(ref got) if got == &id
        ));

        assert!(store.rollback_claim(&id));
        let claimed_again = match store.claim_match_for_mode(&["call_1".into()], PigsMode::A) {
            ClaimMatch::Claimed(continuation) => continuation,
            other => panic!("rollback 后应允许再次 claim，实际 {other:?}"),
        };
        assert_eq!(claimed_again.id, id);
        assert!(store.commit_claim(&id));
        assert_eq!(store.len(), 0, "只有成功 commit 才真正删除旧现场");
        assert!(matches!(
            store.claim_match_for_mode(&["call_1".into()], PigsMode::A),
            ClaimMatch::NoMatch
        ));
    }

    #[test]
    fn claimed_continuation_is_not_evicted_by_capacity_pressure() {
        let mut store = ContinuationStore::new(1, Duration::from_secs(60));
        let claimed_id = store.insert(Continuation {
            id: String::new(),
            pending: vec!["call_1".into()],
            state: state(),
            created: Instant::now(),
        });
        assert!(matches!(
            store.claim_match_for_mode(&["call_1".into()], PigsMode::A),
            ClaimMatch::Claimed(_)
        ));

        store.insert(Continuation {
            id: String::new(),
            pending: vec!["call_2".into()],
            state: state(),
            created: Instant::now(),
        });
        assert!(matches!(
            store.claim_match_for_mode(&["call_1".into()], PigsMode::A),
            ClaimMatch::InFlight(ref got) if got == &claimed_id
        ));
        assert!(store.rollback_claim(&claimed_id));
    }

    #[test]
    fn store_prefers_newest_matching_continuation() {
        let mut store = ContinuationStore::new(4, Duration::from_secs(60));
        let older = store.insert(Continuation {
            id: String::new(),
            pending: vec!["old".into()],
            state: state(),
            created: Instant::now(),
        });
        let newer = store.insert(Continuation {
            id: String::new(),
            pending: vec!["new".into()],
            state: state(),
            created: Instant::now(),
        });

        let taken = store
            .take_match(&["old".into(), "new".into()])
            .expect("长历史同时包含旧/新结果时应命中最新现场");
        assert_eq!(taken.id, newer);
        assert_ne!(taken.id, older);
    }

    #[test]
    fn store_mode_matching_does_not_consume_other_mode() {
        let mut store = ContinuationStore::new(4, Duration::from_secs(60));
        let mut a = state();
        a.mode = PigsMode::A;
        let mut b = state();
        b.mode = PigsMode::B;
        store.insert(Continuation {
            id: String::new(),
            pending: vec!["same".into()],
            state: a,
            created: Instant::now(),
        });
        store.insert(Continuation {
            id: String::new(),
            pending: vec!["same".into()],
            state: b,
            created: Instant::now(),
        });

        let got_a = store
            .take_match_for_mode(&["same".into()], PigsMode::A)
            .expect("A 应只拿到 A continuation");
        assert_eq!(got_a.state.mode, PigsMode::A);
        let got_b = store
            .take_match_for_mode(&["same".into()], PigsMode::B)
            .expect("B 应只拿到 B continuation");
        assert_eq!(got_b.state.mode, PigsMode::B);
    }

    #[test]
    fn store_is_bounded_and_expires() {
        // TTL 为零则全部立即过期。 / Zero TTL immediately expires every entry.
        let mut store = ContinuationStore::new(4, Duration::ZERO);
        store.insert(Continuation {
            id: String::new(),
            pending: vec!["x".into()],
            state: state(),
            created: Instant::now(),
        });
        assert!(
            store.take_match(&["x".into()]).is_none(),
            "过期条目不许匹配"
        );

        // 容量清理保留最新记录。 / Capacity eviction keeps the newest entries.
        let mut store = ContinuationStore::new(2, Duration::from_secs(60));
        for id in ["a", "b", "c"] {
            store.insert(Continuation {
                id: String::new(),
                pending: vec![id.into()],
                state: state(),
                created: Instant::now(),
            });
        }
        assert_eq!(store.len(), 2, "容量上限必须生效");
        assert!(store.take_match(&["a".into()]).is_none(), "最旧的应被挤掉");
        assert!(store.take_match(&["c".into()]).is_some(), "最新的应还在");
    }

    #[test]
    fn state_records_rounds_and_joins_visible() {
        let mut st = state();
        let first = crate::proto::ModelOutput {
            text: "第一轮\nPIGFAIL".into(),
            ..Default::default()
        };
        st.record_round("第一轮\nPIGFAIL", &first, UsageMode::Max);
        let second = crate::proto::ModelOutput {
            text: "第二轮".into(),
            stop_reason: Some("end_turn".into()),
            usage: Some(json!({"input_tokens": 1})),
            ..Default::default()
        };
        st.record_round("第二轮", &second, UsageMode::Max);

        assert_eq!(st.phase_raw_text(), "第一轮\nPIGFAIL\n\n第二轮");
        assert_eq!(st.final_text(), "第一轮\n\n第二轮");
        assert_eq!(st.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(st.usage.unwrap()["input_tokens"], 1);
    }
}
