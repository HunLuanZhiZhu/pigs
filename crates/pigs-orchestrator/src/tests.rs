//! 编排器单元测试：脚本化假传输 + 准则合规断言 + 工具暂停/恢复。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use bytes::Bytes;
use serde_json::json;

/// 脚本化假传输：按调用次序回放响应，并记录收到的每个子请求。
/// 流式方法把脚本响应里的文本按小块喂给 sink（模拟上游增量）。
pub(super) struct FakeTransport {
    responses: Mutex<Vec<SubResponse>>,
    requests: Mutex<Vec<SubRequest>>,
    /// 有值 = 流式时先吐思考增量（再吐签名），模拟"边想边答"。
    thinking: Option<String>,
}

impl FakeTransport {
    fn json(status: u16, body: serde_json::Value) -> SubResponse {
        SubResponse {
            status,
            content_type: Some("application/json".into()),
            body: Bytes::from(body.to_string()),
        }
    }

    /// 同时携带三种协议的文本形状，任何协议都能提取；带 usage 便于验证整对象选择。
    fn text(status: u16, text: &str) -> SubResponse {
        Self::json(
            status,
            json!({
                "choices": [{"message": {"content": text}, "finish_reason": "stop"}],
                "content": [{"type": "text", "text": text}],
                "stop_reason": "end_turn",
                "output_text": text,
                "usage": {"input_tokens": 1, "output_tokens": 2}
            }),
        )
    }

    /// 模型要调用工具（OpenAI Chat 形状）。
    fn tool_calls(status: u16, calls: &[(&str, &str)]) -> SubResponse {
        let calls: Vec<serde_json::Value> = calls
            .iter()
            .map(|(id, name)| {
                json!({
                    "id": id, "type": "function",
                    "function": {"name": name, "arguments": "{\"command\":\"ls\"}"}
                })
            })
            .collect();
        Self::json(
            status,
            json!({
                "choices": [{
                    "message": {"role": "assistant", "content": null, "tool_calls": calls},
                    "finish_reason": "tool_calls"
                }],
                "usage": {"input_tokens": 5, "output_tokens": 1}
            }),
        )
    }

    fn text_of(resp: &SubResponse) -> String {
        let value: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        proto::parse_json_output(proto::Protocol::OpenAI, &value).text
    }
}

#[async_trait::async_trait]
impl Transport for FakeTransport {
    async fn send(&self, req: SubRequest) -> transport::TransportResult {
        self.requests.lock().unwrap().push(req);
        Ok(self.responses.lock().unwrap().remove(0))
    }

