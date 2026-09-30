//! The desktop shell. The window talks only to this process: one `rpc` command carries every
//! protocol command, and events arrive on the `bach` channel. Behind that, commands go either to
//! a backend inside the app (agents run on this computer) or over SSH to `bach-server attach` on
//! another machine. `get_connection` / `set_connection` choose, and `bach-connection` events say
//! how the connection is doing.
mod ports;
mod titlebar;

use bach_client::{
    forward::{self, Forwards},
    Remote, Status,
};
use bach_core::Api;
use bach_protocol::{
    app::{Connection, ConnectionState, ConnectionStatus, Forwarding, PortForward},
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
        incompatible: false,
        version: None,
    }
}

/// `ssh <host> <command> attach`, never prompting (there's no terminal to answer in), noticing a
/// dead connection within a minute, and serving as the master that port forwards are added to.
fn ssh_command(host: &str, command: &str) -> Vec<String> {
    let master = forward::master_options(&forward::control_path());
    ["ssh".to_string()]
        .into_iter()
        .chain(master)
        .chain(ssh_args(host, command))
        .collect()
}

fn ssh_args(host: &str, command: &str) -> Vec<String> {
    ssh_args_for(host, command, "attach")
}

fn ssh_args_for(host: &str, command: &str, verb: &str) -> Vec<String> {
    [
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
    .chain([format!("{command} {verb}")])
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
                let dir = self
                    .handle
                    .path()
                    .app_data_dir()
                    .map_err(|e| e.to_string())?;
                let api = Arc::new(Api::open(&dir.join("bach.db")).await?);
                let (mut events, handle) = (api.subscribe(), self.handle.clone());
                let (generation, mine) = (self.generation.clone(), self.local_generation.clone());
                // It keeps running after switching to a remote; its events only show while it's
                // the current backend.
                tauri::async_runtime::spawn(async move {
                    loop {
                        match events.recv().await {
                            Ok(ev) => {
                                if generation.load(Ordering::SeqCst) == mine.load(Ordering::SeqCst)
                                {
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
                self.handle.state::<ports::Ports>().use_forwards(None).await;
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
            Connection::Ssh { host, command, wrapper } => {
                let control = forward::control_path();
                if let Err(e) = forward::prepare_control_dir(&control) {
                    eprintln!("ssh control socket folder: {e}");
                }
                let forwards = Arc::new(Forwards::new("ssh", vec![], host.clone(), control, {
                    let handle = self.handle.clone();
                    move |_| {
                        let handle = handle.clone();
                        tauri::async_runtime::spawn(async move {
                            let ports = handle.state::<ports::Ports>();
                            let _ = handle
                                .emit(bach_protocol::FORWARDS_CHANNEL, ports.snapshot().await);
                        });
                    }
                }));
                self.handle
                    .state::<ports::Ports>()
                    .use_forwards(Some(forwards))
                    .await;
                let (events, statuses) = (self.handle.clone(), self.handle.clone());
                let (g1, g2) = (self.generation.clone(), self.generation.clone());
                let (conn, wrapper) = (connection.clone(), wrapper.clone());
                let remote = Remote::connect(
                    host.clone(),
                    ssh_command(host, command),
                    move |ev: Value| {
                        if g1.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        // Background tasks' ports decide what is forwarded automatically.
                        if ev["topic"] == "task" {
                            let (handle, data) = (events.clone(), ev["data"].clone());
                            tauri::async_runtime::spawn(async move {
                                handle.state::<ports::Ports>().task_event(&data).await;
                            });
                        }
                        let _ = events.emit(EVENT_CHANNEL, ev);
                    },
                    move |s: Status| {
                        if g2.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        let app = statuses.state::<App>();
                        if matches!(s, Status::Connected { .. }) {
                            // (Re)connected: tell the server how to start agents (it may be
                            // new), reopen forwards and catch up on running tasks.
                            let (handle, wrapper) = (statuses.clone(), wrapper.clone());
                            tauri::async_runtime::spawn(async move {
                                let remote = match &*handle.state::<App>().backend.lock().unwrap() {
                                    Some(Backend::Remote(r)) => Some(r.clone()),
                                    _ => None,
                                };
                                let tasks = match remote {
                                    Some(r) => {
                                        let args = serde_json::json!({ "wrapper": wrapper });
                                        if let Err(e) = r.call("set_agent_wrapper", args).await {
                                            eprintln!("agent wrapper `{wrapper}`: {}", e.message);
                                        }
                                        r.call("list_tasks", Value::Null)
                                            .await
                                            .ok()
                                            .and_then(|v| v.as_array().cloned())
                                            .unwrap_or_default()
                                    }
                                    None => vec![],
                                };
                                handle.state::<ports::Ports>().connected(tasks).await;
                            });
                        }
                        app.set_status(match s {
                            Status::Connecting => status(conn.clone(), ConnectionState::Connecting),
                            Status::Connected { version } => ConnectionStatus {
                                version: Some(version),
                                ..status(conn.clone(), ConnectionState::Connected)
                            },
                            Status::Disconnected {
                                error,
                                retrying,
                                incompatible,
                            } => ConnectionStatus {
                                error: Some(error),
                                retrying,
                                incompatible,
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
async fn get_forwarding(ports: State<'_, ports::Ports>) -> Result<Forwarding, ()> {
    Ok(ports.snapshot().await)
}

#[tauri::command]
async fn forward_port(ports: State<'_, ports::Ports>, port: u16) -> Result<PortForward, String> {
    ports.forward(port).await
}

#[tauri::command]
async fn stop_forward(ports: State<'_, ports::Ports>, port: u16) -> Result<(), String> {
    ports.stop(port).await
}

/// Opens a link in the default browser (the webview can't open windows of its own).
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !["http://", "https://", "mailto:"].iter().any(|p| url.starts_with(p)) {
        return Err(format!("won't open {url}: only web and mail links"));
    }
    ports::open_url(&url)
}

#[tauri::command]
async fn open_port(ports: State<'_, ports::Ports>, port: u16) -> Result<(), String> {
    ports.open(port).await
}

#[tauri::command]
async fn set_auto_forward(ports: State<'_, ports::Ports>, auto: bool) -> Result<(), String> {
    ports.set_auto(auto).await
}

/// Replaces the server on the SSH host with the one installed there (`bach-server restart`), then
/// connects again. For when the app and the server disagree on the protocol because an update
/// left an old server running.
#[tauri::command]
async fn restart_server(app: State<'_, App>) -> Result<(), String> {
    let connection = load_connection(&app.config);
    let Connection::Ssh { host, command, .. } = &connection else {
        return Err("This app runs its own backend; relaunch it instead.".into());
    };
    let mut ssh = tokio::process::Command::new("ssh");
    ssh.args(ssh_args_for(host, command, "restart"))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(45), ssh.output())
        .await
        .map_err(|_| format!("{host} didn't restart bach-server in time."))?
        .map_err(|e| format!("Couldn't run ssh: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.trim().lines().rev().take(4).collect();
        return Err(tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    app.connect(connection).await;
    Ok(())
}

/// Starts the app over, for when a new build is installed but the old one is still running.
#[tauri::command]
fn relaunch(app: AppHandle) {
    app.restart();
}

#[tauri::command]
fn get_connection(app: State<'_, App>) -> ConnectionStatus {
    app.status.lock().unwrap().clone()
}

#[tauri::command]
async fn set_connection(app: State<'_, App>, connection: Connection) -> Result<(), String> {
    let connection = match connection {
        Connection::Ssh {
            host,
            command,
            wrapper,
        } => {
            let host = host.trim().to_string();
            // Passed to ssh as the destination: never let it look like an option.
            if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
                return Err(format!("`{host}` isn't a host ssh can connect to."));
            }
            let command = command.trim();
            bach_core::wrapper::parse(&wrapper)?;
            Connection::Ssh {
                host,
                command: if command.is_empty() {
                    bach_protocol::app::default_command()
                } else {
                    command.to_string()
                },
                wrapper: wrapper.trim().to_string(),
            }
        }
        local => local,
    };
    if let Some(dir) = app.config.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &app.config,
        serde_json::to_string_pretty(&connection).unwrap(),
    )
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
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                titlebar::install(&window)?;
            }
            let config_dir = app.path().app_config_dir()?;
            app.manage(ports::Ports::new(
                app.handle().clone(),
                config_dir.join("forwarding.json"),
            ));
            let config = config_dir.join("connection.json");
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
        .invoke_handler(tauri::generate_handler![
            rpc,
            get_connection,
            set_connection,
            restart_server,
            relaunch,
            get_forwarding,
            forward_port,
            stop_forward,
            open_port,
            open_url,
            set_auto_forward,
            titlebar::title_bar
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bach");
}
