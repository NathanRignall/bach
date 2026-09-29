//! The WebSocket bridge: carries [`ClientFrame`] commands to [`Api::call`] and pushes every
//! [`ServerEvent`](bach_protocol::ServerEvent) to the socket.
//!
//! It runs agent CLIs as the current user, so it must only be reachable locally (SSH tunnel) and
//! only from allowed browser origins.
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bach_core::Api;
use bach_protocol::{ApiError, ClientFrame, ServerFrame};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{broadcast, mpsc};

pub struct Config {
    pub allowed_origins: HashSet<String>,
}

struct AppState {
    api: Arc<Api>,
    config: Config,
}

pub fn router(config: Config, api: Arc<Api>) -> Router {
    Router::new()
        .route("/", get(ws_handler))
        .with_state(Arc::new(AppState { api, config }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(st): State<Arc<AppState>>,
) -> Response {
    // Browsers always send Origin on WebSocket handshakes; this stops other web pages from
    // reaching the local port. Non-browser clients (no Origin) are the local user.
    if let Some(origin) = headers.get("origin") {
        if !origin
            .to_str()
            .is_ok_and(|o| st.config.allowed_origins.contains(o))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    ws.on_upgrade(move |socket| serve(socket, st.api.clone()))
}

async fn serve(socket: WebSocket, api: Arc<Api>) {
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<ServerFrame>();

    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let text = serde_json::to_string(&frame).expect("frames serialize");
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut events = api.subscribe();
    let forward = {
        let out = out.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        let _ = out.send(ServerFrame::Event { event });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        })
    };

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let frame = match serde_json::from_str::<ClientFrame>(&text) {
            Ok(f) => f,
            Err(e) => {
                // Answer if we can tell which request it was, so the caller doesn't hang.
                if let Some(id) = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["id"].as_u64())
                {
                    let error = ApiError::invalid(format!("Bad request: {e}"));
                    let _ = out.send(ServerFrame::Error { id, error });
                }
                continue;
            }
        };
        // Each command runs on its own, so a slow one doesn't hold up the rest.
        let (api, out) = (api.clone(), out.clone());
        tokio::spawn(async move {
            let id = frame.id;
            let reply = match api.call(&frame.cmd, frame.args.unwrap_or_default()).await {
                Ok(result) => ServerFrame::Reply { id, result },
                Err(error) => ServerFrame::Error { id, error },
            };
            let _ = out.send(reply);
        });
    }

    forward.abort();
    writer.abort();
}
