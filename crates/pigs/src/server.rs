//! Local OpenAI-compatible HTTP API host (default `pigs` mode).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream;
use serde::Serialize;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tracing::info;

use pigs_cli::phased_api_convert::{run_converted_turn_arc, ChatCompletionsRequest, ConvertedTurn};
use pigs_cli::phased_runtime::{PhasedRuntime, ProgressSink, TurnProgress};

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<PhasedRuntime>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionsResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pigs: Option<PigsMeta>,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: OutMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct OutMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct PigsMeta {
    pub ended_with: String,
    pub phases: Vec<String>,
}

pub async fn serve(runtime: PhasedRuntime, host: &str, port: u16) -> anyhow::Result<()> {
    let state = AppState {
        runtime: Arc::new(runtime),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address {host}:{port}: {e}"))?;
    info!(%addr, "pigs local API listening");
    println!("pigs API listening on http://{addr}");
    println!("  GET  /health");
    println!("  GET  /v1/models");
    println!("  POST /v1/chat/completions  (stream=true|false)");
    println!("Default bind is loopback-only. Use --host to change.");
    println!("Core conversion is shared with  /  (api_convert).");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "pigs",
        "mode": "phased-api",
        "stream": true
    }))
}

async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "object": "list",
        "data": [{
            "id": state.runtime.wrapped_model.clone(),
            "object": "model",
            "owned_by": "pigs",
            "root": state.runtime.remote_model.clone(),
            "phases": "pre -> executor -> post",
        }]
    }))
}

async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionsRequest>,
) -> Result<Response, (StatusCode, String)> {
    let converted = ConvertedTurn::from_request(&req).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid messages: {e}"),
        )
    })?;

    if converted.stream {
        return Ok(stream_chat(state, converted).into_response());
    }

    let result = run_converted_turn_arc(&state.runtime, &converted, None)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn failed: {e:#}")))?;

    let phases: Vec<String> = result
        .events
        .iter()
        .filter(|e| e.kind == "phase_start")
        .filter_map(|e| e.phase.clone())
        .collect();

    let created = chrono::Utc::now().timestamp();
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let completion_tokens = (result.final_text.len() / 4) as u32;
    let resp = ChatCompletionsResponse {
        id,
        object: "chat.completion",
        created,
        model: state.runtime.wrapped_model.clone(),
        choices: vec![Choice {
            index: 0,
            message: OutMessage {
                role: "assistant",
                content: result.final_text,
            },
            finish_reason: "stop".into(),
        }],
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens,
            total_tokens: completion_tokens,
        },
        pigs: Some(PigsMeta {
            ended_with: result.ended_with,
            phases,
        }),
    };
    Ok(Json(resp).into_response())
}


fn stream_chat(
    state: AppState,
    converted: ConvertedTurn,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();
    let model = state.runtime.wrapped_model.clone();
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
        let result = run_converted_turn_arc(&runtime, &converted, Some(sink)).await;
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

