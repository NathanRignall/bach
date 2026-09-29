//! Codex over `codex app-server`: newline-delimited JSON-RPC on stdin/stdout (without the
//! `jsonrpc` field). One process per turn, as with Claude Code: initialize, start or resume
//! the thread, start the turn, answer its approval requests, and close stdin once the turn
//! completes. Shapes recorded from codex-cli 0.146.0 (tests/fixtures/codex_*.jsonl).
use super::AgentEvent;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// Codex edits the project on its own and asks before anything beyond it (network, other
/// folders, `.git`). Without this, `codex` defaults to a read-only sandbox.
const SANDBOX: &str = "workspace-write";
const APPROVAL_POLICY: &str = "on-request";

/// The session rule "allow for this session" grants for file changes.
const EDIT_RULE: &str = "Edit";

const INITIALIZE: u64 = 1;
const THREAD: u64 = 2;
const TURN: u64 = 3;
const INTERRUPT: u64 = 4;

pub fn args() -> Vec<String> {
    vec!["app-server".into()]
}

/// One turn's conversation with the app server.
pub struct Conversation {
    prompt: String,
    images: Vec<String>,
    thread_id: Option<String>,
    cwd: Option<String>,
    /// Session rules approved earlier: requests they cover are accepted without asking.
    allowed: Vec<String>,
    model: Option<String>,
    /// Whether the model's context window has been reported (it doesn't change mid-turn).
    window_sent: bool,
    turn_id: Option<String>,
    /// File changes by item id: the approval request itself doesn't say what would change.
    changes: HashMap<String, Value>,
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

fn reply(id: &Value, result: Value) -> String {
    json!({ "id": id, "result": result }).to_string()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// `zsh -lc 'git push'` -> `git push`: the shell wrapper Codex runs every command through.
pub fn unwrap_shell(command: &str) -> String {
    match shlex::split(command).as_deref() {
        Some([_, flag, inner]) if flag == "-c" || flag == "-lc" => inner.clone(),
        _ => command.to_string(),
    }
}

fn shell_rule(command: &str) -> String {
    format!("Shell({command})")
}

impl Conversation {
    pub fn new(
        prompt: &str,
        images: &[String],
        thread_id: Option<&str>,
        cwd: Option<&str>,
        allowed: &[String],
    ) -> Self {
        Self {
            prompt: prompt.into(),
            images: images.to_vec(),
            thread_id: thread_id.map(String::from),
            cwd: cwd.map(String::from),
            allowed: allowed.to_vec(),
            model: None,
            window_sent: false,
            turn_id: None,
            changes: HashMap::new(),
        }
    }

    /// The first lines to send.
    pub fn opening(&self) -> Vec<String> {
        vec![request(
            INITIALIZE,
            "initialize",
            json!({ "clientInfo": { "name": "bach", "title": "Bach", "version": env!("CARGO_PKG_VERSION") } }),
        )]
    }

    /// Asks Codex to stop the turn; it then completes as interrupted.
    pub fn interrupt(&self) -> Option<String> {
        let (thread, turn) = (self.thread_id.as_ref()?, self.turn_id.as_ref()?);
        Some(request(
            INTERRUPT,
            "turn/interrupt",
            json!({ "threadId": thread, "turnId": turn }),
        ))
    }

    /// Handles one line from the server: the events to show and the lines to send back.
    pub fn on_line(&mut self, v: &Value) -> (Vec<AgentEvent>, Vec<String>) {
        match (v.get("id"), v["method"].as_str()) {
            (Some(id), Some(method)) => self.on_request(id, method, &v["params"]),
            (Some(id), None) => self.on_response(id.as_u64(), v),
            (None, Some(method)) => (self.on_notification(method, &v["params"]), vec![]),
            _ => (vec![], vec![]),
        }
    }

    fn on_response(&mut self, id: Option<u64>, v: &Value) -> (Vec<AgentEvent>, Vec<String>) {
        if let Some(e) = v.get("error") {
            if id == Some(INTERRUPT) {
                return (vec![], vec![]);
            }
            let message = e["message"].as_str().unwrap_or("Codex refused the request.");
            return (
                vec![
                    AgentEvent::Error {
                        message: message.into(),
                    },
                    AgentEvent::Done {
                        cost_usd: None,
                        is_error: true,
                    },
                ],
                vec![],
            );
        }
        let result = &v["result"];
        match id {
            Some(INITIALIZE) => {
                // Also given when resuming: a thread first run by `codex exec` would otherwise
                // keep its read-only sandbox.
                let mut params = json!({ "approvalPolicy": APPROVAL_POLICY, "sandbox": SANDBOX });
                if let Some(cwd) = &self.cwd {
                    params["cwd"] = json!(cwd);
                }
                let method = match &self.thread_id {
                    Some(id) => {
                        params["threadId"] = json!(id);
                        "thread/resume"
                    }
                    None => "thread/start",
                };
                (
                    vec![],
                    vec![
                        json!({ "method": "initialized" }).to_string(),
                        request(THREAD, method, params),
                    ],
                )
            }
            Some(THREAD) => {
                let id = s(&result["thread"]["id"]);
                self.model = result["model"].as_str().map(String::from);
                self.thread_id = Some(id.clone());
                let mut input: Vec<Value> = vec![];
                if !self.prompt.is_empty() {
                    input.push(json!({ "type": "text", "text": self.prompt, "text_elements": [] }));
                }
                input.extend(
                    self.images
                        .iter()
                        .map(|p| json!({ "type": "localImage", "path": p })),
                );
                (
                    vec![AgentEvent::Session {
                        id: id.clone(),
                        model: self.model.clone(),
                    }],
                    vec![request(
                        TURN,
                        "turn/start",
                        json!({ "threadId": id, "input": input }),
                    )],
                )
            }
            Some(TURN) => {
                self.turn_id = result["turn"]["id"].as_str().map(String::from);
                (vec![], vec![])
            }
            _ => (vec![], vec![]),
        }
    }

    /// Something Codex needs answered before it carries on.
    fn on_request(&mut self, id: &Value, method: &str, p: &Value) -> (Vec<AgentEvent>, Vec<String>) {
        let item_id = s(&p["itemId"]);
        // Unique within the session (the server's own ids restart with every process).
        let request_id = format!(
            "{}/{}",
            p["approvalId"].as_str().unwrap_or(&item_id),
            id
        );
        let reason = p["reason"].as_str().map(String::from);
        let (tool_name, input, rules, directories, kind, amendment) = match method {
            "item/commandExecution/requestApproval" => {
                let command = unwrap_shell(p["command"].as_str().unwrap_or_default());
                let cwd = s(&p["cwd"]);
                let mut input = json!({ "command": command });
                if !cwd.is_empty() {
                    input["cwd"] = json!(cwd);
                }
                (
                    "Shell",
                    input,
                    vec![shell_rule(&command)],
                    // Kept only when outside the project (see `Runs`).
                    [cwd].into_iter().filter(|d| !d.is_empty()).collect(),
                    "command",
                    p["proposedExecpolicyAmendment"].clone(),
                )
            }
            "item/fileChange/requestApproval" => {
                let changes = self.changes.get(&item_id).cloned().unwrap_or(json!([]));
                (
                    "Edit",
                    file_change_input(&changes),
                    vec![EDIT_RULE.to_string()],
                    vec![],
                    "fileChange",
                    Value::Null,
                )
            }
            // Nothing Bach can answer yet (questions, MCP elicitations): say so rather than
            // leave the turn waiting forever.
            _ => {
                let error = json!({ "id": id, "error": { "code": -32601, "message": format!("Bach can't answer `{method}` yet.") } });
                return (vec![], vec![error.to_string()]);
            }
        };
        if rules.iter().all(|r| self.allowed.contains(r)) {
            return (vec![], vec![reply(id, json!({ "decision": "accept" }))]);
        }
        (
            vec![AgentEvent::Approval {
                request_id,
                tool_use_id: Some(item_id),
                tool_name: tool_name.into(),
                input,
                description: None,
                reason,
                rules,
                directories,
                suggestions: json!({
                    "rpcId": id,
                    "kind": kind,
                    "amendment": amendment,
                    "available": p["availableDecisions"],
                }),
            }],
            vec![],
        )
    }

    fn on_notification(&mut self, method: &str, p: &Value) -> Vec<AgentEvent> {
        let item = &p["item"];
        let id = s(&item["id"]);
        match (method, item["type"].as_str()) {
            ("item/started", Some("commandExecution")) => vec![AgentEvent::ToolUse {
                id,
                name: "Shell".into(),
                input: json!({ "command": unwrap_shell(item["command"].as_str().unwrap_or_default()) }),
                parent: None,
            }],
            ("item/completed", Some("commandExecution")) => vec![AgentEvent::ToolResult {
                id,
                output: s(&item["aggregatedOutput"]),
                is_error: item["status"] != "completed"
                    || item["exitCode"].as_i64().is_some_and(|c| c != 0),
                images: vec![],
                parent: None,
            }],
            ("item/started", Some("fileChange")) => {
                self.changes.insert(id.clone(), item["changes"].clone());
                vec![AgentEvent::ToolUse {
                    id,
                    name: "Edit".into(),
                    input: file_change_input(&item["changes"]),
                    parent: None,
                }]
            }
            ("item/completed", Some("fileChange")) => {
                self.changes.remove(&id);
                let status = item["status"].as_str().unwrap_or_default();
                let paths = paths(&item["changes"]);
                vec![AgentEvent::ToolResult {
                    id,
                    output: match status {
                        "completed" => format!("Changed {paths}"),
                        "declined" => format!("Not allowed to change {paths}"),
                        _ => format!("Couldn't change {paths}"),
                    },
                    is_error: status != "completed",
                    images: vec![],
                    parent: None,
                }]
            }
            ("item/started", Some("mcpToolCall")) => vec![AgentEvent::ToolUse {
                id,
                name: format!("mcp__{}__{}", s(&item["server"]), s(&item["tool"])),
                input: item["arguments"].clone(),
                parent: None,
            }],
            ("item/completed", Some("mcpToolCall")) => {
                let failed = !item["error"].is_null() || item["status"] == "failed";
                let output = if failed {
                    item["error"]["message"].as_str().unwrap_or("failed").to_string()
                } else {
                    item["result"]["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                vec![AgentEvent::ToolResult {
                    id,
                    output,
                    is_error: failed,
                    images: vec![],
                    parent: None,
                }]
            }
            ("item/completed", Some("agentMessage")) => {
                let text = s(&item["text"]);
                if text.is_empty() {
                    return vec![];
                }
                vec![AgentEvent::Text { text, parent: None }]
            }
            ("item/completed", Some("reasoning")) => {
                // The summary when there is one; the raw reasoning is rarely given.
                let parts = |k: &str| -> Vec<String> {
                    item[k]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                };
                let mut text = parts("summary");
                if text.is_empty() {
                    text = parts("content");
                }
                let text = text.join("\n\n");
                if text.trim().is_empty() {
                    return vec![];
                }
                vec![AgentEvent::Thinking { text }]
            }
            ("thread/tokenUsage/updated", _) => {
                let usage = &p["tokenUsage"];
                let mut events = vec![];
                if let Some(used) = usage["last"]["totalTokens"].as_u64() {
                    events.push(AgentEvent::Context { used });
                }
                if let (Some(model), Some(window), false) =
                    (&self.model, usage["modelContextWindow"].as_u64(), self.window_sent)
                {
                    self.window_sent = true;
                    events.push(AgentEvent::ContextWindows {
                        windows: BTreeMap::from([(model.clone(), window)]),
                    });
                }
                events
            }
            // Retries are Codex's business; only a final error is worth showing.
            ("error", _) if p["willRetry"] != true => vec![AgentEvent::Error {
                message: p["error"]["message"].as_str().unwrap_or("error").into(),
            }],
            ("turn/completed", _) => {
                let turn = &p["turn"];
                let mut events = vec![];
                if turn["status"] == "failed" {
                    if let Some(m) = turn["error"]["message"].as_str() {
                        events.push(AgentEvent::Error { message: m.into() });
                    }
                }
                events.push(AgentEvent::Done {
                    cost_usd: None,
                    is_error: turn["status"] != "completed",
                });
                events
            }
            ("serverRequest/resolved", _) => {
                // Answered by Bach, or no longer needed (the turn was interrupted).
                vec![]
            }
            _ => vec![],
        }
    }
}

/// `a.rs, b.rs`
fn paths(changes: &Value) -> String {
    changes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["path"].as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a file change shows in the UI: the files and the combined diff.
fn file_change_input(changes: &Value) -> Value {
    let diff = changes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["diff"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    json!({ "file_path": paths(changes), "diff": diff })
}

/// The reply to an approval request, from what was kept of it (`suggestions`).
///
/// Bach remembers "for this session" rules itself and answers later requests they cover, so
/// a plain `accept` does when Codex doesn't offer `acceptForSession` (it lists what it takes in
/// `availableDecisions`, when it says).
pub fn answer(suggestions: &Value, decision: bach_protocol::Decision) -> String {
    use bach_protocol::Decision::*;
    let offered = |name: &str| match suggestions["available"].as_array() {
        Some(a) => a
            .iter()
            .any(|d| d == name || d.get(name).is_some()),
        None => true,
    };
    let for_session = || {
        if offered("acceptForSession") {
            json!("acceptForSession")
        } else {
            json!("accept")
        }
    };
    let decision = match decision {
        Allow => json!("accept"),
        AllowSession => for_session(),
        // Codex saves the command prefix it suggested to its own rules (~/.codex/rules).
        // File changes have no lasting rule; they're allowed for the session.
        AllowAlways => match &suggestions["amendment"] {
            Value::Array(a) if !a.is_empty() && offered("acceptWithExecpolicyAmendment") => {
                json!({ "acceptWithExecpolicyAmendment": { "execpolicy_amendment": a } })
            }
            _ => for_session(),
        },
        Deny => json!("decline"),
    };
    reply(&suggestions["rpcId"], json!({ "decision": decision }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_the_shell_wrapper() {
        assert_eq!(
            unwrap_shell("/run/current-system/sw/bin/zsh -lc 'git commit -m \"hi there\"'"),
            "git commit -m \"hi there\""
        );
        assert_eq!(unwrap_shell("bash -c ls"), "ls");
        assert_eq!(unwrap_shell("git status"), "git status");
    }
}
