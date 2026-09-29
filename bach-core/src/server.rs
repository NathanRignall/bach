//! WebSocket bridge so a browser can drive Bach without the Tauri shell.
//! Runs agent CLIs as the current user, so it must only be reachable locally (SSH tunnel)
//! and only from allowed browser origins.
use crate::{
    git::Git,
    list_agents,
    protocol::{Envelope, Request},
    runs::{Emit, Runs},
    store::Store,
};
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
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{broadcast, mpsc};

pub struct Config {
    pub allowed_origins: HashSet<String>,
    /// Where new git worktrees are created.
    pub worktrees_dir: std::path::PathBuf,
}

struct AppState {
    runs: Runs,
    git: Git,
    store: Store,
    events: broadcast::Sender<Value>,
    emit: Emit,
    config: Config,
}

pub fn router(config: Config, store: Store) -> Router {
    let (events, _) = broadcast::channel(1024);
    let emit: Emit = {
        let events = events.clone();
        Arc::new(move |ev| {
            if let Ok(v) = serde_json::to_value(&ev) {
                let _ = events.send(v);
            }
        })
    };
    let state = Arc::new(AppState {
        runs: Runs::default(),
        git: Git::new(config.worktrees_dir.clone()),
        store,
        events,
        emit,
        config,
    });
    Router::new().route("/", get(ws_handler)).with_state(state)
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
    ws.on_upgrade(move |socket| serve(socket, st))
}

async fn serve(socket: WebSocket, st: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();

    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m.into())).await.is_err() {
                break;
            }
        }
    });

    let mut events = st.events.subscribe();
    let forward = {
        let out = out.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(payload) => {
                        let _ = out.send(
                            json!({ "event": "agent-event", "payload": payload }).to_string(),
                        );
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        })
    };

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let env = match serde_json::from_str::<Envelope>(&text) {
            Ok(env) => env,
            Err(e) => {
                // Answer if we can tell which request it was, so the caller doesn't hang.
                if let Some(id) = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["id"].as_u64())
                {
                    let _ = out.send(
                        json!({ "id": id, "ok": false, "error": format!("bad request: {e}") })
                            .to_string(),
                    );
                }
                continue;
            }
        };
        let reply = match handle(&st, env.request).await {
            Ok(result) => json!({ "id": env.id, "ok": true, "result": result }),
            Err(error) => json!({ "id": env.id, "ok": false, "error": error }),
        };
        let _ = out.send(reply.to_string());
    }

    forward.abort();
    writer.abort();
}

async fn handle(st: &AppState, req: Request) -> Result<Value, String> {
    match req {
        Request::ListAgents => Ok(serde_json::to_value(list_agents()).unwrap()),
        Request::StartRun {
            agent,
            prompt,
            cwd,
            session_id,
            model,
        } => st
            .runs
            .start(st.emit.clone(), agent, prompt, cwd, session_id, model)
            .await
            .map(Value::String),
        Request::ListDir { path, show_hidden } => crate::fs::list_dir(path.as_deref(), show_hidden)
            .map(|l| serde_json::to_value(l).unwrap()),
        Request::GitInfo { path } => st
            .git
            .info(path)
            .await
            .map(|i| serde_json::to_value(i).unwrap()),
        Request::PrepareWorkspace {
            cwd,
            branch,
            worktree,
            new_branch,
        } => st
            .git
            .prepare(cwd, branch, worktree, new_branch)
            .await
            .map(|w| serde_json::to_value(w).unwrap()),
        Request::ListWorktrees => Ok(serde_json::to_value(st.git.list_worktrees().await).unwrap()),
        Request::RemoveWorktree {
            path,
            discard,
            delete_branch,
        } => st
            .git
            .remove_worktree(path, discard, delete_branch)
            .await
            .map(|_| Value::Null),
        Request::ListSessions => st.store.list().map(Value::Array),
        Request::SaveSession { session } => st.store.save(&session).map(|_| Value::Null),
        Request::DeleteSession { session_id } => st.store.delete(&session_id).map(|_| Value::Null),
        Request::CancelRun { run_id } => {
            st.runs.cancel(&run_id).await;
            Ok(Value::Null)
        }
    }
}
