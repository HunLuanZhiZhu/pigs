//! 有界内存存储 —— 用于暂停的外部工具调用。
//! Bounded in-memory storage for paused external tool calls.
//!
//! 当相位运行时遇到工具调用（`tool_use`）时，它不能在进程内执行这些
//! 工具（工具由上游 Agent 执行）。运行时把当前轮次的完整状态打包成
//! `Continuation` 存入 `ContinuationStore`，并返回 `ToolPause`。当上游
//! Agent 把工具结果回传过来时，运行时用工具调用 ID 找回对应的
//! continuation，恢复轮次继续执行。
//!
//! When the phase runtime encounters tool calls (`tool_use`), it cannot
//! execute them in-process (tools are executed by the upstream Agent). The
//! runtime packages the turn's complete state into a `Continuation`, stores
//! it in `ContinuationStore`, and returns `ToolPause`. When the upstream
//! Agent returns the tool results, the runtime looks up the continuation by
//! tool-call ID and resumes the turn.
//!
//! 存储是有界的：容量上限（`capacity`）+ TTL（`ttl`）。超出容量时最老的
//! 条目被淘汰（evicted）；TTL 过期后条目失效（expired）；被成功恢复后
//! 条目被消费（consumed）。三种失效原因都有明确的错误类型，便于上游
//! 区分"ID 未知"与"ID 已过期"。
//!
//! The store is bounded: capacity limit (`capacity`) + TTL (`ttl`). When
//! capacity is exceeded the oldest entry is evicted; after TTL an entry is
//! expired; after a successful resume the entry is consumed. All three
//! failure causes have distinct error types so the caller can distinguish
//! "unknown ID" from "expired ID".

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::orchestration::OrchestrationState;
use crate::phased_phase::Phase;
use crate::protocol::{
    HttpRequestEnvelope, NativeToolCall, NativeToolResultGroup, NativeTranscriptItem, Protocol,
};

/// 暂停轮次的容量与 TTL。
/// Capacity and TTL for paused turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationConfig {
    /// 内存中保留的最大暂停轮次数。
    /// Maximum paused turns retained in memory.
    pub capacity: usize,
    /// 暂停轮次的有效期；超过即失效。
    /// Time after which a paused turn is invalid.
    pub ttl: Duration,
}

impl Default for ContinuationConfig {
    /// 默认：容量 256、TTL 30 分钟。
    /// Default: capacity 256, TTL 30 minutes.
    fn default() -> Self {
        Self {
            capacity: 256,
            ttl: Duration::from_secs(30 * 60),
        }
    }
}

/// 恢复一个暂停相位所需的完整状态。
/// Complete state required to resume one paused phase.
#[derive(Debug, Clone)]
pub struct Continuation {
    /// 内部 continuation 标识符（为空时由 store 生成 UUID）。
    /// Internal continuation identifier (store generates a UUID when empty).
    pub id: String,
    /// 原始客户端请求语义（不含保留的鉴权 header）。
    /// Original client request semantics without retained authentication headers.
    pub original_request: HttpRequestEnvelope,
    /// 暂停时的纯编排状态。
    /// Pure orchestration state at the paused phase.
    pub orchestration: OrchestrationState,
    /// 发出工具调用的当前相位。
    /// Current phase that emitted the tool call.
    pub phase: Phase,
    /// 截至工具调用时的原生相位对话记录。
    /// Native phase transcript up to the tool call.
    pub phase_transcript: Vec<NativeTranscriptItem>,
    /// 保留供 Post 审阅的所有 Executor/Post 原生条目。
    /// All Executor and Post native items retained for Post review.
    pub review_transcript: Vec<NativeTranscriptItem>,
    /// 跨已完成相位和暂停相位累积的可见文本。
    /// Visible text accumulated across completed and paused phases.
    pub visible_parts: Vec<String>,
    /// 相位内累积的原始模型文本（用于最终标记路由）。
    /// Raw model text accumulated within the phase for final marker routing.
    pub phase_raw_parts: Vec<String>,
    /// 仍在等待结果的原生工具调用。
    /// Native tool calls still awaiting results.
    pub pending_calls: Vec<NativeToolCall>,
    /// 暂停前累积的原生 usage 值。
    /// Native usage values accumulated before the pause.
    pub usage_values: Vec<serde_json::Value>,
    /// 跨多次部分恢复请求收到的原生工具结果组。
    /// Native tool-result groups received across partial resume requests.
    pub received_results: Vec<NativeToolResultGroup>,
}

