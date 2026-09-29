//! pigs-orchestrator —— 编排引擎。
//!
//! 三只 pig 组成一头 pigs：Pre（规划/分流）→ Executor（执行）→ Post（验收），
//! 由 PIGEND / PIGFAIL 控制标记与预算驱动的状态机串起来。
//!
//! 红线：不认识 axum（网络走 [`Transport`] 注入）、不做重试、不知道上游是谁。

pub mod lang;
pub mod markers;
pub mod prompts;
pub mod transport;

use markers::{detect_marker, strip_markers, Marker};
use pigs_protocol as proto;
use transport::{SubRequest, SubResponse, Transport, TransportError};

use serde_json::Value;
use std::sync::Arc;

/// 会话头名：编排产生的稳定会话标识，所有子请求共用（mini-proxy 见已带就不覆盖）。
pub const SESSION_HEADER: &str = "x-opencode-session";

/// 回环内部令牌头（proxy 验证后跳过 -pig 分流，防递归）。
pub const LOOPBACK_TOKEN_HEADER: &str = "x-pigs-loopback";

/// 预算常量（legacy 默认值；刻意不进配置——它们是编排语义的一部分）。
const MAX_PRE_REPLANS: u32 = 2;
const MAX_EXECUTOR_LOOPS: u32 = 3;

/// 单只 pig（一个相位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pig {
    /// 规划 / 分流 / GOAL 声明。
    Pre,
    /// 信息收集 + 起草答复。
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

/// 编排输入：proxy 已解析好的现场。
#[derive(Debug, Clone)]
pub struct TurnInput {
    /// 协议（由请求路径判定）。
    pub protocol: proto::Protocol,
    /// 原始请求 body（JSON）。model 字段已被 proxy 剥掉 `-pig` 后缀。
    pub body: Value,
    /// 协议路径（子请求原样使用）。
    pub path: String,
    /// 需要随行的客户端端到端头（鉴权等，通常 2~4 个）。
    pub base_headers: Vec<(String, String)>,
    /// 客户端自带的会话头值（没有则为 None，编排生成）。
    pub client_session: Option<String>,
}

/// 编排结果。
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// 最终答复文本（已剥离控制标记）。
    pub text: String,
    /// 结束方式（诊断/日志用）。
    pub ended_with: EndedWith,
    /// 实际使用的会话头值。
    pub session: String,
    /// 完整走过的 pig 序列（诊断用）。
    pub path: Vec<Pig>,
}

/// 结束方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedWith {
    /// Pre 判定为简单问题，直接作答。
    SimplePath,
    /// Post 验收通过。
    PigEnd,
    /// 重规划预算耗尽。
    PigFailBudget,
    /// Executor 回环预算耗尽。
    ExecutorLoopBudget,
}

