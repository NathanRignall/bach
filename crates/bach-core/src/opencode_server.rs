//! The `opencode serve`s Bach runs: one per session folder, started in that folder on first use
//! (opencode may be a wrapper that sandboxes its writes to the folder it starts in), on loopback
//! with a random port and password. It's restarted if it goes away, and stopped once it has had
//! no runs for a while. Sessions in the same folder share it.
//!
//! It must not outlive Bach, however Bach ends (a restart is a SIGTERM; nothing gets to clean
//! up). So it runs under a small shell that holds Bach's end of a pipe and stops the server
//! when the pipe closes, which happens whenever Bach's process does.
use crate::adapters::{opencode, MCP_SERVER};
use bach_protocol::ModelInfo;
use serde_json::Value;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{broadcast, Mutex},
};

/// A running server. Cheap to clone.
#[derive(Clone)]
pub struct Server {
    base: String,
    password: String,
    http: reqwest::Client,
    events: broadcast::Sender<Value>,
    alive: Arc<AtomicBool>,
    /// Runs using it right now, and when it was last used: an idle server is stopped.
    runs: Arc<AtomicUsize>,
    last_used: Arc<std::sync::Mutex<Instant>>,
    /// Its bach-tasks token, scoped to its folder; revoked once the server is gone.
    tasks: Option<Arc<bach_tasks::Grant>>,
}

/// Held by a run while it uses a server, so the server isn't stopped under it.
pub struct Lease(Server);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.touch();
        self.0.runs.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Bach's end of the pipe keeping the server alive (see the module docs); taking it stops it.
type Lifeline = Arc<Mutex<Option<tokio::process::ChildStdin>>>;

type Servers = Mutex<HashMap<String, (Server, Lifeline)>>;

/// The servers, by folder.
fn servers() -> &'static Servers {
    static SERVERS: OnceLock<Servers> = OnceLock::new();
    SERVERS.get_or_init(|| {
        tokio::spawn(stop_idle());
        Mutex::default()
    })
}

/// How long a server with no runs is kept for the next turn.
const IDLE: Duration = Duration::from_secs(10 * 60);

/// Stops servers that have been idle for [`IDLE`], and forgets ones that went away.
async fn stop_idle() {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut stopped = vec![];
        servers().lock().await.retain(|_, (s, lifeline)| {
            let idle = s.runs.load(Ordering::SeqCst) == 0
                && s.last_used.lock().unwrap().elapsed() > IDLE;
            if idle || !s.alive.load(Ordering::SeqCst) {
                stopped.push(lifeline.clone());
                return false;
            }
            true
        });
        for lifeline in stopped {
            lifeline.lock().await.take();
        }
    }
}

/// Runs `opencode serve` (through the wrapper, if any) until its stdin (Bach's pipe) closes.
fn supervise() -> String {
    format!(
        "{} serve --port 0 --hostname 127.0.0.1 & pid=$!; \
        cat > /dev/null; kill $pid 2>/dev/null; wait $pid",
        crate::wrapper::shell_command("opencode")
    )
}

/// What a run sees when the server has gone away; its turn is over.
pub const LOST: &str = "bach.server.lost";

/// The server for folder `dir`, starting it there if it isn't running. `tasks`, if given, is
/// offered to it as Bach's MCP server (see [`Server::offer_mcp`]).
pub async fn server(dir: &str, tasks: Option<&bach_tasks::Tasks>) -> Result<Server, String> {
    let mut all = servers().lock().await;
    if let Some((s, _)) = all.get(dir) {
        if s.alive.load(Ordering::SeqCst) {
            s.touch();
            return Ok(s.clone());
        }
    }
    let (server, lifeline) = start(dir, tasks).await?;
    all.insert(dir.to_string(), (server.clone(), lifeline));
    Ok(server)
}

