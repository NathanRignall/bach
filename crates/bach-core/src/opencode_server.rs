//! The one `opencode serve` Bach runs for all opencode sessions: started on first use, on
//! loopback with a random port and password, and restarted if it goes away. Every project
//! shares it (requests name their folder), and one event stream serves every run.
//!
//! It must not outlive Bach, however Bach ends (a restart is a SIGTERM; nothing gets to clean
//! up). So it runs under a small shell that holds Bach's end of a pipe and stops the server
//! when the pipe closes, which happens whenever Bach's process does.
use crate::adapters::opencode;
use bach_protocol::ModelInfo;
use serde_json::Value;
use std::{
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
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
}

/// Bach's end of the pipe keeping the server alive (see the module docs); taking it stops it.
type Lifeline = Arc<Mutex<Option<tokio::process::ChildStdin>>>;

/// The server, once started.
static SERVER: Mutex<Option<(Server, Lifeline)>> = Mutex::const_new(None);

/// Runs `opencode serve` until its stdin (Bach's pipe) closes.
const SUPERVISE: &str = "opencode serve --port 0 --hostname 127.0.0.1 & pid=$!; \
    cat > /dev/null; kill $pid 2>/dev/null; wait $pid";

/// What a run sees when the server has gone away; its turn is over.
pub const LOST: &str = "bach.server.lost";

/// The shared server, starting it if it isn't running.
pub async fn server() -> Result<Server, String> {
    let mut slot = SERVER.lock().await;
    if let Some((s, _)) = slot.as_ref() {
        if s.alive.load(Ordering::SeqCst) {
            return Ok(s.clone());
        }
    }
    let (server, lifeline) = start().await?;
    *slot = Some((server.clone(), lifeline));
    Ok(server)
}

async fn start() -> Result<(Server, Lifeline), String> {
    if which::which("opencode").is_err() {
        return Err("opencode isn't installed on the machine running the agents.".into());
    }
    let password = uuid::Uuid::new_v4().to_string();
    let mut child = Command::new("sh")
        .args(["-c", SUPERVISE])
        .env("OPENCODE_SERVER_PASSWORD", &password)
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

/// The models opencode can run (its configured providers'), as `provider/model`.
pub async fn list_models() -> Result<Vec<ModelInfo>, String> {
    let server = server().await?;
    let providers = server.get("/config/providers", None).await?;
    let config = server.get("/config", None).await.unwrap_or_default();
    Ok(opencode::models_from(&providers, config["model"].as_str()))
}
