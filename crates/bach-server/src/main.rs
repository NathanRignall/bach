//! `bach-server` — runs Bach's backend (agents, sessions, background tasks) where the agents should run.
//!
//! - `bach-server [serve]`: the server. Clients reach it on a private Unix socket next to the
//!   session database (what `attach` uses), and browsers on a WebSocket on a second Unix socket,
//!   `ws.sock` (reach it with `ssh -L 3421:/home/<you>/.local/share/bach/ws.sock orion`).
//! - `bach-server attach`: connects stdin/stdout to the server, starting it if needed. The desktop
//!   app runs this over SSH: `ssh orion bach-server attach`.
//!
//! - `bach-server restart`: replaces the running server with this binary's (after an update leaves
//!   an old one running, the app and the server disagree on the protocol).
//!
//! Env: BACH_DB (session database, default ~/.local/share/bach/bach.db), BACH_ALLOWED_ORIGINS (comma-separated browser origins, replaces the defaults),
//! BACH_AGENT_WRAPPER (see `bach_core::wrapper`).
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
        Some("restart") => match local::restart(&paths).await {
            Ok(()) => println!("bach-server restarted"),
            Err(e) => {
                eprintln!("bach-server restart: {e}");
                std::process::exit(1);
            }
        },
        Some("--version" | "-V") => println!(
            "bach-server {} (protocol {})",
            env!("CARGO_PKG_VERSION"),
            bach_protocol::fingerprint()
        ),
        Some(other) => {
            eprintln!("usage: bach-server [serve | attach | restart | --version] (unknown: {other})");
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
    let allowed_origins = match std::env::var("BACH_ALLOWED_ORIGINS") {
        Ok(v) => v.split(',').map(|s| s.trim().to_string()).collect(),
        // The Vite dev page, opened directly or through a tunnel. (The desktop app connects over
        // SSH and the socket instead.)
        Err(_) => ["localhost", "127.0.0.1", "orion", "10.16.20.1"]
            .iter()
            .map(|h| format!("http://{h}:3420"))
            .collect(),
    };
    let api = match Api::open(&paths.db).await {
        Ok(api) => Arc::new(api),
        Err(e) => {
            eprintln!("bach-server: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("sessions in {}", paths.db.display());
    eprintln!("agents' MCP server (bach) on {}", api.tasks().url());

    let socket = {
        let (paths, api) = (Paths::for_db(paths.db.clone()), api.clone());
        tokio::spawn(async move {
            if let Err(e) = local::listen(&paths, api).await {
                eprintln!("socket {}: {e}", paths.socket.display());
            }
        })
    };
    eprintln!("bach-server on {}", paths.socket.display());

    let listener = local::bind_private(&paths.ws_socket).expect("bind the browser socket");
    eprintln!("bach-server (browsers) on {}", paths.ws_socket.display());
    axum::serve(listener, router(Config { allowed_origins }, api))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
    socket.abort();
    let _ = std::fs::remove_file(&paths.ws_socket);
    let _ = std::fs::remove_file(&paths.socket);
}