impl Continuation {
    /// 返回所有待处理的原生工具调用 ID。
    /// Returns all pending native tool-call IDs.
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending_calls
            .iter()
            .map(|call| call.id.clone())
            .collect()
    }
}

/// 带过期时间的存储条目（内部类型）。
/// A stored entry with its expiry time (internal type).
#[derive(Debug, Clone)]
struct StoredContinuation {
    /// 被存储的 continuation / the stored continuation.
    continuation: Continuation,
    /// 过期时刻 / expiry instant.
    expires_at: Instant,
}

/// 匹配一次传入工具结果请求的结果。
/// Result of matching one incoming tool-result request.
#[derive(Debug, Clone)]
pub enum Lookup {
    /// 所有待处理 ID 都已就位，可以恢复暂停的轮次。
    /// All pending IDs were present and the paused turn may resume.
    Ready(Box<Continuation>),
    /// 仍有并行结果未到；暂停的轮次继续保留在存储中。
    /// Some parallel results are still absent; the paused turn remains stored.
    Waiting {
        /// 缺失的待处理工具调用 ID / missing pending tool-call IDs.
        missing_ids: Vec<String>,
    },
}

/// Continuation 查找与校验失败。
/// Continuation lookup and validation failures.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContinuationError {
    /// 没有活动或最近失效的 continuation 拥有这些 ID。
    /// No active or recently invalidated continuation owns the supplied IDs.
    #[error("unknown tool-call id(s): {ids:?}")]
    Unknown {
        /// 找不到归属的 ID / IDs with no owning continuation.
        ids: Vec<String>,
    },
    /// continuation 在工具结果到达前已过期。
    /// The continuation expired before its tool results arrived.
    #[error("expired tool-call id(s): {ids:?}")]
    Expired {
        /// 已过期的 ID / expired IDs.
        ids: Vec<String>,
    },
    /// continuation 被容量上限淘汰。
    /// The continuation was removed by the capacity bound.
    #[error("evicted tool-call id(s): {ids:?}")]
    Evicted {
        /// 被淘汰的 ID / evicted IDs.
        ids: Vec<String>,
    },
    /// continuation 已被成功恢复过一次。
    /// The continuation was already resumed successfully.
    #[error("already consumed tool-call id(s): {ids:?}")]
    Consumed {
        /// 已消费的 ID / already-consumed IDs.
        ids: Vec<String>,
    },
    /// 传入请求的协议与暂停时的协议不一致。
    /// The incoming protocol differs from the paused request.
    #[error("continuation protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch {
        /// 暂停时的协议 / paused protocol.
        expected: Protocol,
        /// 传入请求的协议 / incoming protocol.
        actual: Protocol,
    },
    /// 传入请求的真实模型与暂停时的真实模型不一致。
    /// The incoming real model differs from the paused request.
    #[error("continuation model mismatch: expected {expected}, got {actual}")]
    ModelMismatch {
        /// 暂停时的真实模型 / paused real model.
        expected: String,
        /// 传入请求的真实模型 / incoming real model.
        actual: String,
    },
}

/// 墓碑原因：标记一个已被移除的 continuation 为什么被移除。
/// Tombstone reason: records why a removed continuation was removed.
///
/// 墓碑让后续到达的工具结果能区分"从未存在"与"已过期/已淘汰/已消费"。
/// Tombstones let later-arriving tool results distinguish "never existed"
/// from "expired/evicted/consumed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TombstoneReason {
    /// TTL 过期 / TTL expired.
    Expired,
    /// 容量淘汰 / evicted by capacity bound.
    Evicted,
    /// 已被成功恢复 / successfully resumed.
    Consumed,
}

