//! One client connection, whatever carries it (a WebSocket, or the Unix socket behind
//! `bach-server attach`): says hello, runs each command it sends, and pushes every event.
use bach_core::Api;
use bach_protocol::{fingerprint, ApiError, ClientFrame, ServerFrame};
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

/// The first frame on every connection.
pub fn hello() -> ServerFrame {
    ServerFrame::Hello {
        protocol: fingerprint(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

/// Serves a connection until `incoming` ends. Frames to send go to `out`, starting with the hello.
pub async fn serve(
    api: Arc<Api>,
    mut incoming: impl Stream<Item = String> + Unpin,
    out: mpsc::UnboundedSender<ServerFrame>,
) {
    let _ = out.send(hello());
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

    while let Some(text) = incoming.next().await {
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
}
