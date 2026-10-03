//! The WebSocket bridge, for browsers: frames are WebSocket text messages (see
//! [`crate::connection`]).
//!
//! It runs agent CLIs as the current user, so it listens on a Unix socket only this user can open
//! (not a TCP port, which every user on the machine could reach) and only accepts allowed browser
//! origins. A browser gets to it through `ssh -L <port>:<ws.sock path> <host>`.
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
use bach_protocol::ServerFrame;
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::mpsc;

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
    let (mut sink, stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<ServerFrame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let text = serde_json::to_string(&frame).expect("frames serialize");
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let incoming = stream
        .take_while(|m| std::future::ready(m.is_ok()))
        .filter_map(|m| async move {
            match m {
                Ok(Message::Text(t)) => Some(t.to_string()),
                _ => None,
            }
        });
    crate::connection::serve(api, Box::pin(incoming), out).await;
    let _ = writer.await;
}
