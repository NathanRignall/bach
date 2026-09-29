//! Satie: Bach's launcher for long-running background processes, offered to agents as an MCP
//! server so it works for any agent that speaks MCP and outlives any single agent process.
//!
//! SPIKE: the tools only *record* what would be started; nothing is launched yet. What this
//! proves is the plumbing: the CLI connects, lists the tools, calls them through Bach's approval
//! flow, and the backend knows which run made each call (via a per-run bearer token).
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

/// Who a token belongs to.
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub run_id: String,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub id: u32,
    pub name: String,
    pub command: String,
    pub cwd: Option<String>,
    pub run_id: String,
}

#[derive(Default)]
struct Inner {
    runs: Mutex<HashMap<String, RunInfo>>,
    tasks: Mutex<Vec<Task>>,
}

/// Handle to the running MCP endpoint. Cheap to clone.
#[derive(Clone)]
pub struct Satie {
    url: String,
    inner: Arc<Inner>,
}

/// Revokes a run's token when dropped, so a finished run can't keep calling tools.
pub struct TokenGuard {
    satie: Satie,
    token: String,
}

impl Drop for TokenGuard {
    fn drop(&mut self) {
        self.satie.inner.runs.lock().unwrap().remove(&self.token);
    }
}

impl Satie {
    /// Starts the endpoint on `addr` (use port 0 for any free port). Loopback only.
    pub async fn start(addr: SocketAddr) -> std::io::Result<Satie> {
        assert!(addr.ip().is_loopback(), "Satie only listens on loopback");
        let listener = TcpListener::bind(addr).await?;
        let satie = Satie {
            url: format!("http://{}/mcp", listener.local_addr()?),
            inner: Arc::default(),
        };
        let app = Router::new()
            .route(
                "/mcp",
                post(mcp).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
            )
            .with_state(satie.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(satie)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Gives a run its own token. Returns the `--mcp-config` JSON to hand the agent and a guard
    /// that revokes the token when the run ends.
    pub fn register_run(&self, info: RunInfo) -> (String, TokenGuard) {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.inner.runs.lock().unwrap().insert(token.clone(), info);
        let config = json!({
            "mcpServers": { "satie": {
                "type": "http",
                "url": self.url,
                "headers": { "Authorization": format!("Bearer {token}") },
            }}
        });
        (
            config.to_string(),
            TokenGuard {
                satie: self.clone(),
                token,
            },
        )
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.inner.tasks.lock().unwrap().clone()
    }

    /// Runs that currently hold a token.
    pub fn active_runs(&self) -> usize {
        self.inner.runs.lock().unwrap().len()
    }

    fn tools() -> Value {
        json!([
            {
                "name": "task_start",
                "description": "Start a long-running background process (a dev server, simulation, watcher, ...) that keeps running after this conversation turn ends. Prefer this over running such a command in the background yourself, which is stopped when the turn ends.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to run" },
                        "name": { "type": "string", "description": "Short label shown to the user" },
                        "cwd": { "type": "string", "description": "Working directory; defaults to the project folder" }
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "task_list",
                "description": "List the background processes started with task_start.",
                "inputSchema": { "type": "object", "properties": {} }
            }
        ])
    }

    fn call(&self, run: &RunInfo, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "task_start" => {
                let command = args["command"]
                    .as_str()
                    .map(str::trim)
                    .filter(|c| !c.is_empty());
                let command = command.ok_or("`command` is required")?;
                let mut tasks = self.inner.tasks.lock().unwrap();
                let task = Task {
                    id: tasks.len() as u32 + 1,
                    name: args["name"].as_str().unwrap_or(command).to_string(),
                    command: command.to_string(),
                    cwd: args["cwd"]
                        .as_str()
                        .map(String::from)
                        .or_else(|| run.cwd.clone()),
                    run_id: run.run_id.clone(),
                };
                let text = format!(
                    "[Satie spike] Recorded task #{} `{}` (cwd: {}, run {}). Nothing was launched yet.",
                    task.id,
                    task.command,
                    task.cwd.as_deref().unwrap_or("-"),
                    task.run_id
                );
                tasks.push(task);
                Ok(text)
            }
            "task_list" => {
                let tasks = self.inner.tasks.lock().unwrap();
                Ok(if tasks.is_empty() {
                    "No tasks.".into()
                } else {
                    serde_json::to_string_pretty(&*tasks).unwrap()
                })
            }
            other => Err(format!("Unknown tool `{other}`")),
        }
    }

