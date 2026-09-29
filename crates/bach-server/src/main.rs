//! `bach-server` — runs Bach's backend (agents, sessions, Satie) where the agents should run.
//!
//! - `bach-server [serve]`: the server. Clients reach it on a private Unix socket next to the
//!   session database (what `attach` uses), and browsers on a WebSocket at 127.0.0.1:3421 (reach
//!   it from another machine with `ssh -L 3421:localhost:3421 orion`).
//! - `bach-server attach`: connects stdin/stdout to the server, starting it if needed. The desktop
//!   app runs this over SSH: `ssh orion bach-server attach`.
//!
//! Env: BACH_DB (session database, default ~/.local/share/bach/bach.db), BACH_PORT (default
//! 3421), BACH_ALLOWED_ORIGINS (comma-separated browser origins, replaces the defaults).
mod connection;
mod local;
mod ws;

use bach_core::{store::default_db_path, Api};
use local::Paths;
use std::sync::Arc;
use ws::{router, Config};

#[tokio::main]
async fn main() {
    let paths = Paths::for_db(default_db_path());
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None | Some("serve") => serve(paths).await,
        Some("attach") => {
            // Exit right away when either side hangs up: waiting for the runtime to wind down
            // would wait for a stdin read that may never finish.
            match local::attach(&paths).await {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    eprintln!("bach-server attach: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("--version" | "-V") => println!(
            "bach-server {} (protocol {})",
            env!("CARGO_PKG_VERSION"),
            bach_protocol::fingerprint()
        ),
        Some(other) => {
            eprintln!("usage: bach-server [serve | attach | --version] (unknown: {other})");
            std::process::exit(2);
        }
    }
}

async fn serve(paths: Paths) {
    let Some(_lock) = local::lock(&paths).expect("take the server lock") else {
        eprintln!(
            "bach-server is already running for {} (lock: {})",
            paths.db.display(),
            paths.lock.display()
        );
        return;
    };
    let port: u16 = std::env::var("BACH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3421);
    let allowed_origins = match std::env::var("BACH_ALLOWED_ORIGINS") {
        Ok(v) => v.split(',').map(|s| s.trim().to_string()).collect(),
        // The Vite dev page, opened directly or through a tunnel. (The desktop app connects over
        // SSH and the socket instead.)
        Err(_) => ["localhost", "127.0.0.1", "orion", "10.16.20.1"]
            .iter()
            .map(|h| format!("http://{h}:3420"))
            .collect(),
    };
    let api = Arc::new(Api::open(&paths.db).await.expect("open Bach's database"));
    eprintln!("sessions in {}", paths.db.display());
    eprintln!("satie (MCP) on {}", api.satie().url());

    let socket = {
        let (paths, api) = (Paths::for_db(paths.db.clone()), api.clone());
        tokio::spawn(async move {
            if let Err(e) = local::listen(&paths, api).await {
                eprintln!("socket {}: {e}", paths.socket.display());
            }
        })
    };
    eprintln!("bach-server on {}", paths.socket.display());

    // Browsers are optional: without the port, the socket still serves the desktop app.
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => {
            eprintln!("bach-server listening on ws://127.0.0.1:{port}");
            axum::serve(listener, router(Config { allowed_origins }, api))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .unwrap();
        }
        Err(e) => {
            eprintln!("no WebSocket on port {port} ({e}); serving the socket only");
            tokio::select! {
                _ = socket => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    }
    let _ = std::fs::remove_file(&paths.socket);
}