impl EndedWith {
    pub fn as_str(self) -> &'static str {
        match self {
            EndedWith::SimplePath => "SIMPLE_PATH",
            EndedWith::PigEnd => "PIGEND",
            EndedWith::PigFailBudget => "PIGFAIL_BUDGET",
            EndedWith::ExecutorLoopBudget => "EXECUTOR_LOOP_BUDGET",
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
    #[error("上游响应无法提取文本（协议 {0:?}，pig {1}）")]
    NoText(proto::Protocol, &'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 编排器。无状态，可全局共享。
#[derive(Debug, Clone, Default)]
pub struct Orchestrator;

impl Orchestrator {
    pub fn new() -> Self {
        Self
    }

    /// 跑完一整轮三 pig 编排，返回最终文本。
    pub async fn run(&self, input: TurnInput, transport: Arc<dyn Transport>) -> Result<TurnResult> {
        let session = input
            .client_session
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let lang = lang::detect_lang(&proto::extract_last_user_text(&input.body, input.protocol));
        let user_question = proto::extract_last_user_text(&input.body, input.protocol);

        let ctx = Ctx {
            input,
            session,
            lang,
            user_question,
            transport,
        };

        let mut failure_paths: Vec<String> = Vec::new();
        let mut pre_output = String::new();
        let mut executor_draft = String::new();
        let mut last_post_feedback = String::new();
        let mut pre_replans: u32 = 0;
        let mut executor_loops: u32 = 0;
        let mut path: Vec<Pig> = Vec::new();

        let mut pig = Pig::Pre;
        loop {
            path.push(pig);
            match pig {
                // ---------------- Pre pig：规划 / 分流 ----------------
                Pig::Pre => {
                    tracing::info!(pig = "pre", "starting pig");
                    let payload = prompts::pre_payload(&ctx.user_question, ctx.lang, &failure_paths);
                    let text = ctx.call_pig(Pig::Pre, &payload).await?;
                    match detect_marker(&text) {
                        // 简单路径：Pre 直接给出答案，整轮结束
                        Some(Marker::End) => {
                            return Ok(TurnResult {
                                text: strip_markers(&text),
                                ended_with: EndedWith::SimplePath,
                                session: ctx.session.clone(),
                                path,
                            });
                        }
                        // 路径失败 → 记录，重规划（预算内）
                        Some(Marker::Failed) => {
                            failure_paths.push(strip_markers(&text));
                            pre_replans += 1;
                            if pre_replans > MAX_PRE_REPLANS {
                                return Ok(TurnResult {
                                    text: failure_paths.last().cloned().unwrap_or_default(),
                                    ended_with: EndedWith::PigFailBudget,
                                    session: ctx.session.clone(),
                                    path,
                                });
                            }
                        }
                        // 正常计划 → 交给 Executor
                        None => {
                            pre_output = strip_markers(&text);
                            pig = Pig::Executor;
                        }
                    }
                }
                // ---------------- Executor pig：执行 ----------------
                Pig::Executor => {
                    tracing::info!(pig = "executor", "starting pig");
                    let payload = prompts::executor_payload(
                        &ctx.user_question,
                        ctx.lang,
                        &pre_output,
                        &last_post_feedback,
                    );
                    // 不解析标记：Executor 之后总是进 Post 验收
                    executor_draft = ctx.call_pig(Pig::Executor, &payload).await?;
                    last_post_feedback.clear();
                    pig = Pig::Post;
                }
                // ---------------- Post pig：验收 / 路由 ----------------
                Pig::Post => {
                    tracing::info!(pig = "post", "starting pig");
                    let payload =
                        prompts::post_payload(&ctx.user_question, ctx.lang, &pre_output, &executor_draft);
                    let text = ctx.call_pig(Pig::Post, &payload).await?;
                    match detect_marker(&text) {
                        // 验收通过 → 整轮结束
                        Some(Marker::End) => {
                            return Ok(TurnResult {
                                text: strip_markers(&text),
                                ended_with: EndedWith::PigEnd,
                                session: ctx.session.clone(),
                                path,
                            });
                        }
                        // 执行走偏 → 清空产物，回 Pre 重规划（预算内）
                        Some(Marker::Failed) => {
                            failure_paths.push(strip_markers(&text));
                            pre_output.clear();
                            executor_draft.clear();
                            last_post_feedback.clear();
                            pre_replans += 1;
                            if pre_replans > MAX_PRE_REPLANS {
                                return Ok(TurnResult {
                                    text: failure_paths.last().cloned().unwrap_or_default(),
                                    ended_with: EndedWith::PigFailBudget,
                                    session: ctx.session.clone(),
                                    path,
                                });
                            }
                            pig = Pig::Pre;
                        }
                        // 推进了但没完成 → 反馈给 Executor 回环（预算内）
                        None => {
                            last_post_feedback = strip_markers(&text);
                            executor_loops += 1;
                            if executor_loops > MAX_EXECUTOR_LOOPS {
                                return Ok(TurnResult {
                                    text: if executor_draft.is_empty() {
                                        last_post_feedback.clone()
                                    } else {
                                        executor_draft.clone()
                                    },
                                    ended_with: EndedWith::ExecutorLoopBudget,
                                    session: ctx.session.clone(),
                                    path,
                                });
                            }
                            pig = Pig::Executor;
                        }
                    }
                }
            }
        }
    }
}

/// 一轮编排的内部上下文。
struct Ctx {
    input: TurnInput,
    session: String,
    lang: lang::Lang,
    user_question: String,
    transport: Arc<dyn Transport>,
}

impl Ctx {
    /// 组装并发送一只 pig 的子请求，返回提取出的文本。
    async fn call_pig(&self, pig: Pig, payload: &str) -> Result<String> {
        let mut body = self.input.body.clone();
        proto::set_stream(&mut body, false);
        proto::strip_tools(&mut body);
        proto::replace_last_user_text(&mut body, self.input.protocol, payload)?;

        // 子请求不带 accept-encoding：强制上游回明文 JSON。
        // 否则客户端的 gzip 头会一路透传到上游，压缩体回到编排层无法解析。
        let mut headers: Vec<(String, String)> = self
            .input
            .base_headers
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("accept-encoding"))
            .cloned()
            .collect();
        let has = |headers: &[(String, String)], key: &str| {
            headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
        };
        if !has(&headers, "content-type") {
            headers.push(("content-type".into(), "application/json".into()));
        }
        if !has(&headers, SESSION_HEADER) {
            headers.push((SESSION_HEADER.into(), self.session.clone()));
        }

        let body_bytes = bytes::Bytes::from(
            serde_json::to_vec(&body).map_err(|e| TransportError::Send(format!("body 序列化失败: {e}")))?,
        );
        tracing::debug!(pig = pig.as_str(), bytes = body_bytes.len(), "sending subrequest");

        let resp = self
            .transport
            .send(SubRequest {
                path: self.input.path.clone(),
                headers,
                body: body_bytes,
            })
            .await?;

        extract_text(self.input.protocol, pig, &resp)
    }
}

/// 从子请求响应提取文本：JSON 走结构化提取，SSE 走增量拼接。
fn extract_text(protocol: proto::Protocol, pig: Pig, resp: &SubResponse) -> Result<String> {
    if !(200..300).contains(&resp.status) {
        return Err(Error::Transport(TransportError::Upstream {
            status: resp.status,
            body: String::from_utf8_lossy(&resp.body).chars().take(500).collect(),
        }));
    }
    let text = if proto::is_sse_content_type(resp.content_type.as_deref()) {
        proto::extract_sse_text(protocol, &String::from_utf8_lossy(&resp.body))
    } else {
        match serde_json::from_slice::<Value>(&resp.body) {
            Ok(v) => proto::extract_response_text(protocol, &v),
            Err(e) => {
                // 上游回了 2xx 但 body 不是 JSON：把现场带进错误，便于诊断
                let snippet: String = String::from_utf8_lossy(&resp.body).chars().take(300).collect();
                return Err(Error::Protocol(proto::Error::InvalidJsonWithBody {
                    reason: e.to_string(),
                    content_type: resp.content_type.clone().unwrap_or_default(),
                    snippet,
                }));
            }
        }
    };
    text.ok_or(Error::NoText(protocol, pig.as_str()))
}

/// 从客户端请求头里找会话头（大小写不敏感）。
pub fn find_client_session(headers: &[(String, String)]) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(SESSION_HEADER))
        .map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use bytes::Bytes;
    use std::sync::Mutex;

