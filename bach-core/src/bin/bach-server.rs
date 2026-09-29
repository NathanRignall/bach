//! `bach-server` — serves Bach's commands over a WebSocket on 127.0.0.1 for browser use.
//!
//! Reach it from another machine with `ssh -L 3421:localhost:3421 orion`.
//! Env: BACH_PORT (default 3421), BACH_ALLOWED_ORIGINS (comma-separated, replaces defaults).
use bach_core::server::{router, Config};

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("BACH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3421);
    let allowed_origins = match std::env::var("BACH_ALLOWED_ORIGINS") {
        Ok(v) => v.split(',').map(|s| s.trim().to_string()).collect(),
        // The Vite dev page, opened directly or through a tunnel.
        Err(_) => ["localhost", "127.0.0.1", "orion", "10.16.20.1"]
            .iter()
            .map(|h| format!("http://{h}:3420"))
            .collect(),
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    eprintln!("bach-server listening on ws://127.0.0.1:{port}");
    axum::serve(listener, router(Config { allowed_origins }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}
