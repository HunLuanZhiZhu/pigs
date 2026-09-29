//! 相位状态与"工具调用暂停 → 客户端执行 → 结果回填继续"的句柄。
//!
//! 一只 pig（相位）是一段对话区间：模型要工具 → 客户端执行 → 结果回填 → 模型继续，
//! 直到模型这一轮不再要工具，相位才算产出。跨越这段等待的现场就存在 [`Continuation`] 里。

use crate::lang::Lang;
use pigs_protocol::Part;
use crate::Pig;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

/// 一轮编排的现场（跨暂停保留）。
#[derive(Debug, Clone)]
pub struct TurnState {
    /// 当前相位。
    pub phase: Pig,
    /// 已走过的相位序列（诊断）。
    pub path: Vec<Pig>,
    /// Pre 的分析（Executor 指令要用）。
    pub pre_output: String,
    /// 需要接回对话的相位产物（Post 用；逐条追加，不合并）。
    pub transcript: Vec<String>,
    /// 历次失败路径（Pre 重规划用）。
    pub failure_paths: Vec<String>,
    /// 各段可见文本（最终答复 = 按顺序空行拼接）。
    pub visible: Vec<String>,
    /// 给客户端的内容序列（顺序权威）：文本已去控制标记，其余原生块一律原样。
    pub parts: Vec<Part>,
    /// 本相位各轮的原始文本（含控制标记，用于路由）。
    pub phase_raw: Vec<String>,
    /// Pre 重规划计数。
    pub pre_replans: u32,
    /// Post 无标记重试计数。
    pub post_iterations: u32,
    /// 语言（按用户问题判定）。
    pub lang: Lang,
    /// 会话标识（所有子请求共用）。
    pub session: String,
    /// 跨相位累加的上游 usage（原样对象，数字相加）。
    pub usage: Option<Value>,
    /// 最后一个相位给的停止原因（原样回传）。
    pub stop_reason: Option<String>,
}

impl TurnState {
    pub fn new(lang: Lang, session: String) -> Self {
        Self {
            phase: Pig::Pre,
            path: Vec::new(),
            pre_output: String::new(),
            transcript: Vec::new(),
            failure_paths: Vec::new(),
            visible: Vec::new(),
            parts: Vec::new(),
            phase_raw: Vec::new(),
            pre_replans: 0,
            post_iterations: 0,
            lang,
            session,
            usage: None,
            stop_reason: None,
        }
    }

    /// 记一轮模型输出：文本进 visible、原始文本进 phase_raw、usage 累加、停止原因更新。
    pub fn record_round(&mut self, raw_text: &str, output: &crate::proto::ModelOutput) {
        self.phase_raw.push(raw_text.to_string());
        let visible = crate::markers::strip_markers(raw_text);
        if !visible.is_empty() {
            self.visible.push(visible);
        }
        // 内容序列：文本去标记后进（控制标记绝不许漏给客户端），其余原生块原样
        let has_text = self
            .parts
            .iter()
            .any(|part| matches!(part, Part::Text(text) if !text.is_empty()));
        for part in &output.parts {
            match part {
                Part::Text(text) => {
                    let cleaned = crate::markers::strip_markers(text);
                    if !cleaned.is_empty() {
                        // 相位之间用一个空行分隔（与最终答复的拼接一致）
                        let piece = if has_text {
                            format!("\n\n{cleaned}")
                        } else {
                            cleaned
                        };
                        self.parts.push(Part::Text(piece));
                    }
                }
                other => self.parts.push(other.clone()),
            }
        }
        if let Some(usage) = &output.usage {
            self.usage = Some(match self.usage.take() {
                Some(acc) => add_usage(&acc, usage),
                None => usage.clone(),
            });
        }
        // 以**最后一轮**的值为准：工具暂停那一轮的 tool_calls 不该残留到最终答复
        self.stop_reason = output.stop_reason.clone();
    }