    /// 脚本化假传输：按调用次序回放响应，并记录收到的每个子请求。
    struct FakeTransport {
        responses: Mutex<Vec<SubResponse>>,
        requests: Mutex<Vec<SubRequest>>,
    }

    impl FakeTransport {
        fn json(status: u16, body: Value) -> SubResponse {
            SubResponse {
                status,
                content_type: Some("application/json".into()),
                body: Bytes::from(body.to_string()),
            }
        }
        /// 同时携带三种协议的文本形状，任何协议都能提取
        fn text(status: u16, text: &str) -> SubResponse {
            Self::json(
                status,
                serde_json::json!({
                    "choices": [{"message": {"content": text}}],
                    "content": [{"type": "text", "text": text}],
                    "output_text": text
                }),
            )
        }
    }

    #[async_trait::async_trait]
    impl Transport for FakeTransport {
        async fn send(&self, req: SubRequest) -> transport::TransportResult {
            let resp = self.responses.lock().unwrap().remove(0);
            self.requests.lock().unwrap().push(req);
            Ok(resp)
        }
    }

    fn input(protocol: proto::Protocol) -> TurnInput {
        let (path, model): (&str, &str) = match protocol {
            proto::Protocol::OpenAI => ("/chat/completions", "gpt-x"),
            proto::Protocol::Anthropic => ("/v1/messages", "claude-x"),
            proto::Protocol::Responses => ("/responses", "r-x"),
        };
        let body = if protocol == proto::Protocol::Anthropic {
            // Anthropic：system 是顶层字段，不在 messages 里
            serde_json::json!({
                "model": model,
                "stream": true,
                "tools": [{"type": "function"}],
                "system": "sys",
                "messages": [{"role": "user", "content": "帮我完成任务Z"}]
            })
        } else {
            serde_json::json!({
                "model": model,
                "stream": true,
                "tools": [{"type": "function"}],
                "messages": [
                    {"role": "system", "content": "sys"},
                    {"role": "user", "content": "帮我完成任务Z"}
                ]
            })
        };
        TurnInput {
            protocol,
            body,
            path: path.into(),
            base_headers: vec![("authorization".into(), "Bearer k".into())],
            client_session: None,
        }
    }

