//! pigs-orchestrator —— 编排引擎。
//!
//! 三只 pig 组成一头 pigs：Pre（规划/分流）→ Executor（执行）→ Post（验收），
//! 由 PIGEND / PIGFAIL 控制标记与预算驱动的状态机串起来。
//!
//! 客户端要流式时，子请求也走流式：上游增量边到边过滤控制标记边转发给客户端
//! （[`markers::MarkerFilter`]），最终答复 = 各只 pig 可见文本按顺序拼接。
//!
//! 红线：不认识 axum（网络走 [`Transport`] 注入）、不做重试、不知道上游是谁。

pub mod lang;
pub mod markers;
pub mod prompts;
pub mod transport;

use markers::{detect_marker, strip_markers, Marker, MarkerFilter};
use pigs_protocol as proto;
use transport::{SubRequest, SubResponse, TextSink, Transport, TransportError};
use std::sync::{Arc, Mutex};
use serde_json::Value;

/// 会话头名：编排产生的稳定会话标识，所有子请求共用（mini-proxy 见已带就不覆盖）。
pub const SESSION_HEADER: &str = "x-opencode-session";

/// 回环内部令牌头（proxy 验证后跳过 -pig 分流，防递归）。
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

/// 一只 pig 的生命周期事件（proxy 用它边编排边推 SSE 帧）。
#[derive(Debug, Clone)]
pub enum PigEvent {
    /// 该 pig 的子请求已发出。
    Start(Pig),
    /// 一段可安全转发的可见文本（控制标记已过滤）。
    Delta(String),
    /// 该 pig 的文本流结束。
    End(Pig),
}