    /// 本相位到目前为止的原始文本（含标记）——路由判定用它。
    pub fn phase_raw_text(&self) -> String {
        self.phase_raw.join("\n\n")
    }

    /// 最终答复：各段可见文本按顺序空行拼接。
    pub fn final_text(&self) -> String {
        self.visible
            .iter()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// 收尾：把现场变成一轮结果。
    pub fn complete(self, ended_with: crate::EndedWith) -> crate::TurnResult {
        crate::TurnResult {
            text: self.final_text(),
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

/// 数值字段递归相加（usage 累加语义；与 legacy 一致）。
pub fn add_usage(target: &Value, source: &Value) -> Value {
    match (target, source) {
        (Value::Object(base), Value::Object(extra)) => {
            let mut merged: Map<String, Value> = base.clone();
            for (key, value) in extra {
                let next = match merged.get(key) {
                    Some(existing) => add_usage(existing, value),
                    None => value.clone(),
                };
                merged.insert(key.clone(), next);
            }
            Value::Object(merged)
        }
        (Value::Number(a), Value::Number(b)) => match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => json!(a + b),
            _ => json!(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0)),
        },
        // 其它类型（字符串/布尔/数组/空对象）以 source 为准
        (_, other) => other.clone(),
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

/// 内存 continuation 存储：容量与 TTL 有界，按"尾部工具结果 id"匹配。
///
/// 匹配规则：某条 continuation 的**全部** pending id 都出现在请求的尾部工具结果里，就算它的恢复请求。
#[derive(Debug)]
pub struct ContinuationStore {
    entries: Vec<Continuation>,
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
        while self.entries.len() > self.max {
            self.entries.remove(0);
        }
        id
    }

    /// 按尾部工具结果 id 找并**取出**匹配的 continuation（取出即独占，避免重复恢复）。
    pub fn take_match(&mut self, result_ids: &[String]) -> Option<Continuation> {
        self.evict_expired();
        let index = self.entries.iter().position(|entry| {
            !entry.pending.is_empty()
                && entry
                    .pending
                    .iter()
                    .all(|id| result_ids.iter().any(|got| got == id))
        })?;
        Some(self.entries.remove(index))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn evict_expired(&mut self) {
        let ttl = self.ttl;
        self.entries.retain(|entry| entry.created.elapsed() < ttl);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn state() -> TurnState {
        TurnState::new(Lang::Zh, "sess".into())
    }

    #[test]
    fn usage_accumulates_numerically_and_recursively() {
        let a = json!({"input_tokens": 10, "output_tokens": 2, "cache": {"read": 5}});
        let b = json!({"input_tokens": 3, "output_tokens": 4, "cache": {"read": 1, "write": 7}});
        let sum = add_usage(&a, &b);
        assert_eq!(sum["input_tokens"], 13);
        assert_eq!(sum["output_tokens"], 6);
        assert_eq!(sum["cache"]["read"], 6);
        assert_eq!(sum["cache"]["write"], 7);
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
        let taken = store.take_match(&["call_1".into(), "call_2".into()]).unwrap();
        assert_eq!(taken.id, id);
        assert_eq!(taken.state.phase, Pig::Executor);
        assert!(store.take_match(&["call_1".into(), "call_2".into()]).is_none());
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
        assert!(store.take_match(&["x".into()]).is_none(), "过期条目不许匹配");

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
        st.record_round("第一轮\nPIGFAIL", &first);
        let second = crate::proto::ModelOutput {
            text: "第二轮".into(),
            stop_reason: Some("end_turn".into()),
            usage: Some(json!({"input_tokens": 1})),
            ..Default::default()
        };
        st.record_round("第二轮", &second);

        assert_eq!(st.phase_raw_text(), "第一轮\nPIGFAIL\n\n第二轮");
        assert_eq!(st.final_text(), "第一轮\n\n第二轮");
        assert_eq!(st.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(st.usage.unwrap()["input_tokens"], 1);
    }
}