    fn strip_model(body: &Value) -> &str {
        body.get("model").and_then(|m| m.as_str()).unwrap()
    }

    #[tokio::test]
    async fn happy_path_three_pigs_with_pigend() {
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(vec![
                FakeTransport::text(200, "分析：需要X和Y"),  // pre
                FakeTransport::text(200, "执行结果……"),      // executor
                FakeTransport::text(200, "验收通过\nPIGEND"), // post
            ]),
            requests: Mutex::new(vec![]),
        });
        let result = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake.clone())
            .await
            .unwrap();

        assert_eq!(result.ended_with, EndedWith::PigEnd);
        assert_eq!(result.text, "验收通过");
        assert_eq!(result.path, vec![Pig::Pre, Pig::Executor, Pig::Post]);

        let reqs = fake.requests.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        for (i, req) in reqs.iter().enumerate() {
            assert_eq!(req.path, "/chat/completions");
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            // model 已剥 -pig、关闭流式、去掉工具
            assert_eq!(strip_model(&body), "gpt-x");
            assert_eq!(body["stream"], false);
            assert!(body.get("tools").is_none());
            // 会话头贯穿且一致
            let session = &req
                .headers
                .iter()
                .find(|(k, _)| k == SESSION_HEADER)
                .unwrap()
                .1;
            assert_eq!(session, &result.session);
            // 鉴权透传
            assert!(req.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer k"));
            // payload 递进：pre → executor(含 pre 产物) → post(含草稿)
            let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap();
            if i == 0 {
                assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
            } else if i == 1 {
                assert!(content.contains("分析：需要X和Y"));
            } else {
                assert!(content.contains("执行结果……") && content.contains("验收"));
            }
        }
    }

    #[tokio::test]
    async fn simple_path_short_circuits_in_pre() {
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(vec![FakeTransport::text(200, "答案是 4\nPIGEND")]),
            requests: Mutex::new(vec![]),
        });
        let result = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake)
            .await
            .unwrap();
        assert_eq!(result.ended_with, EndedWith::SimplePath);
        assert_eq!(result.text, "答案是 4");
        assert_eq!(result.path, vec![Pig::Pre]);
    }

    #[tokio::test]
    async fn post_pigfail_returns_to_pre_with_failure_paths() {
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(vec![
                FakeTransport::text(200, "计划一"),                 // pre#1
                FakeTransport::text(200, "草稿一"),                 // executor#1
                FakeTransport::text(200, "走偏了\nPIGFAIL"),        // post#1 → 重规划
                FakeTransport::text(200, "计划二（这次记住失败路径）"), // pre#2
                FakeTransport::text(200, "草稿二"),                 // executor#2
                FakeTransport::text(200, "通过\nPIGEND"),           // post#2
            ]),
            requests: Mutex::new(vec![]),
        });
        let result = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake.clone())
            .await
            .unwrap();
        assert_eq!(result.ended_with, EndedWith::PigEnd);
        assert_eq!(
            result.path,
            vec![Pig::Pre, Pig::Executor, Pig::Post, Pig::Pre, Pig::Executor, Pig::Post]
        );

        // 第二次 Pre 的 payload 应包含失败路径
        let reqs = fake.requests.lock().unwrap();
        let pre2_body: Value = serde_json::from_slice(&reqs[3].body).unwrap();
        let content = pre2_body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(content.contains("曾失败过") && content.contains("走偏了"));
    }

    #[tokio::test]
    async fn post_without_marker_loops_executor_until_budget() {
        let mut responses = vec![FakeTransport::text(200, "计划"), FakeTransport::text(200, "草稿")];
        // post 每次都不给标记 → executor 回环 3 次后超预算
        for _ in 0..4 {
            responses.push(FakeTransport::text(200, "还需改进"));
            responses.push(FakeTransport::text(200, "改进后的草稿"));
        }
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(responses),
            requests: Mutex::new(vec![]),
        });
        let result = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake)
            .await
            .unwrap();
        assert_eq!(result.ended_with, EndedWith::ExecutorLoopBudget);
        assert_eq!(result.text, "改进后的草稿");
    }

    #[tokio::test]
    async fn upstream_error_stops_immediately() {
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(vec![FakeTransport::json(
                500,
                serde_json::json!({"error": "boom"}),
            )]),
            requests: Mutex::new(vec![]),
        });
        let err = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Transport(TransportError::Upstream { status: 500, .. })));
    }

    #[tokio::test]
    async fn client_session_header_is_inherited() {
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(vec![FakeTransport::text(200, "答案\nPIGEND")]),
            requests: Mutex::new(vec![]),
        });
        let mut input = input(proto::Protocol::OpenAI);
        input.client_session = Some("client-fixed-session".into());
        let result = Orchestrator::new().run(input, fake.clone()).await.unwrap();
        assert_eq!(result.session, "client-fixed-session");
        let reqs = fake.requests.lock().unwrap();
        let session = &reqs[0]
            .headers
            .iter()
            .find(|(k, _)| k == SESSION_HEADER)
            .unwrap()
            .1;
        assert_eq!(session, "client-fixed-session");
    }

    #[tokio::test]
    async fn anthropic_protocol_body_surgery() {
        let fake = Arc::new(FakeTransport {
            responses: Mutex::new(vec![FakeTransport::text(200, "答案\nPIGEND")]),
            requests: Mutex::new(vec![]),
        });
        let result = Orchestrator::new()
            .run(input(proto::Protocol::Anthropic), fake.clone())
            .await
            .unwrap();
        assert_eq!(result.ended_with, EndedWith::SimplePath);
        let body: Value = serde_json::from_slice(&fake.requests.lock().unwrap()[0].body).unwrap();
        assert_eq!(strip_model(&body), "claude-x");
        let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
        // system 字段保持原样
        assert_eq!(body["system"], "sys");
    }
}