/// 进度回调：整个编排过程中按顺序收到 [`PigEvent`]。
pub type ProgressSink = Arc<dyn Fn(PigEvent) + Send + Sync>;

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
    /// 最终答复文本 = 各只 pig 可见文本按顺序用空行拼接（已剥离控制标记）。
    pub text: String,
    /// 每只 pig 的可见文本（按执行顺序，诊断/测试用）。
    pub visible: Vec<String>,
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
    #[error("上游响应无法提取文本（协议 {0:?}，pig {1}）")]
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

    /// 跑完一整轮三 pig 编排（非流式：子请求要 JSON 全文）。
    pub async fn run(&self, input: TurnInput, transport: Arc<dyn Transport>) -> Result<TurnResult> {
        self.run_with_progress(input, transport, None).await
    }

    /// 跑完一整轮三 pig 编排；给了 `progress` 就走流式（子请求带 `stream:true`，
    /// 上游增量边过滤边回调）。
    pub async fn run_with_progress(
        &self,
        input: TurnInput,
        transport: Arc<dyn Transport>,
        progress: Option<ProgressSink>,
    ) -> Result<TurnResult> {
        let session = input
            .client_session
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let lang = lang::detect_lang(&proto::extract_last_user_text(&input.body, input.protocol));

        let ctx = Ctx {
            input,
            session,
            lang,
            transport,
            progress,
        };

        let mut failure_paths: Vec<String> = Vec::new();
        let mut pre_output = String::new();
        // 已产出的相位文本（Executor / Post），按顺序作为 assistant 消息接回 Post 的对话
        let mut transcript: Vec<String> = Vec::new();
        let mut visible: Vec<String> = Vec::new();
        let mut path: Vec<Pig> = Vec::new();
        let mut pre_replans: u32 = 0;
        let mut post_iterations: u32 = 0;

        let mut pig = Pig::Pre;
        loop {
            path.push(pig);
            match pig {
                // ---------------- Pre pig：规划 / 分流 ----------------
                Pig::Pre => {
                    tracing::info!(pig = "pre", "starting pig");
                    let instruction = prompts::pre_instruction(ctx.lang, &failure_paths);
                    let raw = ctx
                        .call_pig(Pig::Pre, &instruction, Placement::AppendToUser, &[])
                        .await?;
                    let text = strip_markers(&raw);
                    visible.push(text.clone());
                    match detect_marker(&raw) {
                        // 简单路径：Pre 直接给出答案，整轮结束
                        Some(Marker::End) => {
                            return Ok(ctx.finish(visible, EndedWith::SimplePath, path));
                        }
                        // 路径失败 → 记录，回 Pre 重规划（预算内）
                        Some(Marker::Failed) => {
                            if pre_replans >= MAX_PRE_REPLANS {
                                return Err(Error::Budget(format!(
                                    "Pre 重规划次数超过 {MAX_PRE_REPLANS} 次"
                                )));
                            }
                            failure_paths.push(text);
                            pre_replans += 1;
                            pre_output.clear();
                            post_iterations = 0;
                        }
                        // 正常计划 → 交给 Executor
                        None => {
                            pre_output = text;
                            pig = Pig::Executor;
                        }
                    }
                }
                // ---------------- Executor pig：执行 ----------------
                Pig::Executor => {
                    tracing::info!(pig = "executor", "starting pig");
                    let instruction = prompts::executor_instruction(ctx.lang, &pre_output);
                    let raw = ctx
                        .call_pig(Pig::Executor, &instruction, Placement::AppendToUser, &[])
                        .await?;
                    // 不解析标记：Executor 之后总是进 Post 验收
                    let text = strip_markers(&raw);
                    visible.push(text.clone());
                    transcript.push(text);
                    pig = Pig::Post;
                }
                // ---------------- Post pig：验收 / 路由 ----------------
                Pig::Post => {
                    tracing::info!(pig = "post", "starting pig");
                    // 草稿与历次评审作为 assistant 消息在场，验收指令是新的一条 user 消息
                    let instruction = prompts::post_instruction(ctx.lang);
                    let raw = ctx
                        .call_pig(Pig::Post, &instruction, Placement::NewUserMessage, &transcript)
                        .await?;
                    let text = strip_markers(&raw);
                    visible.push(text.clone());
                    transcript.push(text.clone());
                    match detect_marker(&raw) {
                        // 验收通过 → 整轮结束
                        Some(Marker::End) => {
                            return Ok(ctx.finish(visible, EndedWith::PigEnd, path));
                        }
                        // 执行走偏 → 清空产物，回 Pre 重规划（预算内）
                        Some(Marker::Failed) => {
                            if pre_replans >= MAX_PRE_REPLANS {
                                return Err(Error::Budget(format!(
                                    "Pre 重规划次数超过 {MAX_PRE_REPLANS} 次"
                                )));
                            }
                            failure_paths.push(text);
                            pre_replans += 1;
                            pre_output.clear();
                            post_iterations = 0;
                            pig = Pig::Pre;
                        }
                        // 推进了但没完成 → 提示词要求它继续执行任务，所以再走一次 Post
                        None => {
                            if post_iterations >= MAX_POST_ITERATIONS {
                                return Err(Error::Budget(format!(
                                    "Post 无标记重试次数超过 {MAX_POST_ITERATIONS} 次"
                                )));
                            }
                            post_iterations += 1;
                            pig = Pig::Post;
                        }
                    }
                }
            }
        }
    }
}

/// 相位指令落在 body 的哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// 追加到最后一条 user 消息的文本后面（Pre / Executor：接着用户的话说）。
    AppendToUser,
    /// 作为新的一条 user 消息（Post：草稿已作为 assistant 消息在场）。
    NewUserMessage,
}

/// 一轮编排的内部上下文。
struct Ctx {
    input: TurnInput,
    session: String,
    lang: lang::Lang,
    transport: Arc<dyn Transport>,
    progress: Option<ProgressSink>,
}

impl Ctx {
    /// 收尾：把各 pig 可见文本拼成最终答复。
    fn finish(&self, visible: Vec<String>, ended_with: EndedWith, path: Vec<Pig>) -> TurnResult {
        TurnResult {
            text: join_visible(&visible),
            visible,
            ended_with,
            session: self.session.clone(),
            path,
        }
    }