    /// One JSON-RPC message. `None` for notifications, which get no reply.
    fn handle(&self, run: &RunInfo, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        Some(match msg["method"].as_str().unwrap_or_default() {
            "initialize" => reply(json!({
                // Echo the client's protocol version: we use nothing version-specific.
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "satie", "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => reply(json!({})),
            "tools/list" => reply(json!({ "tools": Self::tools() })),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let (text, is_error) = match self.call(run, name, &msg["params"]["arguments"]) {
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
    let run = bearer(&headers).and_then(|t| satie.inner.runs.lock().unwrap().get(t).cloned());
    let Some(run) = run else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match body {
        Value::Array(msgs) => {
            let replies: Vec<Value> = msgs.iter().filter_map(|m| satie.handle(&run, m)).collect();
            if replies.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(replies).into_response()
            }
        }
        msg => match satie.handle(&run, &msg) {
            Some(reply) => Json(reply).into_response(),
            None => StatusCode::ACCEPTED.into_response(), // a notification
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal HTTP/1.1 client: (status, body).
    async fn post(url: &str, token: Option<&str>, body: &Value) -> (u16, String) {
        let addr = url.trim_start_matches("http://").trim_end_matches("/mcp");
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = body.to_string();
        let auth = token
            .map(|t| format!("Authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            out.split("\r\n\r\n").nth(1).unwrap_or_default().to_string(),
        )
    }

    fn rpc(id: u32, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    #[tokio::test]
    async fn speaks_mcp_and_ties_calls_to_their_run() {
        let satie = Satie::start("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let (config, guard) = satie.register_run(RunInfo {
            run_id: "run-1".into(),
            cwd: Some("/proj".into()),
        });
        let cfg: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(cfg["mcpServers"]["satie"]["type"], "http");
        assert_eq!(cfg["mcpServers"]["satie"]["url"], satie.url());
        let token = cfg["mcpServers"]["satie"]["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .trim_start_matches("Bearer ")
            .to_string();

        // No token, or a wrong one: refused.
        assert_eq!(
            post(satie.url(), None, &rpc(1, "ping", json!({}))).await.0,
            401
        );
        assert_eq!(
            post(satie.url(), Some("nope"), &rpc(1, "ping", json!({})))
                .await
                .0,
            401
        );

        let (st, body) = post(
            satie.url(),
            Some(&token),
            &rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        )
        .await;
        assert_eq!(st, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "satie");
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

        // A notification gets no reply body.
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(post(satie.url(), Some(&token), &note).await.0, 202);

        let (_, body) = post(satie.url(), Some(&token), &rpc(2, "tools/list", json!({}))).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let names: Vec<_> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["task_start", "task_list"]);

        let call = rpc(
            3,
            "tools/call",
            json!({ "name": "task_start", "arguments": { "command": "sleep 60", "name": "nap" } }),
        );
        let (_, body) = post(satie.url(), Some(&token), &call).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["isError"], false);
        let t = satie.tasks();
        assert_eq!(
            (
                t[0].command.as_str(),
                t[0].run_id.as_str(),
                t[0].cwd.as_deref()
            ),
            ("sleep 60", "run-1", Some("/proj"))
        );

        // A bad call is a tool error the agent can read, not a transport failure.
        let bad = rpc(
            4,
            "tools/call",
            json!({ "name": "task_start", "arguments": {} }),
        );
        let v: Value =
            serde_json::from_str(&post(satie.url(), Some(&token), &bad).await.1).unwrap();
        assert_eq!(v["result"]["isError"], true);
        let v: Value = serde_json::from_str(
            &post(satie.url(), Some(&token), &rpc(5, "nope", json!({})))
                .await
                .1,
        )
        .unwrap();
        assert_eq!(v["error"]["code"], -32601);

        // Once the run is over its token stops working.
        assert_eq!(satie.active_runs(), 1);
        drop(guard);
        assert_eq!(satie.active_runs(), 0);
        assert_eq!(
            post(satie.url(), Some(&token), &rpc(6, "ping", json!({})))
                .await
                .0,
            401
        );
    }
}