async fn start(dir: &str, tasks: Option<&bach_tasks::Tasks>) -> Result<(Server, Lifeline), String> {
    if !crate::wrapper::installed("opencode") {
        return Err("opencode isn't installed on the machine running the agents.".into());
    }
    let password = uuid::Uuid::new_v4().to_string();
    let grant = tasks.map(|s| {
        Arc::new(s.grant(bach_tasks::Scope { project: Some(dir.to_string()), owner: None }))
    });
    let mut child = Command::new("sh")
        .args(["-c", &supervise()])
        .env("OPENCODE_SERVER_PASSWORD", &password)
        .envs(crate::runs::no_proxy_for_loopback())
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start `opencode serve`: {e}"))?;
    let lifeline = Arc::new(Mutex::new(child.stdin.take()));

    // It says where it listens once it's ready (the first start also migrates its database).
    let mut out = BufReader::new(child.stdout.take().expect("piped")).lines();
    let base = tokio::time::timeout(Duration::from_secs(120), async {
        while let Ok(Some(line)) = out.next_line().await {
            if let Some(url) = line.split("listening on ").nth(1) {
                return Some(url.trim().trim_end_matches('/').to_string());
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or("`opencode serve` didn't start.")?;
    tokio::spawn(async move {
        while let Ok(Some(_)) = out.next_line().await {}
        // Reap the shell once it's done.
        let _ = child.wait().await;
    });

    let (events, _) = broadcast::channel(4096);
    let server = Server {
        base,
        password,
        http: reqwest::Client::new(),
        events,
        alive: Arc::new(AtomicBool::new(true)),
        runs: Arc::default(),
        last_used: Arc::new(std::sync::Mutex::new(Instant::now())),
        tasks: grant,
    };
    // One stream of every project's events, passed on to the runs.
    let s = server.clone();
    let watched = lifeline.clone();
    tokio::spawn(async move {
        let result = s.read_events().await;
        s.alive.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            eprintln!("opencode server: {e}");
        }
        let _ = s.events.send(serde_json::json!({ "type": LOST }));
        // Whatever is left of it goes; the next run starts a new one.
        watched.lock().await.take();
    });
    Ok((server, lifeline))
}

impl Server {
    /// Keeps the server while the lease lives (a run).
    pub fn lease(&self) -> Lease {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Lease(self.clone())
    }

    /// Adds Bach's MCP server ([`MCP_SERVER`]) for folder `dir`, unless opencode already has it,
    /// and returns the names of the folder's MCP servers. Added over HTTP rather than in its
    /// config: opencode may be a wrapper that sets `OPENCODE_CONFIG_CONTENT` itself, and a
    /// folder's MCP servers go if opencode reloads it, so this is checked every turn. The server
    /// lasts across runs, so its token is the folder's, not a run's.
    pub async fn offer_mcp(&self, dir: &str) -> Result<Vec<String>, String> {
        let names = |status: &Value| -> Vec<String> {
            status.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect()
        };
        let status = self.get("/mcp", Some(dir)).await?;
        let Some(grant) = &self.tasks else { return Ok(names(&status)) };
        if status[MCP_SERVER]["status"] == "connected" {
            return Ok(names(&status));
        }
        let body = serde_json::json!({ "name": MCP_SERVER, "config": {
            "type": "remote",
            "url": grant.url,
            "headers": { "Authorization": format!("Bearer {}", grant.token) },
        }});
        Ok(names(&self.post("/mcp", dir, &body).await?))
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    fn request(&self, method: reqwest::Method, path: &str, directory: Option<&str>) -> reqwest::RequestBuilder {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("opencode", Some(&self.password));
        if let Some(d) = directory {
            r = r.query(&[("directory", d)]);
        }
        r
    }

    async fn send(&self, r: reqwest::RequestBuilder) -> Result<Value, String> {
        let res = r.send().await.map_err(|e| format!("opencode didn't answer: {e}"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let v: Value = serde_json::from_str(&text).unwrap_or_default();
            let why = v["data"]["message"]
                .as_str()
                .or(v["message"].as_str())
                .or(v["error"].as_str())
                .map(String::from)
                .unwrap_or(text);
            return Err(format!("opencode refused ({status}): {why}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn get(&self, path: &str, directory: Option<&str>) -> Result<Value, String> {
        self.send(self.request(reqwest::Method::GET, path, directory)).await
    }

    pub async fn post(&self, path: &str, directory: &str, body: &Value) -> Result<Value, String> {
        self.send(self.request(reqwest::Method::POST, path, Some(directory)).json(body))
            .await
    }

    pub async fn patch(&self, path: &str, directory: &str, body: &Value) -> Result<Value, String> {
        self.send(self.request(reqwest::Method::PATCH, path, Some(directory)).json(body))
            .await
    }

    /// Every event from now on (`{ type, properties }`), for all projects.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    /// Reads the server-sent events until the stream ends.
    async fn read_events(&self) -> Result<(), String> {
        let mut res = self
            .request(reqwest::Method::GET, "/global/event", None)
            .send()
            .await
            .map_err(|e| format!("no event stream: {e}"))?;
        let mut buf: Vec<u8> = vec![];
        while let Some(chunk) = res.chunk().await.map_err(|e| format!("event stream broke: {e}"))? {
            buf.extend_from_slice(&chunk);
            while let Some(end) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim_end().strip_prefix("data: ") else { continue };
                let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
                // `/global/event` wraps each project's events.
                let event = if v["payload"].is_object() { v["payload"].clone() } else { v };
                let _ = self.events.send(event);
            }
        }
        Ok(())
    }
}

/// The models opencode can run in folder `dir` (its configured providers', the project's own
/// included), as `provider/model`.
pub async fn list_models(dir: &str, tasks: Option<&bach_tasks::Tasks>) -> Result<Vec<ModelInfo>, String> {
    let server = server(dir, tasks).await?;
    let providers = server.get("/config/providers", Some(dir)).await?;
    let config = server.get("/config", Some(dir)).await.unwrap_or_default();
    Ok(opencode::models_from(&providers, config["model"].as_str()))
}
