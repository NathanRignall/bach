//! Satie's MCP server: streamable HTTP on loopback, one bearer token per [`Grant`](crate::Grant).
//! Tools see and manage only the tasks of their grant's project. Embedders can add tools of their
//! own ([`Tools`]).
use crate::{
    compose, probe, ProcessAction, Ready, Satie, Scope, StartCompose, StartTask, Task, TaskStatus,
    TaskView,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::{collections::HashMap, future::Future, pin::Pin, time::Duration};

/// More tools for Satie's MCP server, from whoever embeds it (see [`Satie::add_tools`]).
pub trait Tools: Send + Sync + 'static {
    /// Their MCP descriptions (`name`, `description`, `inputSchema`).
    fn list(&self) -> Vec<Value>;
    /// What agents are told about them, after Satie's own instructions.
    fn instructions(&self) -> Option<String> {
        None
    }
    /// Runs tool `name` for a caller with `scope`: the text to show it, or an error to show it.
    /// `None` if `name` isn't one of these tools.
    fn call<'a>(
        &'a self,
        scope: &'a Scope,
        name: &'a str,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<Result<String, String>>> + Send + 'a>>;
}

/// Given to the agent with the tools (MCP `instructions`), so it reaches the agent without taking
/// its system prompt from anyone else: Claude Code keeps only the last `--append-system-prompt`,
/// and a wrapper may put its own there.
const INSTRUCTIONS: &str = "Anything that must keep running after your turn ends (dev servers, simulations, watchers, \
long jobs) has to be started with the `task_start` tool, not from a shell command in the background (Bash \
`run_in_background`, `nohup`, a trailing `&` or tmux): processes started those ways are stopped when the turn ends. \
`task_start` keeps the process running on its own, shows it to the user in the Tasks panel, and `task_logs`, `task_list` \
and `task_stop` manage it. Pass `port` when the process serves on one, so the call waits until it is up, and \
`interactive: false` when only you will use it (a server for tests or headless browser checks), so its ports aren't \
forwarded to the user's computer. For a process-compose project use `compose_start` with the compose file instead of \
running process-compose yourself: each of its processes then gets its own state and log, and `task_process` restarts one \
without the rest.";