    async fn send_streaming(
        &self,
        req: SubRequest,
        _protocol: proto::Protocol,
        sink: LiveSink,
    ) -> transport::TransportResult {
        let resp = self.send(req).await?;
        // 先思考（含签名），再逐字吐文本——模拟上游真实的增量顺序
        if let Some(thinking) = &self.thinking {
            for ch in thinking.chars() {
                sink(proto::LiveEvent::Thinking(ch.to_string()));
            }
            sink(proto::LiveEvent::ThinkingSignature("sig-1".into()));
        }
        for ch in Self::text_of(&resp).chars() {
            sink(proto::LiveEvent::Text(ch.to_string()));
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
        json!({
            "model": model,
            "stream": true,
            "tools": [{"name": "Bash", "input_schema": {}}],
            "tool_choice": {"type": "auto"},
            "temperature": 0.3,
            "system": "sys",
            "messages": [{"role": "user", "content": "帮我完成任务Z"}]
        })
    } else {
        json!({
            "model": model,
            "stream": true,
            "tools": [{"type": "function", "function": {"name": "Bash"}}],
            "tool_choice": "auto",
            "temperature": 0.3,
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
        query: Some("beta=1".into()),
        base_headers: vec![
            ("authorization".into(), "Bearer k".into()),
            ("accept-encoding".into(), "gzip, deflate, br".into()),
        ],
        client_session: None,
    }
}

/// 建一份运行时（默认非流式），返回 Runtime 与 continuation 存储。
fn runtime(transport: Arc<FakeTransport>) -> (Runtime, Arc<Mutex<ContinuationStore>>) {
    let store = Arc::new(Mutex::new(ContinuationStore::default()));
    let rt = Runtime {
        transport,
        store: Arc::clone(&store),
        progress: None,
    };
    (rt, store)
}

fn fake(responses: Vec<SubResponse>) -> Arc<FakeTransport> {
    Arc::new(FakeTransport {
        responses: Mutex::new(responses),
        requests: Mutex::new(vec![]),
        thinking: None,
    })
}

/// 假传输 + "先思考再回答"的流式行为。
fn fake_with_thinking(responses: Vec<SubResponse>, thinking: &str) -> Arc<FakeTransport> {
    Arc::new(FakeTransport {
        responses: Mutex::new(responses),
        requests: Mutex::new(vec![]),
        thinking: Some(thinking.to_string()),
    })
}

fn completed(outcome: Outcome) -> TurnResult {
    match outcome {
        Outcome::Completed(result) => result,
        Outcome::Paused(paused) => panic!("期望完成，却暂停在工具调用: {:?}", paused.tool_calls),
    }
}

fn paused(outcome: Outcome) -> PausedTurn {
    match outcome {
        Outcome::Paused(paused) => paused,
        Outcome::Completed(result) => panic!("期望暂停，却完成了: {result:?}"),
    }
}

fn strip_model(body: &serde_json::Value) -> &str {
    body.get("model").and_then(|m| m.as_str()).unwrap()
}

fn roles(body: &serde_json::Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_string())
        .collect()
}

fn last_message_content(body: &serde_json::Value) -> String {
    body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 造一发"带工具结果"的客户端请求（OpenAI 形状）：尾部是 tool 消息。
fn resume_body(ids: &[&str]) -> TurnInput {
    let mut input = input(proto::Protocol::OpenAI);
    let messages = input.body["messages"].as_array_mut().unwrap();
    for id in ids {
        messages.push(json!({
            "role": "assistant", "content": null,
            "tool_calls": [{"id": id, "type": "function",
                            "function": {"name": "Bash", "arguments": "{}"}}]
        }));
        messages.push(json!({"role": "tool", "tool_call_id": id, "content": "输出"}));
    }
    input
}

/// 准则（AGENTS.md）：子请求**一个字段都不许改**——`tools`/`stream`/`temperature`/query/头全部原样，
/// 只允许 model 剥后缀、尾部追加相位指令、（现行）补会话头。
#[tokio::test]
async fn subrequests_pass_everything_through_untouched() {
    let transport = fake(vec![FakeTransport::text(200, "分析\nPIGEND")]);
    let (rt, _store) = runtime(transport.clone());
    let result = completed(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );
    assert_eq!(result.ended_with, EndedWith::SimplePath);

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    // model 剥后缀（唯一允许的字段改动）
    assert_eq!(strip_model(&body), "gpt-x");
    // 其余字段原样
    assert_eq!(body["stream"], true, "stream 不许被改写");
    assert_eq!(body["tools"][0]["function"]["name"], "Bash", "tools 不许被删");
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["temperature"], 0.3);
    // 尾部追加相位指令（末条 user 消息被追加）
    let content = last_message_content(&body);
    assert!(content.starts_with("帮我完成任务Z"));
    assert!(content.contains("执行前分析"));
    // query 原样带上
    assert_eq!(req.query.as_deref(), Some("beta=1"));
    // 头原样（含 accept-encoding：解压是本地的事，不改请求）
    assert!(req
        .headers
        .iter()
        .any(|(k, v)| k == "accept-encoding" && v.contains("gzip")));
    assert!(req
        .headers
        .iter()
        .any(|(k, v)| k == "authorization" && v == "Bearer k"));
    // 会话头补上了（现行决定：客户端没带就注入）
    let session = req
        .headers
        .iter()
        .find(|(k, _)| k == SESSION_HEADER)
        .unwrap();
    assert_eq!(session.1, result.session);
}

#[tokio::test]
async fn happy_path_three_pigs_with_pigend() {
    let transport = fake(vec![
        FakeTransport::text(200, "分析：需要X和Y"),  // pre
        FakeTransport::text(200, "执行结果……"),      // executor
        FakeTransport::text(200, "验收通过\nPIGEND"), // post
    ]);
    let (rt, _store) = runtime(transport.clone());
    let result = completed(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );

    assert_eq!(result.ended_with, EndedWith::PigEnd);
    assert_eq!(result.text, "分析：需要X和Y\n\n执行结果……\n\n验收通过");
    assert_eq!(result.path, vec![Pig::Pre, Pig::Executor, Pig::Post]);
    // usage 取 input 最大的那只子请求的原对象、stop_reason 取上游的值
    assert_eq!(
        result.usage.unwrap(),
        json!({"input_tokens": 1, "output_tokens": 2})
    );
    assert_eq!(result.stop_reason.as_deref(), Some("stop"));

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    for (i, req) in reqs.iter().enumerate() {
        assert_eq!(req.path, "/chat/completions");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let content = last_message_content(&body);
        if i == 0 {
            assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
        } else if i == 1 {
            assert!(content.contains("分析：需要X和Y"), "Executor 指令要带 Pre 分析");
        } else {
            assert!(content.contains("根据设定的目标"));
            assert_eq!(
                roles(&body),
                vec!["system", "user", "assistant", "user"],
                "Post = 原消息 + assistant(草稿) + user(验收指令)"
            );
            assert_eq!(body["messages"][2]["content"], "执行结果……");
        }
    }
}

#[tokio::test]
async fn simple_path_short_circuits_in_pre() {
    let transport = fake(vec![FakeTransport::text(200, "答案是 4\nPIGEND")]);
    let (rt, _store) = runtime(transport.clone());
    let result = completed(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );
    assert_eq!(result.ended_with, EndedWith::SimplePath);
    assert_eq!(result.text, "答案是 4");
    assert_eq!(result.path, vec![Pig::Pre]);
}

/// 工具调用：相位**不结束**，调用原样交给客户端，现场存进 continuation。
#[tokio::test]
async fn tool_calls_pause_the_phase_and_give_native_calls_to_client() {
    let transport = fake(vec![FakeTransport::tool_calls(200, &[("call_1", "Bash")])]);
    let (rt, store) = runtime(transport.clone());
    let paused = paused(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );

    assert_eq!(paused.tool_calls.len(), 1);
    assert_eq!(paused.tool_calls[0].id, "call_1");
    assert_eq!(paused.tool_calls[0].name, "Bash");
    assert_eq!(paused.tool_calls[0].arguments_json(), "{\"command\":\"ls\"}");
    assert_eq!(paused.text, "");
    assert!(
        paused.parts.iter().any(|part| matches!(part, Part::ToolCall(call) if call.id == "call_1")),
        "Paused 响应必须包含本轮工具调用"
    );
    let continuation = store
        .lock()
        .unwrap()
        .take_match(&["call_1".into()])
        .expect("现场必须存下来等结果");
    assert!(
        continuation
            .state
            .parts
            .iter()
            .all(|part| !matches!(part, Part::ToolCall(_))),
        "continuation 不得持久化已发给客户端的 ToolCall"
    );
}

/// 恢复：客户端把工具结果发回来，接着**同一只 pig** 继续跑。
#[tokio::test]
async fn resume_continues_same_phase_with_tool_result() {
    let transport = fake(vec![
        FakeTransport::tool_calls(200, &[("call_1", "Bash")]), // Pre 第一轮：要工具
        FakeTransport::text(200, "答案是 4\nPIGEND"),           // 恢复后：Pre 给出答案
    ]);
    let (rt, store) = runtime(transport.clone());
    let orchestrator = Orchestrator::new();

    let _paused = paused(
        orchestrator
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );
    let continuation = store
        .lock()
        .unwrap()
        .take_match(&["call_1".into()])
        .expect("应当能按工具结果 id 匹配到现场");
    assert_eq!(continuation.state.phase, Pig::Pre);

    let (rt2, _store2) = runtime(transport.clone());
    let result = completed(
        orchestrator
            .resume(resume_body(&["call_1"]), rt2, continuation)
            .await
            .unwrap(),
    );
    assert_eq!(result.ended_with, EndedWith::SimplePath);
    assert_eq!(result.text, "答案是 4");
    assert!(
        result.parts.iter().all(|part| !matches!(part, Part::ToolCall(_))),
        "已消费的工具调用不能进入最终 Completed"
    );

    // 恢复请求：阶段提示仍只在最初 user 消息中出现一次；工具结果保持在尾部，
    // 不再新增一条 user 消息重新布置阶段任务。
    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    let body: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();
    assert_eq!(roles(&body), vec!["system", "user", "assistant", "tool"]);
    assert_eq!(body["messages"][3]["content"], "输出", "工具结果原样保留");
    let phase_user = body["messages"][1]["content"].as_str().unwrap();
    assert!(phase_user.contains("执行前分析"));
    assert_eq!(phase_user.matches("执行前分析").count(), 1);
    assert_eq!(body["tools"][0]["function"]["name"], "Bash", "工具定义仍带着");
}

/// 多轮工具往返：每次暂停交出一批调用，结果回齐后相位继续，最终产出。
#[tokio::test]
async fn multiple_tool_rounds_inside_one_phase() {
    let transport = fake(vec![
        FakeTransport::tool_calls(200, &[("call_1", "Bash")]),
        FakeTransport::tool_calls(200, &[("call_2", "Bash")]),
        FakeTransport::text(200, "做完了\nPIGEND"),
    ]);
    let (rt, store) = runtime(transport.clone());
    let orchestrator = Orchestrator::new();

    let paused1 = paused(
        orchestrator
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );
    assert_eq!(paused1.tool_calls[0].id, "call_1");

    let continuation = store.lock().unwrap().take_match(&["call_1".into()]).unwrap();
    let (rt2, store2) = runtime(transport.clone());
    let paused2 = paused(
        orchestrator
            .resume(resume_body(&["call_1"]), rt2, continuation)
            .await
            .unwrap(),
    );
    assert_eq!(paused2.tool_calls[0].id, "call_2");
    let paused2_ids: Vec<&str> = paused2
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::ToolCall(call) => Some(call.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        paused2_ids,
        vec!["call_2"],
        "第二次暂停只能发本轮新调用，不能重放 call_1"
    );

    let continuation = store2.lock().unwrap().take_match(&["call_2".into()]).unwrap();
    assert_eq!(continuation.state.phase, Pig::Pre, "全程都在同一只 pig 里");
    let (rt3, _store3) = runtime(transport.clone());
    let result = completed(
        orchestrator
            .resume(resume_body(&["call_1", "call_2"]), rt3, continuation)
            .await
            .unwrap(),
    );
    assert_eq!(result.text, "做完了");
    assert_eq!(result.path, vec![Pig::Pre], "相位只在第一次推进时记一次");
    assert!(
        result.parts.iter().all(|part| !matches!(part, Part::ToolCall(_))),
        "多轮工具都消费完成后，最终结果不能重放历史 ToolCall"
    );

    let reqs = transport.requests.lock().unwrap();
    let resume1: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();
    let resume2: serde_json::Value = serde_json::from_slice(&reqs[2].body).unwrap();
    assert_eq!(roles(&resume1), vec!["system", "user", "assistant", "tool"]);
    assert_eq!(
        roles(&resume2),
        vec!["system", "user", "assistant", "tool", "assistant", "tool"]
    );
    for body in [&resume1, &resume2] {
        let phase_user = body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(
            phase_user.matches("执行前分析").count(),
            1,
            "同一 Pre pig 内阶段提示只能出现一次"
        );
    }
}

/// Executor 连续工具恢复时，执行计划只在进入 Executor 时注入一次。
#[tokio::test]
async fn executor_tool_resumes_keep_one_phase_prompt() {
    let transport = fake(vec![
        FakeTransport::text(200, "计划A"),
        FakeTransport::tool_calls(200, &[("call_1", "Read")]),
        FakeTransport::tool_calls(200, &[("call_2", "Read")]),
        FakeTransport::text(200, "草稿完成"),
        FakeTransport::text(200, "验收通过\nPIGEND"),
    ]);
    let orchestrator = Orchestrator::new();

    let (rt1, store1) = runtime(transport.clone());
    let paused1 = paused(
        orchestrator
            .run(input(proto::Protocol::OpenAI), rt1)
            .await
            .unwrap(),
    );
    assert_eq!(paused1.tool_calls[0].id, "call_1");

    let continuation1 = store1
        .lock()
        .unwrap()
        .take_match(&["call_1".into()])
        .unwrap();
    assert_eq!(continuation1.state.phase, Pig::Executor);
    let (rt2, store2) = runtime(transport.clone());
    let paused2 = paused(
        orchestrator
            .resume(resume_body(&["call_1"]), rt2, continuation1)
            .await
            .unwrap(),
    );
    assert_eq!(paused2.tool_calls[0].id, "call_2");

    let continuation2 = store2
        .lock()
        .unwrap()
        .take_match(&["call_2".into()])
        .unwrap();
    let (rt3, _store3) = runtime(transport.clone());
    let result = completed(
        orchestrator
            .resume(
                resume_body(&["call_1", "call_2"]),
                rt3,
                continuation2,
            )
            .await
            .unwrap(),
    );
    assert_eq!(result.ended_with, EndedWith::PigEnd);

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 5);
    let exec_first: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();
    let exec_resume1: serde_json::Value = serde_json::from_slice(&reqs[2].body).unwrap();
    let exec_resume2: serde_json::Value = serde_json::from_slice(&reqs[3].body).unwrap();
    assert_eq!(roles(&exec_first), vec!["system", "user"]);
    assert_eq!(
        roles(&exec_resume1),
        vec!["system", "user", "assistant", "tool"]
    );
    assert_eq!(
        roles(&exec_resume2),
        vec!["system", "user", "assistant", "tool", "assistant", "tool"]
    );
    for body in [&exec_first, &exec_resume1, &exec_resume2] {
        let user = body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(
            user.matches("完成对内部信息和外部信息的获取后").count(),
            1,
            "Executor 阶段说明在同一 pig 中必须只有一份"
        );
    }
}

#[tokio::test]
async fn post_pigfail_returns_to_pre_with_failure_paths() {
    let transport = fake(vec![
        FakeTransport::text(200, "计划一"),
        FakeTransport::text(200, "草稿一"),
        FakeTransport::text(200, "走偏了\nPIGFAIL"),
        FakeTransport::text(200, "计划二（这次记住失败路径）"),
        FakeTransport::text(200, "草稿二"),
        FakeTransport::text(200, "通过\nPIGEND"),
    ]);
    let (rt, _store) = runtime(transport.clone());
    let result = completed(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );
    assert_eq!(result.ended_with, EndedWith::PigEnd);
    assert_eq!(
        result.path,
        vec![Pig::Pre, Pig::Executor, Pig::Post, Pig::Pre, Pig::Executor, Pig::Post]
    );

    let reqs = transport.requests.lock().unwrap();
    // 第二次 Pre 的指令里带上了失败路径
    let pre2: serde_json::Value = serde_json::from_slice(&reqs[3].body).unwrap();
    assert!(last_message_content(&pre2).contains("走偏了"));
    // 第二次 Post：产物**逐条**接回（不合并），指令是新的一条 user
    let post2: serde_json::Value = serde_json::from_slice(&reqs[5].body).unwrap();
    assert_eq!(
        roles(&post2),
        vec!["system", "user", "assistant", "assistant", "assistant", "user"],
        "逐条追加：草稿一 / 走偏了 / 草稿二 各占一条"
    );
    assert_eq!(post2["messages"][2]["content"], "草稿一");
    assert_eq!(post2["messages"][3]["content"], "走偏了");
    assert_eq!(post2["messages"][4]["content"], "草稿二");
    assert_eq!(
        post2["messages"][1]["content"], "帮我完成任务Z",
        "原问题不许被覆盖"
    );
}

#[tokio::test]
async fn post_without_marker_retries_post_until_budget() {
    let mut responses = vec![
        FakeTransport::text(200, "计划"),
        FakeTransport::text(200, "草稿"),
    ];
    for _ in 0..4 {
        responses.push(FakeTransport::text(200, "还需改进"));
    }
    let transport = fake(responses);
    let (rt, _store) = runtime(transport.clone());
    let err = Orchestrator::new()
        .run(input(proto::Protocol::OpenAI), rt)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Budget(m) if m.contains("Post 无标记重试")));
    assert_eq!(
        transport.requests.lock().unwrap().len(),
        MAX_POST_ITERATIONS as usize + 3
    );
}

