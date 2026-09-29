//! `bach-server` — serves Bach's commands over a WebSocket on 127.0.0.1 for browser use.
//!
//! Reach it from another machine with `ssh -L 3421:localhost:3421 orion`.
//! Env: BACH_PORT (default 3421), BACH_ALLOWED_ORIGINS (comma-separated, replaces defaults),
//! BACH_DB (session database, default ~/.local/share/bach/bach.db).
use bach_core::{
    satie::Satie,
    server::{router, Config},
    store::{default_db_path, Store},
};

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("BACH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3421);
    let allowed_origins = match std::env::var("BACH_ALLOWED_ORIGINS") {
        Ok(v) => v.split(',').map(|s| s.trim().to_string()).collect(),
        // The Vite dev page (opened directly or through a tunnel) and the Tauri app's
        // webview origins (macOS uses tauri://localhost, Linux/Windows http://tauri.localhost).
        Err(_) => ["localhost", "127.0.0.1", "orion", "10.16.20.1"]
            .iter()
            .map(|h| format!("http://{h}:3420"))
            .chain(
                [
                    "tauri://localhost",
                    "http://tauri.localhost",
                    "https://tauri.localhost",
                ]
                .map(String::from),
            )
            .collect(),
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    let db = default_db_path();
    let worktrees_dir = db
        .parent()
        .map_or_else(|| ".".into(), |p| p.join("worktrees"));
    let store = Store::open(&db).expect("open session database");
    eprintln!("sessions in {}", db.display());
    // Bach's own MCP server for agents (any free loopback port; agents are handed the URL).
    let satie = Satie::start("127.0.0.1:0".parse().unwrap())
        .await
        .expect("start satie");
    eprintln!("satie (MCP) on {}", satie.url());
    eprintln!("bach-server listening on ws://127.0.0.1:{port}");
    axum::serve(
        listener,
        router(
            Config {
                allowed_origins,
                worktrees_dir,
            },
            store,
            Some(satie),
        ),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .unwrap();
}