    /// 组装并发送一只 pig 的子请求，返回提取出的原始文本（含控制标记）。
    ///
    /// body 的手术顺序：① 把对话记录（上一只 pig 的产出）作为 assistant 消息接回；
    /// ② 再落本相位的指令——Pre/Executor 追加到最后一条 user 消息文本上，
    /// Post 作为新的一条 user 消息。这样用户原话、图片等内容一律不动。
    async fn call_pig(
        &self,
        pig: Pig,
        instruction: &str,
        placement: Placement,
        transcript: &[String],
    ) -> Result<String> {
        // 客户端要流式 → 子请求也流式（增量边到边转发）；否则要 JSON 全文
        let streaming = self.progress.is_some();
        let mut body = self.input.body.clone();
        proto::set_stream(&mut body, streaming);
        proto::strip_tools(&mut body);
        for text in transcript {
            proto::push_assistant_message(&mut body, self.input.protocol, text)?;
        }
        match placement {
            Placement::AppendToUser => {
                proto::append_to_last_user_text(&mut body, self.input.protocol, instruction)?
            }
            Placement::NewUserMessage => {
                proto::push_user_message(&mut body, self.input.protocol, instruction)?
            }
        }

        // 子请求不带 accept-encoding：否则客户端可能空的 gzip 头会一路透传到上游，
        // 压缩体回到编排层就没法解析（也不利于增量解析 SSE）。
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
        tracing::debug!(pig = pig.as_str(), bytes = body_bytes.len(), streaming, "sending subrequest");

        let req = SubRequest {
            path: self.input.path.clone(),
            headers,
            body: body_bytes,
        };

        let resp = match &self.progress {
            // 流式：增量交给 MarkerFilter，确认安全的文本立刻回调出去
            Some(progress) => {
                progress(PigEvent::Start(pig));
                let filter = Arc::new(Mutex::new(MarkerFilter::new()));
                let sink: TextSink = {
                    let filter = Arc::clone(&filter);
                    let progress = Arc::clone(progress);
                    Arc::new(move |delta: &str| {
                        let visible = filter
                            .lock()
                            .map(|mut f| f.push(delta))
                            .unwrap_or_default();
                        if !visible.is_empty() {
                            progress(PigEvent::Delta(visible));
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

        extract_text(self.input.protocol, pig, &resp)
    }
}

/// 把各段可见文本用空行拼接成最终答复（空段丢弃）。
fn join_visible(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 从子请求响应提取原始文本：JSON 走结构化提取，SSE 走增量拼接。
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
    /// 流式方法把脚本响应里的文本按小块喂给 sink（模拟上游增量）。
    struct FakeTransport {
        responses: Mutex<Vec<SubResponse>>,
        requests: Mutex<Vec<SubRequest>>,
        /// 每次发送时 body 里的 stream 标志（断言子请求是否流式）
        stream_flags: Mutex<Vec<bool>>,
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
        /// 从脚本响应里取出文本（假上游的"增量源"）
        fn text_of(resp: &SubResponse) -> String {
            let v: Value = serde_json::from_slice(&resp.body).unwrap();
            proto::extract_response_text(proto::Protocol::OpenAI, &v).unwrap_or_default()
        }
    }

    #[async_trait::async_trait]
    impl Transport for FakeTransport {
        async fn send(&self, req: SubRequest) -> transport::TransportResult {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            self.stream_flags
                .lock()
                .unwrap()
                .push(body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false));
            self.requests.lock().unwrap().push(req);
            Ok(self.responses.lock().unwrap().remove(0))
        }

        async fn send_streaming(
            &self,
            req: SubRequest,
            _protocol: proto::Protocol,
            sink: TextSink,
        ) -> transport::TransportResult {
            let resp = self.send(req).await?;
            // 逐字喂入，模拟上游把一行拆成多个 SSE 增量
            for ch in Self::text_of(&resp).chars() {
                sink(&ch.to_string());
            }
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

    fn fake(responses: Vec<SubResponse>) -> Arc<FakeTransport> {
        Arc::new(FakeTransport {
            responses: Mutex::new(responses),
            requests: Mutex::new(vec![]),
            stream_flags: Mutex::new(vec![]),
        })
    }

    fn strip_model(body: &Value) -> &str {
        body.get("model").and_then(|m| m.as_str()).unwrap()
    }

    fn last_user_content(body: &Value) -> String {
        body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn happy_path_three_pigs_with_pigend() {
        let fake = fake(vec![
            FakeTransport::text(200, "分析：需要X和Y"),  // pre
            FakeTransport::text(200, "执行结果……"),      // executor
            FakeTransport::text(200, "验收通过\nPIGEND"), // post
        ]);
        let result = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake.clone())
            .await
            .unwrap();

        assert_eq!(result.ended_with, EndedWith::PigEnd);
        // 最终答复 = 三只 pig 的可见文本按顺序拼接（legacy 语义）
        assert_eq!(result.text, "分析：需要X和Y\n\n执行结果……\n\n验收通过");
        assert_eq!(result.path, vec![Pig::Pre, Pig::Executor, Pig::Post]);

        let reqs = fake.requests.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        // 非流式编排：子请求都要求 JSON 全文
        assert_eq!(*fake.stream_flags.lock().unwrap(), vec![false, false, false]);
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
            // 相位指令的落点：pre/executor 追加在用户原话后面；post 是新的一条 user 消息
            let msgs = body["messages"].as_array().unwrap();
            let content = last_user_content(&body);
            if i == 0 {
                assert_eq!(msgs.len(), 2, "Pre：指令追加在原 user 消息上");
                assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
                assert!(content.contains("---"), "追加格式 = 空行 + 分隔线 + 空行");
            } else if i == 1 {
                assert_eq!(msgs.len(), 2, "Executor：同样追加在原 user 消息上");
                assert!(content.contains("帮我完成任务Z") && content.contains("分析：需要X和Y"));
            } else {
                // Post：原问题不许被覆盖；草稿作为 assistant 消息在场；指令是新的一条 user
                assert_eq!(msgs.len(), 4, "system + user(原问题) + assistant(草稿) + user(验收指令)");
                assert_eq!(msgs[1]["role"], "user");
                assert_eq!(msgs[1]["content"], "帮我完成任务Z");
                assert_eq!(msgs[2]["role"], "assistant");
                assert_eq!(msgs[2]["content"], "执行结果……");
                assert_eq!(msgs[3]["role"], "user");
                assert!(content.contains("验收"));
                assert!(!content.contains("执行结果……"), "草稿不该再抄一遍进指令");
            }
        }
    }

    /// 流式编排：子请求带 `stream:true`，增量逐字到达并被实时过滤。
    /// 断言：① 客户端边收边拿到的内容（经协议编码器）== 最终答复；
    /// ② 控制标记绝不出现在客户端流里。
    #[tokio::test]
    async fn streaming_turn_pushes_filtered_deltas_as_they_arrive() {
        let fake = fake(vec![
            FakeTransport::text(200, "分析：需要X\n第二行"),
            FakeTransport::text(200, "草稿\nPIGFAIL"),
            FakeTransport::text(200, "评审：还差一点"),
            FakeTransport::text(200, "继续做完\nPIGEND"),
        ]);
        // 模拟 proxy：把进度事件喂给协议 SSE 编码器，攒出客户端实际收到的字节
        let frames: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let encoder = Arc::new(Mutex::new(proto::StreamEncoder::new(
            proto::Protocol::OpenAI,
            "gpt-x-pig",
        )));
        frames.lock().unwrap().push_str(&encoder.lock().unwrap().start());
        let events: Arc<Mutex<Vec<PigEvent>>> = Arc::new(Mutex::new(vec![]));
        let sink: ProgressSink = {
            let frames = Arc::clone(&frames);
            let encoder = Arc::clone(&encoder);
            let events = Arc::clone(&events);
            Arc::new(move |event| {
                let mut encoder = encoder.lock().unwrap();
                let mut out = match &event {
                    PigEvent::Delta(text) => encoder.push_text(text),
                    PigEvent::End(_) => encoder.end_pig(),
                    PigEvent::Start(_) => String::new(),
                };
                drop(encoder);
                if !out.is_empty() {
                    frames.lock().unwrap().push_str(&mut out);
                }
                events.lock().unwrap().push(event);
            })
        };
        let result = Orchestrator::new()
            .run_with_progress(input(proto::Protocol::OpenAI), fake.clone(), Some(sink))
            .await
            .unwrap();
        frames
            .lock()
            .unwrap()
            .push_str(&encoder.lock().unwrap().finish());

        // 子请求必须是流式的
        assert_eq!(
            *fake.stream_flags.lock().unwrap(),
            vec![true, true, true, true]
        );
        // 每只 pig 一对 Start/End，顺序正确
        let starts: Vec<&'static str> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                PigEvent::Start(p) => Some(p.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec!["pre", "executor", "post", "post"]);

        // 最终答复 = 各 pig 可见文本空行拼接
        assert_eq!(
            result.text,
            "分析：需要X\n第二行\n\n草稿\n\n评审：还差一点\n\n继续做完"
        );
        assert_eq!(result.ended_with, EndedWith::PigEnd);

        // 客户端边收边拿到的内容（编码后的 SSE 回读）必须与最终答复完全一致
        let streamed = proto::extract_sse_text(proto::Protocol::OpenAI, &frames.lock().unwrap())
            .unwrap_or_default();
        assert_eq!(streamed, result.text);
        // 控制标记绝不出现在客户端流里
        assert!(!streamed.contains("PIGFAIL") && !streamed.contains("PIGEND"));
    }

    #[tokio::test]
    async fn simple_path_short_circuits_in_pre() {
        let fake = fake(vec![FakeTransport::text(200, "答案是 4\nPIGEND")]);
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
        let fake = fake(vec![
            FakeTransport::text(200, "计划一"),                 // pre#1
            FakeTransport::text(200, "草稿一"),                 // executor#1
            FakeTransport::text(200, "走偏了\nPIGFAIL"),        // post#1 → 重规划
            FakeTransport::text(200, "计划二（这次记住失败路径）"), // pre#2
            FakeTransport::text(200, "草稿二"),                 // executor#2
            FakeTransport::text(200, "通过\nPIGEND"),           // post#2
        ]);
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
        assert!(last_user_content(&pre2_body).contains("走偏了"));
    }

    #[tokio::test]
    async fn post_without_marker_retries_post_until_budget() {
        // pre + executor + post×4（最后一次超预算 → 报错，不假装成功）
        let mut responses = vec![
            FakeTransport::text(200, "计划"),
            FakeTransport::text(200, "草稿"),
        ];
        for _ in 0..4 {
            responses.push(FakeTransport::text(200, "还需改进"));
        }
        let fake = fake(responses);
        let err = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Budget(m) if m.contains("Post 无标记重试")));
        // Post 回环了三只 pig + 三次重试
        let reqs = fake.requests.lock().unwrap();
        assert_eq!(reqs.len(), MAX_POST_ITERATIONS as usize + 3);
    }

    #[tokio::test]
    async fn pre_replan_budget_exhausted_is_an_error() {
        let fake = fake(vec![
            FakeTransport::text(200, "计划一\nPIGFAIL"),
            FakeTransport::text(200, "计划二\nPIGFAIL"),
            FakeTransport::text(200, "计划三\nPIGFAIL"),
        ]);
        let err = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Budget(m) if m.contains("Pre 重规划")));
    }

    #[tokio::test]
    async fn upstream_error_stops_immediately() {
        let fake = fake(vec![FakeTransport::json(500, serde_json::json!({"error": "boom"}))]);
        let err = Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), fake)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Transport(TransportError::Upstream { status: 500, .. })));
    }

    #[tokio::test]
    async fn client_session_header_is_inherited() {
        let fake = fake(vec![FakeTransport::text(200, "答案\nPIGEND")]);
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
        let fake = fake(vec![FakeTransport::text(200, "答案\nPIGEND")]);
        let result = Orchestrator::new()
            .run(input(proto::Protocol::Anthropic), fake.clone())
            .await
            .unwrap();
        assert_eq!(result.ended_with, EndedWith::SimplePath);
        let body: Value = serde_json::from_slice(&fake.requests.lock().unwrap()[0].body).unwrap();
        assert_eq!(strip_model(&body), "claude-x");
        let content = last_user_content(&body);
        assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
        // system 字段保持原样
        assert_eq!(body["system"], "sys");
    }
}