#[tokio::test]
async fn pre_replan_budget_exhausted_is_an_error() {
    let transport = fake(vec![
        FakeTransport::text(200, "计划一\nPIGFAIL"),
        FakeTransport::text(200, "计划二\nPIGFAIL"),
        FakeTransport::text(200, "计划三\nPIGFAIL"),
    ]);
    let (rt, _store) = runtime(transport);
    let err = Orchestrator::new()
        .run(input(proto::Protocol::OpenAI), rt)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Budget(m) if m.contains("Pre 重规划")));
}

#[tokio::test]
async fn upstream_error_stops_immediately() {
    let transport = fake(vec![FakeTransport::json(500, json!({"error": "boom"}))]);
    let (rt, _store) = runtime(transport);
    let err = Orchestrator::new()
        .run(input(proto::Protocol::OpenAI), rt)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Transport(TransportError::Upstream { status: 500, .. })
    ));
}

#[tokio::test]
async fn client_session_header_is_inherited() {
    let transport = fake(vec![FakeTransport::text(200, "答案\nPIGEND")]);
    let (rt, _store) = runtime(transport.clone());
    let mut turn = input(proto::Protocol::OpenAI);
    turn.client_session = Some("client-fixed-session".into());
    let result = completed(Orchestrator::new().run(turn, rt).await.unwrap());
    assert_eq!(result.session, "client-fixed-session");
    let reqs = transport.requests.lock().unwrap();
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
    let transport = fake(vec![FakeTransport::text(200, "答案\nPIGEND")]);
    let (rt, _store) = runtime(transport.clone());
    let result = completed(
        Orchestrator::new()
            .run(input(proto::Protocol::Anthropic), rt)
            .await
            .unwrap(),
    );
    assert_eq!(result.ended_with, EndedWith::SimplePath);
    let body: serde_json::Value =
        serde_json::from_slice(&transport.requests.lock().unwrap()[0].body).unwrap();
    assert_eq!(strip_model(&body), "claude-x");
    let content = last_message_content(&body);
    assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
    // system / tools / tool_choice 一律原样
    assert_eq!(body["system"], "sys");
    assert_eq!(body["tools"][0]["name"], "Bash");
    assert_eq!(body["tool_choice"], json!({"type": "auto"}));
}

