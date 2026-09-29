//! Satie's MCP server: streamable HTTP on loopback, one bearer token per [`Grant`](crate::Grant).
//! Tools see and manage only the tasks of their grant's project.
use crate::{probe, Ready, Satie, Scope, StartTask, Task, TaskStatus, TaskView};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};

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
                        "timeout_seconds": { "type": "integer", "description": "How long to wait for `ports` / `ready_url` (default 30, at most 120)" }
                    },
                    "required": ["command"]
                }
            },
            { "name": "task_list", "description": "List this project's background tasks with their state, ports, running processes and any problems found (a port already taken, an expected port not listening).", "inputSchema": { "type": "object", "properties": {} } },
            {
                "name": "task_logs",
                "description": "Show the latest output of a background task.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string" }, "lines": { "type": "integer", "description": "How many lines (default 40)" } },
                    "required": ["id"]
                }
            },
            { "name": "task_stop", "description": "Stop a background task and everything it started.", "inputSchema": id },
            {
                "name": "port_info",
                "description": "Say what is listening on TCP ports on this machine: the process, its command line, folder and age, and whether it is a Satie task. Use it to find out who holds a port before deciding what to do about a conflict. Without `ports`, lists the recognisable listeners.",
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
        if !view.processes.is_empty() {
            s += &format!("\n  Processes: {}", view.processes.join(", "));
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
            let owner = match l.pgid.and_then(|g| tasks.get(&g)) {
                Some((id, name)) => format!("Satie task {id} \"{name}\""),
                None => "not a Satie task".to_string(),
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
                let out = self.logs(&task.id, 15).unwrap_or_default();
                if !out.is_empty() {
                    text += &format!("\nOutput so far:\n{out}");
                }
                Ok(text)
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
                self.logs(id, lines).map_err(|e| e.to_string())
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

    /// One JSON-RPC message. `None` for notifications, which get no reply.
    pub(crate) async fn handle(&self, scope: &Scope, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        Some(match msg["method"].as_str().unwrap_or_default() {
            "initialize" => reply(json!({
                // Echo the client's protocol version: nothing here is version-specific.
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "satie", "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => reply(json!({})),
            "tools/list" => reply(json!({ "tools": Self::tools() })),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let (text, is_error) = match self.call(scope, name, &msg["params"]["arguments"]).await
                {
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
