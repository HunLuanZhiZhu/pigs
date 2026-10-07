//! 相位状态与"工具调用暂停 → 客户端执行 → 结果回填继续"的句柄。
//!
//! 一只 pig（相位）是一段对话区间：模型要工具 → 客户端执行 → 结果回填 → 模型继续，
//! 直到模型这一轮不再要工具，相位才算产出。跨越这段等待的现场就存在 [`Continuation`] 里。

use crate::lang::Lang;
use crate::{Pig, UsageMode};
use pigs_protocol::{Part, PigsMode};
use serde_json::Value;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 一轮编排的现场（跨暂停保留）。
#[derive(Debug, Clone)]
pub struct TurnState {
    /// 当前相位。
    pub phase: Pig,
    /// 本轮输出模式。
    pub mode: PigsMode,
    /// 本轮最初的客户端请求 body。Pre / Executor 从这里构造；Post 直接继承 Executor 完整上下文。
    pub root_body: Value,
    /// 当前 pig 第一次进入时构造好的基础请求（已注入一次阶段提示）。
    pub phase_base_body: Option<Value>,
    /// 当前 pig 内已经发生过的协议原生 assistant/tool 对话记录。
    pub phase_transcript: Vec<Value>,
    /// 最近一次 Executor 完成后的完整上下文截面（尚未追加 Post 指令）。
    /// Post 返回 PIGNEXT 时从这里恢复，再用 Post 反馈作为新的 Executor 分析输入。
    pub executor_checkpoint_body: Option<Value>,
    /// 已走过的相位序列（诊断）。
    pub path: Vec<Pig>,
    /// Pre 的分析（Executor 指令要用）。
    pub pre_output: String,
    /// 历次失败路径（Pre 重规划用）。
    pub failure_paths: Vec<String>,
    /// 模式 A 的各段可见文本；模式 B 只在最终提交时写入一段。
    pub visible: Vec<String>,
    /// 模式 B：当前 Executor 候选正文。Post 未接受前不对客户端可见。
    pub candidate_text: String,
    /// 模式 B：已经提交的最终业务正文。
    pub committed_text: String,
    /// 持久内容序列（顺序权威）：文本已去控制标记；已交给客户端执行的 ToolCall 不持久化。
    pub parts: Vec<Part>,
    /// 本相位各轮的原始文本（含控制标记，用于路由）。
    pub phase_raw: Vec<String>,
    /// Executor 高层执行次数；同一次 Executor 内的工具暂停/恢复不重复计数。
    pub executor_runs: u32,
    /// Post 无控制标记时的协议重试次数；它不是任务执行预算。
    pub post_protocol_retries: u32,
    /// 语言（按用户问题判定）。
    pub lang: Lang,
    /// 会话标识（所有子请求共用）。
    pub session: String,
    /// 本次客户端 API 请求按 usage 模式聚合后的 usage。
    pub usage: Option<Value>,
    /// 最后一个相位给的停止原因（原样回传）。
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
            root_body,
            phase_base_body: None,
            phase_transcript: Vec::new(),
            executor_checkpoint_body: None,
            path: Vec::new(),
            pre_output: String::new(),
            failure_paths: Vec::new(),
            visible: Vec::new(),
            candidate_text: String::new(),
            committed_text: String::new(),
            parts: Vec::new(),
            phase_raw: Vec::new(),
            executor_runs: 0,
            post_protocol_retries: 0,
            lang,
            session,
            usage: None,
            stop_reason: None,
        }
    }

    /// 清掉当前 pig 的临时对话现场。进入 Pre / Executor 新阶段或 Pre 重规划时调用。
    pub fn reset_phase_conversation(&mut self) {
        self.phase_base_body = None;
        self.phase_transcript.clear();
    }

    /// 把当前模型输出整理成给客户端看的内容序列。
    ///
    /// 这里会保留 ToolCall；调用方只在 Paused 响应里使用它们。
    pub(crate) fn client_parts_for_round(&self, output: &crate::proto::ModelOutput) -> Vec<Part> {
        let has_text = self
            .parts
            .iter()
            .any(|part| matches!(part, Part::Text(text) if !text.is_empty()));
        let mut parts = Vec::new();
        for part in &output.parts {
            match part {
                Part::Text(text) => {
                    if self.mode == PigsMode::A {
                        let cleaned = crate::markers::strip_markers(text);
                        if !cleaned.is_empty() {
                            // 相位之间用一个空行分隔（与最终答复的拼接一致）
                            let piece = if has_text {
                                format!("\n\n{cleaned}")
                            } else {
                                cleaned
                            };
                            parts.push(Part::Text(piece));
                        }
                    }
                }
                other => parts.push(other.clone()),
            }
        }
        parts
    }

    /// 记一轮模型输出：文本/思考等持久化；ToolCall 只属于暂停响应，不进入 continuation / 最终 Completed。
    pub fn record_round(
        &mut self,
        raw_text: &str,
        output: &crate::proto::ModelOutput,
        usage_mode: UsageMode,
    ) {
        self.phase_raw.push(raw_text.to_string());
        let visible = crate::markers::strip_markers(raw_text);
        if self.mode == PigsMode::A && !visible.is_empty() {
            self.visible.push(visible);
        }
        // ToolCall 是一次性的暂停信号：客户端执行后就消费掉，不能在最终响应里再次出现。
        self.parts.extend(
            self.client_parts_for_round(output)
                .into_iter()
                .filter(|part| !matches!(part, Part::ToolCall(_))),
        );
        if let Some(usage) = &output.usage {
            match usage_mode {
                UsageMode::Max => {
                    // 只比较一次真实调用的总 token；胜出的 usage 整对象原样返回，不拼字段。
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
        // 以**最后一轮**的值为准：工具暂停那一轮的 tool_calls 不该残留到最终答复
        self.stop_reason = output.stop_reason.clone();
    }

    /// 记录当前 Executor 业务正文；模式 B 把它作为待验收 candidate。
    pub fn set_candidate(&mut self, text: String) {
        if self.mode == PigsMode::B {
            self.candidate_text = text;
        }
    }

    /// 模式 B：丢弃当前 Executor 候选。
    pub fn discard_candidate(&mut self) {
        if self.mode == PigsMode::B {
            self.candidate_text.clear();
        }
    }

    /// 模式 B：提交一段最终业务正文。
    pub fn commit_text(&mut self, text: String) {
        if self.mode == PigsMode::B {
            self.committed_text = text.clone();
            self.visible.clear();
            if !text.is_empty() {
                self.visible.push(text);
            }
        }
    }

    /// 模式 B：接受当前 Executor candidate。
    pub fn commit_candidate(&mut self) {
        if self.mode == PigsMode::B {
            let text = std::mem::take(&mut self.candidate_text);
            self.commit_text(text);
        }
    }

    /// 本相位到目前为止的原始文本（含标记）——路由判定用它。
    pub fn phase_raw_text(&self) -> String {
        self.phase_raw.join("\n\n")
    }

    /// 最终答复：各段可见文本按顺序空行拼接。
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

    /// 收尾：把现场变成一轮结果。
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

/// 一次真实上游调用的总 token。优先使用协议直接给出的 total_tokens；
/// 缺失时只用主输入/输出字段回退计算，缓存与 reasoning 详情都是其子集，不重复相加。
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

/// sum 模式：同路径的数值字段逐项相加；对象递归合并；只在某次调用出现的字段原样保留。
/// usage 正常只有数值/对象字段；若遇到同路径的非数值元数据，保留第一份，避免伪造语义。
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

/// 一条 continuation：等着客户端把工具结果送回来的相位现场。
#[derive(Debug, Clone)]
pub struct Continuation {
    /// 恢复时放回存储用的 id（诊断/日志）。
    pub id: String,
    /// 正在等待结果的工具调用 id。
    pub pending: Vec<String>,
    /// 相位现场。
    pub state: TurnState,
    /// 创建时刻（TTL 用）。
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

/// continuation claim 的匹配结果。
#[derive(Debug, Clone)]
pub enum ClaimMatch {
    /// 已原子占用该 continuation；调用方必须在 resume 结束后 commit 或 rollback。
    Claimed(Continuation),
    /// 最新匹配现场已被另一个 resume 请求占用，不能并发恢复同一现场。
    InFlight(String),
    /// 没有任何匹配现场。
    NoMatch,
}

/// 内存 continuation 存储：容量与 TTL 有界，按客户端请求中出现的工具结果 id 匹配；真实 proxy 恢复还限定 A/B 模式一致。
///
/// 匹配规则：某条 continuation 的**全部** pending id 都出现在当前请求的工具结果里，
/// 就算它的恢复请求。若历史长对话同时命中多条遗留现场，优先取最新的一条。
#[derive(Debug)]
pub struct ContinuationStore {
    entries: Vec<Continuation>,
    /// 正在被某个 resume 请求占用、但尚未成功消费的 continuation id。
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

    /// 存入一条 continuation，返回它的 id（同时清理过期项与超容量的最旧项）。
    pub fn insert(&mut self, mut continuation: Continuation) -> String {
        self.evict_expired();
        self.counter += 1;
        let id = format!("pigc-{}", self.counter);
        continuation.id = id.clone();
        continuation.created = Instant::now();
        self.entries.push(continuation);
        // 正在恢复中的现场绝不能因为另一个新 continuation 插入而被容量淘汰。
        // 若所有旧现场都处于 in-flight，允许短暂超过 max；resume 收尾后会恢复到正常容量。
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

    /// 原子占用匹配的 continuation，但不删除。resume 成功后调用 [`commit_claim`]，
    /// 失败则调用 [`rollback_claim`]，避免瞬时上游错误把现场永久销毁。
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

    /// resume 成功：正式消费已 claim 的旧 continuation。
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

    /// resume 失败：仅解除占用，原 continuation 保留，允许相同工具结果再次恢复。
    pub fn rollback_claim(&mut self, id: &str) -> bool {
        self.in_flight.remove(id)
    }

    /// 测试/低层调用兼容接口：claim 后立即 commit，保持旧的 destructive-take 语义。
    pub fn take_match(&mut self, result_ids: &[String]) -> Option<Continuation> {
        match self.claim_match_inner(result_ids, None) {
            ClaimMatch::Claimed(continuation) => {
                self.commit_claim(&continuation.id);
                Some(continuation)
            }
            ClaimMatch::InFlight(_) | ClaimMatch::NoMatch => None,
        }
    }

    /// 测试/低层调用兼容接口；proxy 的真实恢复路径必须使用 claim/commit/rollback。
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

    /// 诊断用快照：不会消费 continuation；先清理已过期项再返回当前等待现场。
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

    /// 构造一个只带 usage 的模型输出（其余字段空）。
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
        // max 只按 total_tokens 选一次真实调用，胜出的整个 JSON 原样返回。
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
        cont.state.phase = Pig::Executor;
        let id = store.insert(cont);
        assert_eq!(store.len(), 1);

        // 只回来一个结果 → 不匹配（还要等另一个）
        assert!(store.take_match(&["call_1".into()]).is_none());
        // 两个都回来 → 匹配并被取出
        let taken = store
            .take_match(&["call_1".into(), "call_2".into()])
            .unwrap();
        assert_eq!(taken.id, id);
        assert_eq!(taken.state.phase, Pig::Executor);
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
        // TTL 为 0 → 任何条目都立刻过期：匹配不到
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

        // 容量上限：只保留最新的 max 条
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