pub(crate) fn router(satie: Satie) -> Router {
    Router::new()
        .route(
            "/mcp",
            post(mcp).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .with_state(satie)
}

impl Satie {
    pub(crate) fn tools() -> Value {
        let interactive = json!({ "type": "boolean", "description": "Default true: the user will open it (a dev server to browse to), so the app forwards its ports to their computer. Set false for a task only you use, such as a server for tests or headless browser checks: it is still listed, but its ports are not forwarded automatically." });
        let id = json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] });
        json!([
            {
                "name": "task_start",
                "description": "Start a long-running background process (dev server, simulation, watcher, long job) that keeps running after this turn ends. Use this instead of running such commands in the background yourself, which are stopped when the turn ends. The user can see and stop it. Give every port it should serve on in `ports`: the call waits for them and reports any that do not come up, and why if another process holds the port. Returns the task id, its state and the first output.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to run" },
                        "name": { "type": "string", "description": "Short label shown to the user" },
                        "cwd": { "type": "string", "description": "Working directory; defaults to the project folder" },
                        "ports": { "type": "array", "items": { "type": "integer" }, "description": "TCP ports the task should be listening on; wait (up to 30s) for all of them" },
                        "port": { "type": "integer", "description": "Shorthand for a single entry in `ports`" },
                        "ready_url": { "type": "string", "description": "A local http://localhost:PORT/... URL that must answer before the task counts as ready; the response status is reported" },
                        "ready_status": { "type": "integer", "description": "Exact status `ready_url` must return (default: anything below 500)" },
                        "timeout_seconds": { "type": "integer", "description": "How long to wait for `ports` / `ready_url` (default 30, at most 120)" },
                        "interactive": interactive.clone()
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "compose_start",
                "description": "Run a process-compose project (a compose file such as process-compose.yaml) as a background task that keeps running after this turn ends. Prefer this to task_start for process-compose: it starts it in the project's environment (direnv, else the flake's devShell) with the right flags, so don't add any, and then shows, logs and controls each process separately. Waits until every process is running (and ready, where it has a readiness probe) or one fails, and reports each process's state, ports and, for failures, its last output.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "The compose file, relative to `cwd`" },
                        "name": { "type": "string", "description": "Short label shown to the user; defaults to the file" },
                        "cwd": { "type": "string", "description": "Working directory; defaults to the project folder" },
                        "ports": { "type": "array", "items": { "type": "integer" }, "description": "TCP ports the project should be listening on; wait for them too" },
                        "timeout_seconds": { "type": "integer", "description": "How long to wait for the processes (default 60, at most 300)" },
                        "shell": { "type": "string", "description": "Command prefix that enters the project's environment, e.g. `nix develop .#sim --command`; empty for none. Default: `direnv exec .` with an .envrc, `nix develop --command` with only a flake.nix" },
                        "interactive": interactive
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "task_process",
                "description": "Start, stop or restart one process of a process-compose task (started with compose_start), e.g. restart a server after changing its code. The rest of the project keeps running.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "process": { "type": "string", "description": "The process name from the compose file" },
                        "action": { "type": "string", "enum": ["start", "stop", "restart"] }
                    },
                    "required": ["id", "process", "action"]
                }
            },
            { "name": "task_list", "description": "List this project's background tasks with their state, ports, running processes and any problems found (a port already taken, an expected port not listening).", "inputSchema": { "type": "object", "properties": {} } },
            {
                "name": "task_logs",
                "description": "Show the latest output of a background task, or of one process of a process-compose task.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "lines": { "type": "integer", "description": "How many lines (default 40)" },
                        "process": { "type": "string", "description": "For a process-compose task: just this process's output" }
                    },
                    "required": ["id"]
                }
            },
            { "name": "task_stop", "description": "Stop a background task and everything it started.", "inputSchema": id },
            {
                "name": "port_info",
                "description": "Say what is listening on TCP ports on this machine: the process, its command line, folder and age, and whether it is one of these background tasks. Use it to find out who holds a port before deciding what to do about a conflict. Without `ports`, lists the recognisable listeners.",
                "inputSchema": { "type": "object", "properties": { "ports": { "type": "array", "items": { "type": "integer" } } } }
            },
            {
                "name": "http_check",
                "description": "Request a local http://localhost:PORT/... URL and report the HTTP status. Only localhost can be checked.",
                "inputSchema": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
            }
        ])
    }

    pub(crate) fn describe(view: &TaskView) -> String {
        let t = &view.task;
        // First line: state and ports. Details, if any, follow on their own lines.
        let mut s = format!(
            "Task {} \"{}\": {}",
            t.id,
            t.name,
            format!("{:?}", t.status).to_lowercase()
        );
        if let Some(code) = t.exit_code {
            s += &format!(" (exit code {code})");
        }
        if !t.interactive {
            s += ", not interactive";
        }
        if t.status == TaskStatus::Running {
            s += &format!(", pid {}", t.pid);
        }
        if !view.ports.is_empty() {
            let ports: Vec<String> = view.ports.iter().map(|p| p.to_string()).collect();
            s += &format!(", listening on {}", ports.join(", "));
        }
        if t.status == TaskStatus::Running && !t.expected_ports.is_empty() {
            let parts: Vec<String> = t
                .expected_ports
                .iter()
                .map(|p| {
                    let state = if view.up_ports.contains(p) {
                        "up"
                    } else if view.missing_ports.contains(p) {
                        "NOT listening"
                    } else {
                        "not up yet"
                    };
                    format!("{p} {state}")
                })
                .collect();
            s += &format!("\n  Expected ports: {}", parts.join(", "));
        }
        match &view.compose {
            Some(procs) if !procs.is_empty() => {
                s += &format!("\n  Compose file: {}", t.compose_file.as_deref().unwrap_or_default());
                for p in procs {
                    s += &format!("\n  - {}: {}", p.name, p.status);
                    if !p.running && p.status != "Pending" {
                        s += &format!(" (exit code {})", p.exit_code);
                    }
                    match p.ready {
                        Some(true) => s += ", ready",
                        Some(false) if p.running => s += ", NOT ready",
                        _ => {}
                    }
                    if !p.ports.is_empty() {
                        let ports: Vec<String> = p.ports.iter().map(|p| p.to_string()).collect();
                        s += &format!(", listening on {}", ports.join(", "));
                    }
                    match p.restarts {
                        0 => {}
                        1 => s += ", restarted once",
                        n => s += &format!(", restarted {n} times"),
                    }
                }
            }
            Some(_) if t.status == TaskStatus::Running => s += "\n  Processes: (process-compose not answering yet)",
            _ => {
                if !view.processes.is_empty() {
                    s += &format!("\n  Processes: {}", view.processes.join(", "));
                }
            }
        }
        for p in &view.problems {
            s += &format!("\n  Problem: {p}");
        }
        s
    }

    /// Who is listening on `wanted` ports (or, with none given, on the recognisable ones).
    pub(crate) fn port_report(&self, wanted: &[u16]) -> String {
        let all = probe::listeners();
        let tasks: HashMap<u32, (String, String)> = self
            .inner
            .tasks
            .lock()
            .unwrap()
            .values()
            .map(|t| (t.pid, (t.id.clone(), t.name.clone())))
            .collect();
        let line = |l: &probe::Listener| {
            let owner = match l.sid.and_then(|s| tasks.get(&s)) {
                Some((id, name)) => format!("background task {id} \"{name}\""),
                None => "not a background task".to_string(),
            };
            match l.pid {
                Some(pid) => format!(
                    "Port {}: pid {pid} `{}`{}{} - {owner}",
                    l.port,
                    l.command,
                    l.cwd
                        .as_ref()
                        .map(|c| format!(", in {c}"))
                        .unwrap_or_default(),
                    l.up_secs
                        .map(|u| format!(", up {}", probe::age(u)))
                        .unwrap_or_default(),
                ),
                None => format!("Port {}: {}", l.port, l.command),
            }
        };
        let mut lines = vec![];
        if wanted.is_empty() {
            let mut shown = all.clone();
            shown.retain(|l| l.port < 32768);
            shown.dedup_by_key(|l| (l.port, l.pid));
            lines.extend(shown.iter().take(60).map(line));
            if lines.is_empty() {
                lines.push("Nothing is listening on a recognisable port.".into());
            }
        } else {
            for p in wanted {
                let mut holders: Vec<&probe::Listener> =
                    all.iter().filter(|l| l.port == *p).collect();
                holders.dedup_by_key(|l| l.pid);
                if holders.is_empty() {
                    lines.push(format!("Port {p}: nothing is listening."));
                }
                lines.extend(holders.into_iter().map(line));
            }
        }
        lines.join("\n")
    }

    /// A task, checked to belong to the calling run's project.
    pub(crate) fn owned(&self, scope: &Scope, id: &str) -> Result<Task, String> {
        let t = self
            .get(id)
            .ok_or_else(|| format!("No task with id `{id}`."))?;
        match (&scope.project, &t.project) {
            (Some(mine), Some(theirs)) if mine != theirs => Err(format!("No task with id `{id}`.")),
            _ => Ok(t),
        }
    }

    pub(crate) fn view(&self, id: &str) -> Option<TaskView> {
        self.list(None).into_iter().find(|v| v.task.id == id)
    }

    pub(crate) async fn call(&self, scope: &Scope, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "task_start" => {
                let mut ports: Vec<u16> = args["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(args.get("port"))
                    .filter_map(|p| {
                        p.as_u64()
                            .and_then(|p| u16::try_from(p).ok())
                            .filter(|p| *p > 0)
                    })
                    .collect();
                ports.sort_unstable();
                ports.dedup();
                if let Some(url) = args["ready_url"].as_str() {
                    // Fail before launching anything: a URL we would never request can't become ready.
                    probe::validate_local_url(url)
                        .map_err(|e| format!("`ready_url`: {e}"))?;
                }
                let wait = Duration::from_secs(
                    args["timeout_seconds"].as_u64().unwrap_or(30).clamp(1, 120),
                );
                let ready = args["ready_url"].as_str().map(|url| Ready {
                    url: url.to_string(),
                    status: args["ready_status"]
                        .as_u64()
                        .and_then(|p| u16::try_from(p).ok()),
                });
                let task = self.start_task(StartTask {
                    command: args["command"].as_str().unwrap_or_default().to_string(),
                    cwd: args["cwd"].as_str().map(String::from),
                    name: args["name"].as_str().map(String::from),
                    project: scope.project.clone(),
                    owner: scope.owner.clone(),
                    ports: ports.clone(),
                    interactive: args["interactive"].as_bool(),
                })
                .map_err(|e| e.to_string())?;
                let probe = self
                    .wait_ready(&task.id, &ports, ready.as_ref(), wait)
                    .await;
                // The wait is over: anything still not listening is a problem now, not "still starting".
                let view = self
                    .list_settled(None, 0)
                    .into_iter()
                    .find(|v| v.task.id == task.id)
                    .ok_or("The task disappeared.")?;
                let mut text = Self::describe(&view);
                if let Some(r) = &ready {
                    match probe {
                        Some(Ok(p)) => {
                            text += &format!(
                                "\n  HTTP check: GET {} -> {} ({} ms)",
                                r.url, p.status, p.ms
                            )
                        }
                        Some(Err(e)) => {
                            text += &format!("\n  HTTP check: GET {} did not answer: {e}", r.url)
                        }
                        None => {
                            text += &format!(
                            "\n  HTTP check: GET {} was not attempted (the ports did not come up)",
                            r.url
                        )
                        }
                    }
                }
                let out = self.logs(&task.id, 15, None).unwrap_or_default();
                if !out.is_empty() {
                    text += &format!("\nOutput so far:\n{out}");
                }
                Ok(text)
            }
            "compose_start" => {
                let ports: Vec<u16> = args["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0))
                    .collect();
                let wait = Duration::from_secs(args["timeout_seconds"].as_u64().unwrap_or(60).clamp(1, 300));
                let task = self
                    .start_compose(StartCompose {
                        file: args["file"].as_str().unwrap_or_default().to_string(),
                        shell: args["shell"].as_str().map(String::from),
                        task: StartTask {
                            cwd: args["cwd"].as_str().map(String::from),
                            name: args["name"].as_str().map(String::from),
                            project: scope.project.clone(),
                            owner: scope.owner.clone(),
                            ports: ports.clone(),
                            interactive: args["interactive"].as_bool(),
                            ..Default::default()
                        },
                    })
                    .map_err(|e| e.to_string())?;
                let started = std::time::Instant::now();
                self.wait_compose(&task.id, wait).await;
                if !ports.is_empty() {
                    let left = wait.saturating_sub(started.elapsed()).max(Duration::from_secs(1));
                    self.wait_ready(&task.id, &ports, None, left).await;
                }
                let view = self
                    .list_settled(None, 0)
                    .into_iter()
                    .find(|v| v.task.id == task.id)
                    .ok_or("The task disappeared.")?;
                let mut text = Self::describe(&view);
                let failed: Vec<&str> = view
                    .compose
                    .iter()
                    .flatten()
                    .filter(|p| compose::failed(p))
                    .map(|p| p.name.as_str())
                    .collect();
                for name in &failed {
                    let out = self.logs(&task.id, 15, Some(name)).unwrap_or_default();
                    if !out.is_empty() {
                        text += &format!("\nLast output of {name}:\n{out}");
                    }
                }
                if view.compose.as_ref().is_none_or(|p| p.is_empty()) || view.task.status != TaskStatus::Running {
                    let out = self.logs(&task.id, 15, None).unwrap_or_default();
                    if !out.is_empty() {
                        text += &format!("\nOutput so far:\n{out}");
                    }
                }
                Ok(text)
            }
            "task_process" => {
                let id = args["id"].as_str().unwrap_or_default();
                self.owned(scope, id)?;
                let process = args["process"].as_str().unwrap_or_default();
                let action = match args["action"].as_str().unwrap_or_default() {
                    "start" => ProcessAction::Start,
                    "stop" => ProcessAction::Stop,
                    "restart" => ProcessAction::Restart,
                    other => return Err(format!("Unknown action `{other}`: use start, stop or restart.")),
                };
                self.process_action(id, process, action)
                    .await
                    .map_err(|e| e.to_string())?;
                if action != ProcessAction::Stop {
                    // Give it the moment it needs to come up (or fail) before reporting; process-compose
                    // takes a moment to even start on it.
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    self.wait_compose(id, Duration::from_secs(15)).await;
                }
                Ok(self
                    .view(id)
                    .map(|v| Self::describe(&v))
                    .unwrap_or_else(|| "Done.".into()))
            }
            "port_info" => {
                let wanted: Vec<u16> = args["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_u64().and_then(|p| u16::try_from(p).ok()))
                    .collect();
                Ok(self.port_report(&wanted))
            }
            "http_check" => {
                let url = args["url"].as_str().unwrap_or_default();
                Ok(
                    match probe::http_get(url, Duration::from_secs(4)).await {
                        Ok(p) => format!("GET {url} -> {} ({} ms)", p.status, p.ms),
                        Err(e) => format!("GET {url} did not answer: {e}"),
                    },
                )
            }
            "task_list" => {
                let views = self.list(scope.project.as_deref());
                Ok(if views.is_empty() {
                    "No background tasks.".into()
                } else {
                    views
                        .iter()
                        .map(Self::describe)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            "task_logs" => {
                let id = args["id"].as_str().unwrap_or_default();
                self.owned(scope, id)?;
                let lines = args["lines"].as_u64().unwrap_or(40).clamp(1, 500) as usize;
                self.logs(id, lines, args["process"].as_str())
                    .map_err(|e| e.to_string())
            }
            "task_stop" => {
                let id = args["id"].as_str().unwrap_or_default();
                self.owned(scope, id)?;
                self.stop_task(id).await.map_err(|e| e.to_string())?;
                Ok(self
                    .view(id)
                    .map(|v| Self::describe(&v))
                    .unwrap_or_else(|| "Stopped.".into()))
            }
            other => Err(format!("Unknown tool `{other}`")),
        }
    }

    /// The server's MCP `instructions`: Satie's, then the embedder's.
    fn instructions(&self) -> String {
        let extra = self.inner.extra_tools.read().unwrap().clone();
        std::iter::once(INSTRUCTIONS.to_string())
            .chain(extra.iter().filter_map(|t| t.instructions()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Runs one of Satie's tools or, failing that, one of the embedder's.
    async fn call_any(&self, scope: &Scope, name: &str, args: &Value) -> Result<String, String> {
        let own = Self::tools();
        if own.as_array().expect("a list").iter().any(|t| t["name"] == name) {
            return self.call(scope, name, args).await;
        }
        let extra = self.inner.extra_tools.read().unwrap().clone();
        for tools in &extra {
            if let Some(answer) = tools.call(scope, name, args).await {
                return answer;
            }
        }
        Err(format!("Unknown tool `{name}`"))
    }

    /// One JSON-RPC message. `None` for notifications, which get no reply.
    pub(crate) async fn handle(&self, scope: &Scope, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        Some(match msg["method"].as_str().unwrap_or_default() {
            "initialize" => reply(json!({
                // Echo the client's protocol version: nothing here is version-specific.
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": *self.inner.server_name.read().unwrap(), "version": env!("CARGO_PKG_VERSION") },
                "instructions": self.instructions(),
            })),
            "ping" => reply(json!({})),
            "tools/list" => {
                let mut tools = Self::tools();
                let extra = self.inner.extra_tools.read().unwrap().clone();
                tools
                    .as_array_mut()
                    .expect("a list")
                    .extend(extra.iter().flat_map(|t| t.list()));
                reply(json!({ "tools": tools }))
            }
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let args = &msg["params"]["arguments"];
                let (text, is_error) = match self.call_any(scope, name, args).await {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                reply(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
            }
            other => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {other}") },
            }),
        })
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn mcp(State(satie): State<Satie>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let scope = bearer(&headers).and_then(|t| satie.inner.grants.lock().unwrap().get(t).cloned());
    let Some(scope) = scope else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match body {
        Value::Array(msgs) => {
            let mut replies = vec![];
            for m in &msgs {
                replies.extend(satie.handle(&scope, m).await);
            }
            if replies.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(replies).into_response()
            }
        }
        msg => match satie.handle(&scope, &msg).await {
            Some(reply) => Json(reply).into_response(),
            None => StatusCode::ACCEPTED.into_response(), // a notification
        },
    }
}
