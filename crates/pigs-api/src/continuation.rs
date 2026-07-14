//! Bounded in-memory storage for paused external tool calls.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::orchestration::OrchestrationState;
use crate::phased_phase::Phase;
use crate::protocol::{
    HttpRequestEnvelope, NativeToolCall, NativeToolResultGroup, NativeTranscriptItem, Protocol,
};

/// Capacity and TTL for paused turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationConfig {
    /// Maximum paused turns retained in memory.
    pub capacity: usize,
    /// Time after which a paused turn is invalid.
    pub ttl: Duration,
}

impl Default for ContinuationConfig {
    fn default() -> Self {
        Self {
            capacity: 256,
            ttl: Duration::from_secs(30 * 60),
        }
    }
}

/// Complete state required to resume one paused phase.
#[derive(Debug, Clone)]
pub struct Continuation {
    /// Internal continuation identifier.
    pub id: String,
    /// Original client request semantics without retained authentication headers.
    pub original_request: HttpRequestEnvelope,
    /// Pure orchestration state at the paused phase.
    pub orchestration: OrchestrationState,
    /// Current phase that emitted the tool call.
    pub phase: Phase,
    /// Native phase transcript up to the tool call.
    pub phase_transcript: Vec<NativeTranscriptItem>,
    /// All Executor and Post native items retained for Post review.
    pub review_transcript: Vec<NativeTranscriptItem>,
    /// Visible text accumulated across completed and paused phases.
    pub visible_parts: Vec<String>,
    /// Raw model text accumulated within the phase for final marker routing.
    pub phase_raw_parts: Vec<String>,
    /// Native tool calls still awaiting results.
    pub pending_calls: Vec<NativeToolCall>,
    /// Native usage values accumulated before the pause.
    pub usage_values: Vec<serde_json::Value>,
    /// Native tool-result groups received across partial resume requests.
    pub received_results: Vec<NativeToolResultGroup>,
}

impl Continuation {
    /// Returns all pending native tool-call IDs.
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending_calls
            .iter()
            .map(|call| call.id.clone())
            .collect()
    }
}

#[derive(Debug, Clone)]
struct StoredContinuation {
    continuation: Continuation,
    expires_at: Instant,
}

/// Result of matching one incoming tool-result request.
#[derive(Debug, Clone)]
pub enum Lookup {
    /// All pending IDs were present and the paused turn may resume.
    Ready(Box<Continuation>),
    /// Some parallel results are still absent; the paused turn remains stored.
    Waiting {
        /// Missing pending tool-call IDs.
        missing_ids: Vec<String>,
    },
}