/// 有界、基于 TTL 的 continuation 存储。永不写盘。
/// Bounded, TTL-based continuation store. It never writes to disk.
#[derive(Debug)]
pub struct ContinuationStore {
    /// 容量与 TTL 配置 / capacity and TTL config.
    config: ContinuationConfig,
    /// continuation_id → 存储条目 / continuation_id → stored entry.
    entries: HashMap<String, StoredContinuation>,
    /// tool_call_id → continuation_id 的反查索引 / tool_call_id → continuation_id reverse index.
    tool_to_continuation: HashMap<String, String>,
    /// 按插入顺序排列的 continuation_id（用于 FIFO 淘汰）/ continuation_ids in insertion order (for FIFO eviction).
    order: VecDeque<String>,
    /// tool_call_id → 墓碑原因 / tool_call_id → tombstone reason.
    tombstones: HashMap<String, TombstoneReason>,
    /// 墓碑的插入顺序（用于 FIFO 淘汰墓碑）/ tombstone insertion order (for FIFO eviction).
    tombstone_order: VecDeque<String>,
}

impl ContinuationStore {
    /// 创建一个空存储。
    /// Creates an empty store.
    pub fn new(config: ContinuationConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            tool_to_continuation: HashMap::new(),
            order: VecDeque::new(),
            tombstones: HashMap::new(),
            tombstone_order: VecDeque::new(),
        }
    }

    /// 插入一个暂停轮次并返回其 continuation ID。
    /// Inserts a paused turn and returns its continuation ID.
    pub fn insert(&mut self, continuation: Continuation) -> String {
        self.insert_at(continuation, Instant::now())
    }

    /// 内部：用可注入的"当前时刻"插入（便于测试控制时钟）。
    /// Internal: insert with an injectable "now" (for testable clocks).
    fn insert_at(&mut self, mut continuation: Continuation, now: Instant) -> String {
        // 先清理过期条目 / Purge expired entries first.
        self.purge_expired(now);
        // 若 id 为空则生成 UUID / Generate a UUID if id is empty.
        if continuation.id.is_empty() {
            continuation.id = uuid::Uuid::new_v4().to_string();
        }
        let id = continuation.id.clone();
        // 收集与已有 continuation 冲突的工具 ID（同一 tool_id 已属于别的 continuation）
        // Collect tool IDs that conflict with an existing continuation.
        let conflicting: Vec<String> = continuation
            .pending_ids()
            .iter()
            .filter_map(|tool_id| self.tool_to_continuation.get(tool_id))
            .filter(|existing_id| *existing_id != &id)
            .cloned()
            .collect();
        // 冲突的旧 continuation 被淘汰 / Evict conflicting older continuations.
        for existing_id in conflicting {
            self.remove_with_reason(&existing_id, Some(TombstoneReason::Evicted));
        }
        // 容量已满时按 FIFO 淘汰最老的 / Evict oldest (FIFO) when at capacity.
        while self.entries.len() >= self.config.capacity.max(1) {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.remove_with_reason(&oldest, Some(TombstoneReason::Evicted));
        }

        // 为本 continuation 的每个工具 ID 建立反查索引 / Build reverse index for each tool ID.
        for tool_id in continuation.pending_ids() {
            // 清除可能存在的旧墓碑 / Clear any stale tombstone for this tool ID.
            self.tombstones.remove(&tool_id);
            self.tombstone_order.retain(|tid| tid != &tool_id);
            self.tool_to_continuation.insert(tool_id, id.clone());
        }
        // 追加到顺序队列并插入条目 / Append to order queue and insert entry.
        self.order.push_back(id.clone());
        self.entries.insert(
            id.clone(),
            StoredContinuation {
                continuation,
                expires_at: now + self.config.ttl,
            },
        );
        id
    }

    /// 解析传入的工具结果，仅当所有 ID 都就位时才消费条目。
    ///
    /// 匹配规则：
    /// 1. 第一个 supplied_id 定位 continuation；
    /// 2. 所有 supplied_id 必须属于同一 continuation；
    /// 3. 协议与真实模型必须匹配；
    /// 4. 合并本次与之前收到的结果，若仍有缺失 → `Waiting`；
    /// 5. 全部就位 → 取出 continuation 并标记为 Consumed。
    ///
    /// Resolves incoming tool results, consuming the entry only when all IDs are present.
    pub fn take_ready(
        &mut self,
        protocol: Protocol,
        real_model: &str,
        supplied_results: &[NativeToolResultGroup],
    ) -> Result<Lookup, ContinuationError> {
        self.take_ready_at(protocol, real_model, supplied_results, Instant::now())
    }

    /// 内部：带可注入时钟的 `take_ready`。
    /// Internal: `take_ready` with an injectable clock.
    fn take_ready_at(
        &mut self,
        protocol: Protocol,
        real_model: &str,
        supplied_results: &[NativeToolResultGroup],
        now: Instant,
    ) -> Result<Lookup, ContinuationError> {
        // 收集本次请求提供的所有工具 ID / Collect all supplied tool IDs.
        let supplied_ids: Vec<String> = supplied_results
            .iter()
            .flat_map(|group| group.ids.clone())
            .collect();
        // 先清理过期条目 / Purge expired entries first.
        self.purge_expired(now);
        // 没有任何 supplied ID → 未知错误 / No supplied IDs → unknown error.
        let Some(first_id) = supplied_ids.first() else {
            return Err(ContinuationError::Unknown { ids: Vec::new() });
        };
        // 用第一个 ID 定位 continuation / Locate continuation by the first ID.
        let Some(continuation_id) = self.tool_to_continuation.get(first_id).cloned() else {
            return Err(self.missing_error(&supplied_ids));
        };
        // 所有 supplied ID 必须属于同一 continuation / All supplied IDs must share one continuation.
        if supplied_ids
            .iter()
            .any(|id| self.tool_to_continuation.get(id) != Some(&continuation_id))
        {
            return Err(ContinuationError::Unknown {
                ids: supplied_ids.to_vec(),
            });
        }
        // 取出存储条目 / Retrieve the stored entry.
        let stored =
            self.entries
                .get_mut(&continuation_id)
                .ok_or_else(|| ContinuationError::Unknown {
                    ids: supplied_ids.to_vec(),
                })?;
        // 协议必须匹配 / Protocol must match.
        if stored.continuation.original_request.protocol != protocol {
            return Err(ContinuationError::ProtocolMismatch {
                expected: stored.continuation.original_request.protocol,
                actual: protocol,
            });
        }
        // 真实模型必须匹配 / Real model must match.
        if stored.continuation.original_request.real_model != real_model {
            return Err(ContinuationError::ModelMismatch {
                expected: stored.continuation.original_request.real_model.clone(),
                actual: real_model.to_owned(),
            });
        }

        // 待处理 ID 与已收到 ID / Pending IDs and already-received IDs.
        let pending = stored.continuation.pending_ids();
        let already_received: Vec<String> = stored
            .continuation
            .received_results
            .iter()
            .flat_map(|group| group.ids.clone())
            .collect();
        // 本次提供的 ID 全部已收到 → 已消费错误 / All supplied IDs already received → consumed error.
        if supplied_ids.iter().all(|id| already_received.contains(id)) {
            return Err(ContinuationError::Consumed {
                ids: supplied_ids.to_vec(),
            });
        }

        // 合并本次与之前收到的结果 / Merge supplied results with previously received ones.
        let mut merged_results = stored.continuation.received_results.clone();
        for supplied in supplied_results {
            let merged_ids: Vec<String> = merged_results
                .iter()
                .flat_map(|group| group.ids.clone())
                .collect();
            // 本组 ID 是否已全部在合并集合中 / Are all of this group's IDs already merged?
            let all_received = supplied.ids.iter().all(|id| merged_ids.contains(id));
            if all_received {
                // 完全重复且值相同 → 跳过；值不同 → 已消费错误
                // Exact duplicate → skip; mismatched value → consumed error.
                if merged_results.contains(supplied) {
                    continue;
                }
                return Err(ContinuationError::Consumed {
                    ids: supplied.ids.clone(),
                });
            }
            // 部分 ID 已存在 → 用新值替换旧组 / Partial overlap → replace old group with new value.
            if supplied.ids.iter().any(|id| merged_ids.contains(id)) {
                merged_results
                    .retain(|existing| !existing.ids.iter().any(|id| supplied.ids.contains(id)));
            }
            merged_results.push(supplied.clone());
        }
        // 写回合并后的结果 / Write back merged results.
        stored.continuation.received_results = merged_results;
        // 重新计算已收到与缺失 / Recompute received vs missing.
        let received: Vec<String> = stored
            .continuation
            .received_results
            .iter()
            .flat_map(|group| group.ids.clone())
            .collect();
        let missing_ids: Vec<String> = pending
            .iter()
            .filter(|id| !received.contains(id))
            .cloned()
            .collect();
        // 仍有缺失 → 等待 / Still missing → waiting.
        if !missing_ids.is_empty() {
            return Ok(Lookup::Waiting { missing_ids });
        }
        // 全部就位 → 取出 continuation 并标记为已消费 / All present → take and mark consumed.
        let continuation = stored.continuation.clone();
        self.remove_with_reason(&continuation_id, Some(TombstoneReason::Consumed));
        Ok(Lookup::Ready(Box::new(continuation)))
    }

    /// 清理所有已过期的条目。
    /// Remove all expired entries.
    fn purge_expired(&mut self, now: Instant) {
        // 收集过期 continuation_id / Collect expired continuation IDs.
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.remove_with_reason(&id, Some(TombstoneReason::Expired));
        }
    }

    /// 为一组找不到归属的 ID 生成错误，优先用墓碑解释原因。
    /// Produce an error for IDs with no owner, preferring tombstone reasons.
    fn missing_error(&self, ids: &[String]) -> ContinuationError {
        // 查找第一个有墓碑记录的 ID / Find the first ID with a tombstone.
        let reason = ids.iter().find_map(|id| self.tombstones.get(id)).copied();
        match reason {
            Some(TombstoneReason::Expired) => ContinuationError::Expired { ids: ids.to_vec() },
            Some(TombstoneReason::Evicted) => ContinuationError::Evicted { ids: ids.to_vec() },
            Some(TombstoneReason::Consumed) => ContinuationError::Consumed { ids: ids.to_vec() },
            // 没有墓碑 → 真正从未存在 / No tombstone → truly never existed.
            None => ContinuationError::Unknown { ids: ids.to_vec() },
        }
    }

    /// 移除一个 continuation，并为其工具 ID 建立墓碑（若给定原因）。
    /// Remove a continuation, optionally tombstoning its tool IDs.
    fn remove_with_reason(&mut self, continuation_id: &str, reason: Option<TombstoneReason>) {
        if let Some(entry) = self.entries.remove(continuation_id) {
            // 清理反查索引并为每个工具 ID 建墓碑 / Clear reverse index and tombstone each tool ID.
            for tool_id in entry.continuation.pending_ids() {
                if self.tool_to_continuation.get(&tool_id).map(String::as_str)
                    == Some(continuation_id)
                {
                    self.tool_to_continuation.remove(&tool_id);
                    if let Some(reason) = reason {
                        self.insert_tombstone(tool_id, reason);
                    }
                }
            }
        }
        // 从顺序队列中移除 / Remove from the order queue.
        self.order.retain(|id| id != continuation_id);
    }

    /// 插入一条墓碑，墓碑数量也有上限（容量的 8 倍）。
    /// Insert a tombstone; tombstones are also bounded (8× capacity).
    fn insert_tombstone(&mut self, tool_id: String, reason: TombstoneReason) {
        // 墓碑上限 = max(capacity,1) * 8 / Tombstone limit = max(capacity,1) * 8.
        let limit = self.config.capacity.max(1).saturating_mul(8);
        // 超限时按 FIFO 淘汰最老墓碑 / Evict oldest tombstone (FIFO) when over limit.
        while self.tombstones.len() >= limit {
            let Some(oldest) = self.tombstone_order.pop_front() else {
                break;
            };
            self.tombstones.remove(&oldest);
        }
        self.tombstone_order.push_back(tool_id.clone());
        self.tombstones.insert(tool_id, reason);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::orchestration::OrchestrationLimits;
    use crate::protocol::{NativeToolCall, ProtocolCodec};
    use serde_json::json;

    /// 测试辅助：构造一个带工具调用的 continuation。
    /// Test helper: build a continuation with tool calls.
    fn continuation(id: &str, protocol: Protocol, model: &str, tool_ids: &[&str]) -> Continuation {
        // 根据协议构造最小请求体 / Build a minimal request body per protocol.
        let body = match protocol {
            Protocol::OpenAiChat | Protocol::AnthropicMessages => json!({
                "model": format!("{model}-pig"),
                "messages": [{"role": "user", "content": "task"}]
            }),
            Protocol::OpenAiResponses => json!({
                "model": format!("{model}-pig"),
                "input": "task"
            }),
        };
        // 解析为 HttpRequestEnvelope / Parse into an HttpRequestEnvelope.
        let request = ProtocolCodec::new(protocol)
            .parse_request("POST", "/test", Vec::new(), body)
            .unwrap();
        Continuation {
            id: id.into(),
            original_request: request,
            orchestration: OrchestrationState::new(OrchestrationLimits::default()),
            phase: Phase::Executor,
            phase_transcript: Vec::new(),
            review_transcript: Vec::new(),
            visible_parts: Vec::new(),
            phase_raw_parts: Vec::new(),
            pending_calls: tool_ids
                .iter()
                .map(|tool_id| NativeToolCall {
                    id: (*tool_id).into(),
                    name: "run".into(),
                    arguments: json!({}),
                    native: json!({"id": tool_id}),
                })
                .collect(),
            usage_values: Vec::new(),
            received_results: Vec::new(),
        }
    }

    /// 测试辅助：构造一个单 ID 的工具结果组。
    /// Test helper: build a single-ID tool result group.
    fn result(id: &str) -> NativeToolResultGroup {
        NativeToolResultGroup {
            ids: vec![id.into()],
            item: NativeTranscriptItem::new(
                Protocol::OpenAiChat,
                crate::protocol::NativeTranscriptKind::ToolResult,
                json!({"role": "tool", "tool_call_id": id, "content": "done"}),
            ),
        }
    }

    #[test]
    fn expires_with_a_distinct_error_using_a_controlled_clock() {
        // TTL 过期应返回 Expired 而非 Unknown / Expired TTL should yield Expired, not Unknown.
        let now = Instant::now();
        let mut store = ContinuationStore::new(ContinuationConfig {
            capacity: 2,
            ttl: Duration::from_secs(5),
        });
        store.insert_at(
            continuation("one", Protocol::OpenAiChat, "gpt", &["call_1"]),
            now,
        );

        assert!(matches!(
            store.take_ready_at(
                Protocol::OpenAiChat,
                "gpt",
                &[result("call_1")],
                now + Duration::from_secs(6),
            ),
            Err(ContinuationError::Expired { ids }) if ids == vec!["call_1"]
        ));
    }

    #[test]
    fn capacity_eviction_and_consumption_are_distinguishable() {
        // 容量淘汰、正常消费、重复消费应分别返回不同错误
        // Eviction, consumption, and re-consumption should yield distinct errors.
        let now = Instant::now();
        let mut store = ContinuationStore::new(ContinuationConfig {
            capacity: 1,
            ttl: Duration::from_secs(60),
        });
        store.insert_at(
            continuation("one", Protocol::OpenAiChat, "gpt", &["call_1"]),
            now,
        );
        // 插入第二条会淘汰第一条 / Inserting the second evicts the first.
        store.insert_at(
            continuation("two", Protocol::OpenAiChat, "gpt", &["call_2"]),
            now,
        );
        assert!(matches!(
            store.take_ready_at(
                Protocol::OpenAiChat,
                "gpt",
                &[result("call_1")],
                now,
            ),
            Err(ContinuationError::Evicted { ids }) if ids == vec!["call_1"]
        ));

        // 第二条可正常恢复 / The second can be resumed.
        assert!(matches!(
            store
                .take_ready_at(Protocol::OpenAiChat, "gpt", &[result("call_2")], now,)
                .unwrap(),
            Lookup::Ready(_)
        ));
        // 重复恢复同一条 → 已消费错误 / Resuming the same one again → consumed error.
        assert!(matches!(
            store.take_ready_at(
                Protocol::OpenAiChat,
                "gpt",
                &[result("call_2")],
                now,
            ),
            Err(ContinuationError::Consumed { ids }) if ids == vec!["call_2"]
        ));
    }

    #[test]
    fn duplicate_tool_ids_evict_the_old_owner_without_removing_the_new_mapping() {
        // 同一 tool_id 的两个 continuation：旧的被淘汰，新的保持可恢复
        // Two continuations sharing a tool_id: the old is evicted, the new stays ready.
        let now = Instant::now();
        let mut store = ContinuationStore::new(ContinuationConfig {
            capacity: 2,
            ttl: Duration::from_secs(60),
        });
        store.insert_at(
            continuation("old", Protocol::OpenAiChat, "gpt", &["same"]),
            now,
        );
        store.insert_at(
            continuation("new", Protocol::OpenAiChat, "gpt", &["same"]),
            now,
        );

        let ready = store
            .take_ready_at(Protocol::OpenAiChat, "gpt", &[result("same")], now)
            .unwrap();
        let Lookup::Ready(continuation) = ready else {
            panic!("new owner should remain ready");
        };
        assert_eq!(continuation.id, "new");
    }

    #[test]
    fn unknown_protocol_model_and_partial_parallel_results_are_explicit() {
        // 未知 ID、协议不匹配、模型不匹配、部分并行结果都应有明确错误
        // Unknown IDs, protocol mismatch, model mismatch, and partial parallel
        // results should all produce explicit errors.
        let now = Instant::now();
        let mut store = ContinuationStore::new(ContinuationConfig::default());
        store.insert_at(
            continuation("one", Protocol::OpenAiChat, "gpt", &["call_1", "call_2"]),
            now,
        );
        // 未知 ID / unknown ID.
        assert!(matches!(
            store.take_ready_at(
                Protocol::OpenAiChat,
                "gpt",
                &[result("unknown")],
                now,
            ),
            Err(ContinuationError::Unknown { ids }) if ids == vec!["unknown"]
        ));
        // 协议不匹配 / protocol mismatch.
        assert!(matches!(
            store.take_ready_at(Protocol::AnthropicMessages, "gpt", &[result("call_1")], now,),
            Err(ContinuationError::ProtocolMismatch { .. })
        ));
        // 模型不匹配 / model mismatch.
        assert!(matches!(
            store.take_ready_at(Protocol::OpenAiChat, "other", &[result("call_1")], now,),
            Err(ContinuationError::ModelMismatch { .. })
        ));
        // 只到了一个并行结果 → 等待另一个 / only one parallel result arrived → waiting.
        assert!(matches!(
            store
                .take_ready_at(
                    Protocol::OpenAiChat,
                    "gpt",
                    &[result("call_1")],
                    now,
                )
                .unwrap(),
            Lookup::Waiting { missing_ids } if missing_ids == vec!["call_2"]
        ));
        // 两个结果都到 → 就绪 / both results arrived → ready.
        assert!(matches!(
            store
                .take_ready_at(
                    Protocol::OpenAiChat,
                    "gpt",
                    &[result("call_1"), result("call_2")],
                    now,
                )
                .unwrap(),
            Lookup::Ready(_)
        ));
    }
}
