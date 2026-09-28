from pathlib import Path
p = Path('crates/pigs/src/server.rs')
t = p.read_text(encoding='utf-8')

t = t.replace(
'''use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use pigs_core::Message;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::runtime::PhasedRuntime;
''',
'''use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream;
use pigs_core::Message;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::runtime::{PhasedRuntime, ProgressSink, TurnProgress};
'''
)

t = t.replace(
'''        "mode": "phased-api"
''',
'''        "mode": "phased-api",
        "stream": true
'''
)

t = t.replace(
'    println!("  POST /v1/chat/completions");',
'    println!("  POST /v1/chat/completions  (stream=true|false)");'
)

old = '''async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.stream.unwrap_or(false) {
        return Err((
            StatusCode::BAD_REQUEST,
            "stream=true not implemented yet; call with stream=false".into(),
        ));
    }

    let (history, question) = split_history_and_question(&req.messages).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid messages: {e}"),
        )
    })?;

    let result = state
        .runtime
        .run_turn(&history, &question)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn failed: {e:#}")))?;
'''
new = '''async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionsRequest>,
) -> Result<Response, (StatusCode, String)> {
    let (history, question) = split_history_and_question(&req.messages).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid messages: {e}"),
        )
    })?;

    let model = req.model.clone().unwrap_or_else(|| "pigs".into());
    if req.stream.unwrap_or(false) {
        return Ok(stream_chat(state, history, question, model).into_response());
    }

    let result = state
        .runtime
        .run_turn(&history, &question)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn failed: {e:#}")))?;
'''
if old not in t:
    raise SystemExit('chat_completions block missing')
t = t.replace(old, new, 1)

t = t.replace(
'''        model: req.model.unwrap_or_else(|| "pigs".into()),
''',
'''        model,
'''
)
t = t.replace('    Ok(Json(resp))\n}', '    Ok(Json(resp).into_response())\n}')

anchor = 'fn split_history_and_question'
if 'fn stream_chat(' not in t:
    helper = r'''
fn stream_chat(
    state: AppState,
    history: Vec<Message>,
    question: String,
    model: String,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();
    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let _ = tx.send(Ok(Event::default().data(role_json(&id, created, &model))));

    let tx_p = tx.clone();
    let id_p = id.clone();
    let model_p = model.clone();
    let sink: ProgressSink = Arc::new(move |p: TurnProgress| match p {
        TurnProgress::PhaseStart { phase } => {
            let _ = tx_p.send(Ok(Event::default().event("pigs.phase").data(phase)));
        }
        TurnProgress::TextDelta { text, .. } => {
            if !text.is_empty() {
                let _ = tx_p.send(Ok(Event::default().data(content_json(
                    &id_p, created, &model_p, &text,
                ))));
            }
        }
        TurnProgress::ToolStart { phase, name } => {
            let _ = tx_p.send(Ok(Event::default()
                .event("pigs.tool")
                .data(format!("start {phase} {name}"))));
        }
        TurnProgress::ToolEnd {
            phase,
            name,
            is_error,
        } => {
            let _ = tx_p.send(Ok(Event::default()
                .event("pigs.tool")
                .data(format!("end {phase} {name} error={is_error}"))));
        }
        _ => {}
    });

    let runtime = Arc::clone(&state.runtime);
    let tx_end = tx;
    let id_e = id;
    let model_e = model;
    tokio::spawn(async move {
        let result = runtime
            .run_turn_with_progress(&history, &question, Some(sink))
            .await;
        match result {
            Ok(r) => {
                let phases: Vec<String> = r
                    .events
                    .iter()
                    .filter(|e| e.kind == "phase_start")
                    .filter_map(|e| e.phase.clone())
                    .collect();
                if phases.len() == 1 && phases.first().map(|s| s.as_str()) == Some("pre") {
                    let _ = tx_end.send(Ok(Event::default().data(content_json(
                        &id_e, created, &model_e, &r.final_text,
                    ))));
                }
                let _ = tx_end.send(Ok(Event::default().data(stop_json(&id_e, created, &model_e))));
                let _ = tx_end.send(Ok(Event::default().event("pigs.done").data(r.ended_with)));
                let _ = tx_end.send(Ok(Event::default().data("[DONE]")));
            }
            Err(e) => {
                let msg = format!("turn failed: {e:#}");
                let _ = tx_end.send(Ok(Event::default().data(content_json(
                    &id_e, created, &model_e, &msg,
                ))));
                let _ = tx_end.send(Ok(Event::default().data(stop_json(&id_e, created, &model_e))));
                let _ = tx_end.send(Ok(Event::default().data("[DONE]")));
            }
        }
    });

    let s = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(item) => Some((item, rx)),
            None => None,
        }
    });
    Sse::new(s).keep_alive(KeepAlive::default())
}

fn role_json(id: &str, created: i64, model: &str) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant"},
            "finish_reason": null
        }]
    })
    .to_string()
}

fn content_json(id: &str, created: i64, model: &str, content: &str) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"content": content},
            "finish_reason": null
        }]
    })
    .to_string()
}

fn stop_json(id: &str, created: i64, model: &str) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    })
    .to_string()
}

'''
    if anchor not in t:
        raise SystemExit('anchor missing')
    t = t.replace(anchor, helper + anchor, 1)

p.write_text(t, encoding='utf-8')
print('ok', 'stream_chat' in t, 'ProgressSink' in t)
