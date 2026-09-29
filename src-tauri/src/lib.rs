//! The desktop shell. The window talks only to this process: one `rpc` command carries every
//! protocol command, and events arrive on the `bach` channel. Behind that, commands go either to
//! a backend inside the app (agents run on this computer) or over SSH to `bach-server attach` on
//! another machine. `get_connection` / `set_connection` choose, and `bach-connection` events say
//! how the connection is doing.
use bach_client::{Remote, Status};
use bach_core::Api;
use bach_protocol::{
    app::{Connection, ConnectionState, ConnectionStatus},
    ApiError, ErrorCode, CONNECTION_CHANNEL, EVENT_CHANNEL,
};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{broadcast::error::RecvError, OnceCell};

enum Backend {
    Local(Arc<Api>),
    Remote(Arc<Remote>),
}

struct App {
    handle: AppHandle,
    /// Where the connection setting is saved.
    config: PathBuf,
    /// This computer's backend, opened the first time it's used and kept from then on.
    local: OnceCell<Arc<Api>>,
    backend: Mutex<Option<Backend>>,
    status: Mutex<ConnectionStatus>,
    /// Which connection is current; events from an earlier one are dropped.
    generation: Arc<AtomicU64>,
    /// The generation in which the local backend is the current one.
    local_generation: Arc<AtomicU64>,
}

fn status(connection: Connection, state: ConnectionState) -> ConnectionStatus {
    ConnectionStatus {
        connection,
        state,
        error: None,
        retrying: false,
        version: None,
    }
}

/// `ssh <host> <command> attach`, never prompting (there's no terminal to answer in) and
/// noticing a dead connection within a minute.
fn ssh_command(host: &str, command: &str) -> Vec<String> {
    [
        "ssh",
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        host,
        "--",
    ]
    .into_iter()
    .map(String::from)
    .chain([format!("{command} attach")])
    .collect()
}

impl App {
    fn set_status(&self, s: ConnectionStatus) {
        *self.status.lock().unwrap() = s.clone();
        let _ = self.handle.emit(CONNECTION_CHANNEL, s);
    }

    async fn local(&self) -> Result<Arc<Api>, String> {
        self.local
            .get_or_try_init(|| async {
                let dir = self.handle.path().app_data_dir().map_err(|e| e.to_string())?;
                let api = Arc::new(Api::open(&dir.join("bach.db")).await?);
                let (mut events, handle) = (api.subscribe(), self.handle.clone());
                let (generation, mine) = (self.generation.clone(), self.local_generation.clone());
                // It keeps running after switching to a remote; its events only show while it's
                // the current backend.
                tauri::async_runtime::spawn(async move {
                    loop {
                        match events.recv().await {
                            Ok(ev) => {
                                if generation.load(Ordering::SeqCst) == mine.load(Ordering::SeqCst) {
                                    let _ = handle.emit(EVENT_CHANNEL, ev);
                                }
                            }
                            Err(RecvError::Lagged(_)) => continue,
                            Err(RecvError::Closed) => break,
                        }
                    }
                });
                Ok(api)
            })
            .await
            .cloned()
    }

    /// Switches to `connection`, dropping the previous one.
    async fn connect(&self, connection: Connection) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.backend.lock().unwrap() = None;
        match &connection {
            Connection::Local => {
                self.set_status(status(connection.clone(), ConnectionState::Connecting));
                match self.local().await {
                    Ok(api) => {
                        self.local_generation.store(generation, Ordering::SeqCst);
                        *self.backend.lock().unwrap() = Some(Backend::Local(api));
                        self.set_status(status(connection, ConnectionState::Connected));
                    }
                    Err(e) => self.set_status(ConnectionStatus {
                        error: Some(e),
                        ..status(connection, ConnectionState::Disconnected)
                    }),
                }
            }
            Connection::Ssh { host, command } => {
                let (events, statuses) = (self.handle.clone(), self.handle.clone());
                let (g1, g2) = (self.generation.clone(), self.generation.clone());
                let conn = connection.clone();
                let remote = Remote::connect(
                    host.clone(),
                    ssh_command(host, command),
                    move |ev: Value| {
                        if g1.load(Ordering::SeqCst) == generation {
                            let _ = events.emit(EVENT_CHANNEL, ev);
                        }
                    },
                    move |s: Status| {
                        if g2.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        let app = statuses.state::<App>();
                        app.set_status(match s {
                            Status::Connecting => status(conn.clone(), ConnectionState::Connecting),
                            Status::Connected { version } => ConnectionStatus {
                                version: Some(version),
                                ..status(conn.clone(), ConnectionState::Connected)
                            },
                            Status::Disconnected { error, retrying } => ConnectionStatus {
                                error: Some(error),
                                retrying,
                                ..status(conn.clone(), ConnectionState::Disconnected)
                            },
                        });
                    },
                );
                *self.backend.lock().unwrap() = Some(Backend::Remote(Arc::new(remote)));
            }
        }
    }
}