/// Continuation lookup and validation failures.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContinuationError {
    /// No active or recently invalidated continuation owns the supplied IDs.
    #[error("unknown tool-call id(s): {ids:?}")]
    Unknown { ids: Vec<String> },
    /// The continuation expired before its tool results arrived.
    #[error("expired tool-call id(s): {ids:?}")]
    Expired { ids: Vec<String> },
    /// The continuation was removed by the capacity bound.
    #[error("evicted tool-call id(s): {ids:?}")]
    Evicted { ids: Vec<String> },
    /// The continuation was already resumed successfully.
    #[error("already consumed tool-call id(s): {ids:?}")]
    Consumed { ids: Vec<String> },
    /// The incoming protocol differs from the paused request.
    #[error("continuation protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch {
        /// Paused protocol.
        expected: Protocol,
        /// Incoming protocol.
        actual: Protocol,
    },
    /// The incoming real model differs from the paused request.
    #[error("continuation model mismatch: expected {expected}, got {actual}")]
    ModelMismatch {
        /// Paused real model.
        expected: String,
        /// Incoming real model.
        actual: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TombstoneReason {
    Expired,
    Evicted,
    Consumed,
}

/// Bounded, TTL-based continuation store. It never writes to disk.
#[derive(Debug)]
pub struct ContinuationStore {
    config: ContinuationConfig,
    entries: HashMap<String, StoredContinuation>,
    tool_to_continuation: HashMap<String, String>,
    order: VecDeque<String>,
    tombstones: HashMap<String, TombstoneReason>,
    tombstone_order: VecDeque<String>,
}

impl ContinuationStore {
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

    /// Inserts a paused turn and returns its continuation ID.
    pub fn insert(&mut self, continuation: Continuation) -> String {
        self.insert_at(continuation, Instant::now())
    }

    fn insert_at(&mut self, mut continuation: Continuation, now: Instant) -> String {
        self.purge_expired(now);
        if continuation.id.is_empty() {
            continuation.id = uuid::Uuid::new_v4().to_string();
        }
        let id = continuation.id.clone();
        let conflicting: Vec<String> = continuation
            .pending_ids()
            .iter()
            .filter_map(|tool_id| self.tool_to_continuation.get(tool_id))
            .filter(|existing_id| *existing_id != &id)
            .cloned()
            .collect();
        for existing_id in conflicting {
            self.remove_with_reason(&existing_id, Some(TombstoneReason::Evicted));
        }
        while self.entries.len() >= self.config.capacity.max(1) {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.remove_with_reason(&oldest, Some(TombstoneReason::Evicted));
        }

        for tool_id in continuation.pending_ids() {
            self.tombstones.remove(&tool_id);
            self.tombstone_order.retain(|id| id != &tool_id);
            self.tool_to_continuation.insert(tool_id, id.clone());
        }
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

    /// Resolves incoming tool results, consuming the entry only when all IDs are present.
    pub fn take_ready(
        &mut self,
        protocol: Protocol,
        real_model: &str,
        supplied_results: &[NativeToolResultGroup],
    ) -> Result<Lookup, ContinuationError> {
        self.take_ready_at(protocol, real_model, supplied_results, Instant::now())
    }

    fn take_ready_at(
        &mut self,
        protocol: Protocol,
        real_model: &str,
        supplied_results: &[NativeToolResultGroup],
        now: Instant,
    ) -> Result<Lookup, ContinuationError> {
        let supplied_ids: Vec<String> = supplied_results
            .iter()
            .flat_map(|group| group.ids.clone())
            .collect();
        self.purge_expired(now);
        let Some(first_id) = supplied_ids.first() else {
            return Err(ContinuationError::Unknown { ids: Vec::new() });
        };
        let Some(continuation_id) = self.tool_to_continuation.get(first_id).cloned() else {
            return Err(self.missing_error(&supplied_ids));
        };
        if supplied_ids
            .iter()
            .any(|id| self.tool_to_continuation.get(id) != Some(&continuation_id))
        {
            return Err(ContinuationError::Unknown {
                ids: supplied_ids.to_vec(),
            });
        }
        let stored =
            self.entries
                .get_mut(&continuation_id)
                .ok_or_else(|| ContinuationError::Unknown {
                    ids: supplied_ids.to_vec(),
                })?;
        if stored.continuation.original_request.protocol != protocol {
            return Err(ContinuationError::ProtocolMismatch {
                expected: stored.continuation.original_request.protocol,
                actual: protocol,
            });
        }
        if stored.continuation.original_request.real_model != real_model {
            return Err(ContinuationError::ModelMismatch {
                expected: stored.continuation.original_request.real_model.clone(),
                actual: real_model.to_owned(),
            });
        }

        let pending = stored.continuation.pending_ids();
        let already_received: Vec<String> = stored
            .continuation
            .received_results
            .iter()
            .flat_map(|group| group.ids.clone())
            .collect();
        if supplied_ids.iter().all(|id| already_received.contains(id)) {
            return Err(ContinuationError::Consumed {
                ids: supplied_ids.to_vec(),
            });
        }

        let mut merged_results = stored.continuation.received_results.clone();
        for supplied in supplied_results {
            let merged_ids: Vec<String> = merged_results
                .iter()
                .flat_map(|group| group.ids.clone())
                .collect();
            let all_received = supplied.ids.iter().all(|id| merged_ids.contains(id));
            if all_received {
                if merged_results.contains(supplied) {
                    continue;
                }
                return Err(ContinuationError::Consumed {
                    ids: supplied.ids.clone(),
                });
            }
            if supplied.ids.iter().any(|id| merged_ids.contains(id)) {
                merged_results
                    .retain(|existing| !existing.ids.iter().any(|id| supplied.ids.contains(id)));
            }
            merged_results.push(supplied.clone());
        }
        stored.continuation.received_results = merged_results;
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
        if !missing_ids.is_empty() {
            return Ok(Lookup::Waiting { missing_ids });
        }
        let continuation = stored.continuation.clone();
        self.remove_with_reason(&continuation_id, Some(TombstoneReason::Consumed));
        Ok(Lookup::Ready(Box::new(continuation)))
    }

    fn purge_expired(&mut self, now: Instant) {
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

    fn missing_error(&self, ids: &[String]) -> ContinuationError {
        let reason = ids.iter().find_map(|id| self.tombstones.get(id)).copied();
        match reason {
            Some(TombstoneReason::Expired) => ContinuationError::Expired { ids: ids.to_vec() },
            Some(TombstoneReason::Evicted) => ContinuationError::Evicted { ids: ids.to_vec() },
            Some(TombstoneReason::Consumed) => ContinuationError::Consumed { ids: ids.to_vec() },
            None => ContinuationError::Unknown { ids: ids.to_vec() },
        }
    }

    fn remove_with_reason(&mut self, continuation_id: &str, reason: Option<TombstoneReason>) {
        if let Some(entry) = self.entries.remove(continuation_id) {
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
        self.order.retain(|id| id != continuation_id);
    }

    fn insert_tombstone(&mut self, tool_id: String, reason: TombstoneReason) {
        let limit = self.config.capacity.max(1).saturating_mul(8);
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

    fn continuation(id: &str, protocol: Protocol, model: &str, tool_ids: &[&str]) -> Continuation {
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
        let now = Instant::now();
        let mut store = ContinuationStore::new(ContinuationConfig {
            capacity: 1,
            ttl: Duration::from_secs(60),
        });
        store.insert_at(
            continuation("one", Protocol::OpenAiChat, "gpt", &["call_1"]),
            now,
        );
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

        assert!(matches!(
            store
                .take_ready_at(Protocol::OpenAiChat, "gpt", &[result("call_2")], now,)
                .unwrap(),
            Lookup::Ready(_)
        ));
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
        let now = Instant::now();
        let mut store = ContinuationStore::new(ContinuationConfig::default());
        store.insert_at(
            continuation("one", Protocol::OpenAiChat, "gpt", &["call_1", "call_2"]),
            now,
        );
        assert!(matches!(
            store.take_ready_at(
                Protocol::OpenAiChat,
                "gpt",
                &[result("unknown")],
                now,
            ),
            Err(ContinuationError::Unknown { ids }) if ids == vec!["unknown"]
        ));
        assert!(matches!(
            store.take_ready_at(Protocol::AnthropicMessages, "gpt", &[result("call_1")], now,),
            Err(ContinuationError::ProtocolMismatch { .. })
        ));
        assert!(matches!(
            store.take_ready_at(Protocol::OpenAiChat, "other", &[result("call_1")], now,),
            Err(ContinuationError::ModelMismatch { .. })
        ));
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