/// 流式编排：增量逐字到达并被实时过滤；客户端实时内容 == 最终答复。
#[tokio::test]
async fn streaming_turn_pushes_filtered_deltas_as_they_arrive() {
    let transport = fake(vec![
        FakeTransport::text(200, "分析：需要X\n第二行"),
        FakeTransport::text(200, "草稿\nPIGFAIL"),
        FakeTransport::text(200, "评审：还差一点"),
        FakeTransport::text(200, "继续做完\nPIGEND"),
    ]);
    let frames: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let encoder = Arc::new(Mutex::new(proto::StreamEncoder::new(
        proto::Protocol::OpenAI,
        "gpt-x-pigs",
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
                PigEvent::Thought(text) => encoder.push_reasoning(text),
                PigEvent::ThoughtSummary { item_id, text } => {
                    encoder.push_reasoning_summary(item_id, text)
                }
                PigEvent::ThoughtSignature(signature) => encoder.push_reasoning_signature(signature),
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
    let store = Arc::new(Mutex::new(ContinuationStore::default()));
    let rt = Runtime {
        transport: transport.clone(),
        store,
        progress: Some(sink),
    };
    let result = completed(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );
    frames
        .lock()
        .unwrap()
        .push_str(&encoder.lock().unwrap().finish());

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
    assert_eq!(
        result.text,
        "分析：需要X\n第二行\n\n草稿\n\n评审：还差一点\n\n继续做完"
    );
    let streamed = proto::extract_sse_text(proto::Protocol::OpenAI, &frames.lock().unwrap())
        .unwrap_or_default();
    assert_eq!(streamed, result.text, "客户端边收边拿到的必须与最终答复一致");
    assert!(!streamed.contains("PIGFAIL") && !streamed.contains("PIGEND"));
}

/// 思考要**边想边流**：进度事件里先来 Thought/ThoughtSignature，再来文本；
/// 编码后的客户端流里思考是原生帧，文本里不含思考。
#[tokio::test]
async fn live_thinking_is_streamed_before_text() {
    let transport = fake_with_thinking(
        vec![FakeTransport::text(200, "答案是 4\nPIGEND")],
        "先想一想",
    );
    let frames: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let encoder = Arc::new(Mutex::new(proto::StreamEncoder::new(
        proto::Protocol::Anthropic,
        "claude-x-pigs",
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
                PigEvent::Thought(text) => encoder.push_reasoning(text),
                PigEvent::ThoughtSummary { item_id, text } => {
                    encoder.push_reasoning_summary(item_id, text)
                }
                PigEvent::ThoughtSignature(signature) => encoder.push_reasoning_signature(signature),
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
    let rt = Runtime {
        transport,
        store: Arc::new(Mutex::new(ContinuationStore::default())),
        progress: Some(sink),
    };
    let mut turn = input(proto::Protocol::Anthropic);
    turn.body["stream"] = json!(true);
    let result = completed(Orchestrator::new().run(turn, rt).await.unwrap());
    frames
        .lock()
        .unwrap()
        .push_str(&encoder.lock().unwrap().finish());

    // 事件顺序：思考（含签名）先于文本
    let order: Vec<&'static str> = events
        .lock()
        .unwrap()
        .iter()
        .map(|event| match event {
            PigEvent::Start(_) => "start",
            PigEvent::Thought(_) => "thought",
            PigEvent::ThoughtSummary { .. } => "thought",
            PigEvent::ThoughtSignature(_) => "signature",
            PigEvent::Delta(_) => "delta",
            PigEvent::End(_) => "end",
        })
        .collect();
    let first_thought = order.iter().position(|k| *k == "thought").expect("没有思考事件");
    let first_delta = order.iter().position(|k| *k == "delta").expect("没有文本事件");
    assert!(first_thought < first_delta, "思考必须先于文本: {order:?}");
    assert!(order.iter().any(|k| *k == "signature"), "签名要跟着走");

    // 客户端流：思考是原生帧；文本里不含思考
    let sse = frames.lock().unwrap().clone();
    assert!(sse.contains("thinking_delta") && sse.contains("signature_delta"));
    assert_eq!(
        proto::extract_sse_text(proto::Protocol::Anthropic, &sse).unwrap(),
        "答案是 4",
        "思考不许混进答案文本"
    );
    assert_eq!(result.text, "答案是 4");
}

/// 流式 + 工具调用：文本先流给客户端，暂停时把原生调用作为终止帧发出。
#[tokio::test]
async fn streaming_tool_pause_emits_text_then_native_calls() {
    let transport = fake(vec![FakeTransport::tool_calls(200, &[("call_1", "Bash")])]);
    let frames: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let encoder = Arc::new(Mutex::new(proto::StreamEncoder::new(
        proto::Protocol::OpenAI,
        "gpt-x-pigs",
    )));
    let sink: ProgressSink = {
        let frames = Arc::clone(&frames);
        let encoder = Arc::clone(&encoder);
        Arc::new(move |event| {
            let mut encoder = encoder.lock().unwrap();
            let out = match &event {
                PigEvent::Delta(text) => encoder.push_text(text),
                PigEvent::Thought(text) => encoder.push_reasoning(text),
                PigEvent::ThoughtSummary { item_id, text } => {
                    encoder.push_reasoning_summary(item_id, text)
                }
                PigEvent::ThoughtSignature(signature) => encoder.push_reasoning_signature(signature),
                PigEvent::End(_) => encoder.end_pig(),
                PigEvent::Start(_) => String::new(),
            };
            if !out.is_empty() {
                frames.lock().unwrap().push_str(&out);
            }
        })
    };
    let store = Arc::new(Mutex::new(ContinuationStore::default()));
    let rt = Runtime {
        transport: transport.clone(),
        store: Arc::clone(&store),
        progress: Some(sink),
    };
    let turn = paused(
        Orchestrator::new()
            .run(input(proto::Protocol::OpenAI), rt)
            .await
            .unwrap(),
    );

    // 传输层把这一轮的原生调用也带回来了（供 proxy 发终止帧）
    assert_eq!(turn.tool_calls[0].id, "call_1");
    assert_eq!(store.lock().unwrap().len(), 1);
}