fn load_connection(path: &PathBuf) -> Connection {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Connection::Local)
}

#[tauri::command]
async fn rpc(app: State<'_, App>, name: String, args: Option<Value>) -> Result<Value, ApiError> {
    let args = args.unwrap_or_default();
    let backend = match &*app.backend.lock().unwrap() {
        Some(Backend::Local(api)) => Some(Backend::Local(api.clone())),
        Some(Backend::Remote(r)) => Some(Backend::Remote(r.clone())),
        None => None,
    };
    match backend {
        Some(Backend::Local(api)) => api.call(&name, args).await,
        Some(Backend::Remote(remote)) => remote.call(&name, args).await,
        None => Err(ApiError::new(
            ErrorCode::Unavailable,
            app.status
                .lock()
                .unwrap()
                .error
                .clone()
                .unwrap_or_else(|| "Not connected yet.".into()),
        )),
    }
}

#[tauri::command]
fn get_connection(app: State<'_, App>) -> ConnectionStatus {
    app.status.lock().unwrap().clone()
}

#[tauri::command]
async fn set_connection(app: State<'_, App>, connection: Connection) -> Result<(), String> {
    let connection = match connection {
        Connection::Ssh { host, command } => {
            let host = host.trim().to_string();
            // Passed to ssh as the destination: never let it look like an option.
            if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
                return Err(format!("`{host}` isn't a host ssh can connect to."));
            }
            let command = command.trim();
            Connection::Ssh {
                host,
                command: if command.is_empty() {
                    bach_protocol::app::default_command()
                } else {
                    command.to_string()
                },
            }
        }
        local => local,
    };
    if let Some(dir) = app.config.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&app.config, serde_json::to_string_pretty(&connection).unwrap())
        .map_err(|e| e.to_string())?;
    app.connect(connection).await;
    Ok(())
}

/// Apps started from the Finder get a bare PATH, without Homebrew, Nix or ~/.local/bin, so
/// `claude`, `git` and whatever `ssh` needs (a ProxyCommand) wouldn't be found. Take the PATH a
/// login shell sets up instead.
#[cfg(target_os = "macos")]
fn use_login_shell_path() {
    use std::{process::Command, sync::mpsc, time::Duration};
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = Command::new(shell)
            .args(["-l", "-c", "printf '%s' \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    // A shell profile that hangs mustn't hang the app.
    if let Ok(Ok(out)) = rx.recv_timeout(Duration::from_secs(5)) {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !path.is_empty() {
            std::env::set_var("PATH", path);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    use_login_shell_path();

    tauri::Builder::default()
        .setup(|app| {
            let config = app.path().app_config_dir()?.join("connection.json");
            let connection = load_connection(&config);
            app.manage(App {
                handle: app.handle().clone(),
                config,
                local: OnceCell::new(),
                backend: Mutex::new(None),
                status: Mutex::new(status(connection.clone(), ConnectionState::Connecting)),
                generation: Arc::new(AtomicU64::new(0)),
                local_generation: Arc::new(AtomicU64::new(u64::MAX)),
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                handle.state::<App>().connect(connection).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc, get_connection, set_connection])
        .run(tauri::generate_context!())
        .expect("error while running Bach");
}
