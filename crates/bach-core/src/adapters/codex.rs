//! Codex over `codex app-server`: newline-delimited JSON-RPC on stdin/stdout (without the
//! `jsonrpc` field). One process per turn, as with Claude Code: initialize, start or resume
//! the thread, start the turn, answer its approval requests, and close stdin once the turn
//! completes. Shapes recorded from codex-cli 0.146.0 (tests/fixtures/codex_*.jsonl).
use super::{AgentEvent, Turn};
use crate::attachments::{Kind, SavedFile};
use bach_protocol::{DeltaKind, ModelInfo};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// The permission modes Bach offers for Codex: its own presets, and its plan mode. Anything
/// else (none, or a mode left over from Claude Code) is "auto".
pub const PERMISSION_MODES: &[&str] = &["auto", "readOnly", "fullAccess", "plan"];

/// The sandbox and approval policy for a permission mode.
///
/// "auto": Codex edits the project on its own and asks before anything beyond it (network,
/// other folders, `.git`); without a sandbox given, `codex` would default to read-only. Plan
/// mode runs the same way; Codex itself holds back from changing things while planning.
fn preset(mode: Option<&str>) -> (&'static str, &'static str) {
    match mode {
        Some("readOnly") => ("read-only", "on-request"),
        Some("fullAccess") => ("danger-full-access", "never"),
        _ => ("workspace-write", "on-request"),
    }
}

/// The session rule "allow for this session" grants for file changes.
const EDIT_RULE: &str = "Edit";

/// Questions for the user show as Claude Code's question tool does, on the question card.
const QUESTION_TOOL: &str = "AskUserQuestion";

const INITIALIZE: u64 = 1;
const THREAD: u64 = 2;
const TURN: u64 = 3;
const INTERRUPT: u64 = 4;

/// Codex only asks the user questions (its request_user_input tool) in plan mode unless this
/// feature is on; Bach shows them on the question card, like Claude Code's.
const QUESTIONS_FEATURE: &str = "default_mode_request_user_input";

/// Satie's tools that only look (as Claude Code is allowed them without asking).
const SATIE_READ_ONLY: &[&str] = &["task_list", "task_logs", "port_info", "http_check"];

/// Where Codex reads its Satie token from (see [`env`]).
const SATIE_TOKEN_ENV: &str = "BACH_SATIE_TOKEN";

/// What Codex is told about Satie. Its shell commands end with the turn, like Claude Code's.
const SATIE_GUIDANCE: &str = "Anything that must keep running after your turn ends (dev servers, simulations, watchers, \
long jobs) has to be started with the `satie` MCP tool `task_start`, not from a shell command (a trailing `&`, `nohup`, \
`setsid`, tmux or screen): every process a shell command starts is stopped when the turn ends. `task_start` keeps the \
process running on its own, shows it to the user in the Tasks panel, and `task_logs`, `task_list` and `task_stop` manage it. \
Pass `port` when the process serves on one, so the call waits until it is up. For a process-compose project use \
`compose_start` with the compose file instead of running process-compose yourself: each of its processes then gets its own \
state and log, and `task_process` restarts one without the rest.";

/// `-c` values are TOML; a JSON string is also a TOML basic string.
fn toml_string(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// `satie` adds Bach's background-task launcher as the `satie` MCP server, with guidance on
/// when to use it. The token itself goes in the environment ([`env`]), not the command line.
pub fn args(satie: Option<&satie::Grant>) -> Vec<String> {
    args_with(satie, crate::wrapper::active())
}

/// Codex keeps only the last `developer_instructions` it is given, wherever they come from. A
/// wrapper may put its own there (describing its sandbox), and those must stand: under one,
/// Satie's tool descriptions are what steer Codex to it.
fn args_with(satie: Option<&satie::Grant>, wrapped: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["app-server".into(), "--enable".into(), QUESTIONS_FEATURE.into()];
    if let Some(grant) = satie {
        a.extend(["-c".into(), format!("mcp_servers.satie.url={}", toml_string(&grant.url))]);
        a.extend([
            "-c".into(),
            format!("mcp_servers.satie.bearer_token_env_var={}", toml_string(SATIE_TOKEN_ENV)),
        ]);
        if !wrapped {
            a.extend(["-c".into(), format!("developer_instructions={}", toml_string(SATIE_GUIDANCE))]);
        }
    }
    a
}

/// The environment Codex needs on top of Bach's: the Satie token, when it has Satie.
pub fn env(satie: Option<&satie::Grant>) -> Vec<(&'static str, String)> {
    satie
        .map(|g| (SATIE_TOKEN_ENV, g.token.clone()))
        .into_iter()
        .collect()
}

/// One turn's conversation with the app server.
pub struct Conversation {
    prompt: String,
    /// Attachments saved as files: images go as images, the rest by path in the text.
    files: Vec<SavedFile>,
    thread_id: Option<String>,
    cwd: Option<String>,
    /// Session rules approved earlier: requests they cover are accepted without asking.
    allowed: Vec<String>,
    /// The model chosen for the session, if any.
    choice: Option<String>,
    /// See [`PERMISSION_MODES`].
    mode: Option<String>,
    /// Whether Codex may use its own sandbox. Not under a wrapper, unless asked for: it sandboxes
    /// Codex itself, and Codex can't start its bubblewrap inside another one, so every command
    /// would fail. Without it Codex runs with full access, inside the wrapper's sandbox.
    own_sandbox: bool,
    /// Reasoning effort for the turn, if chosen.
    effort: Option<String>,
    /// The model the thread runs (chosen, or Codex's default).
    model: Option<String>,
    /// Whether the model's context window has been reported (it doesn't change mid-turn).
    window_sent: bool,
    turn_id: Option<String>,
    /// File changes by item id: the approval request itself doesn't say what would change.
    changes: HashMap<String, Value>,
    /// The approval each of Codex's open requests became, by the request's JSON-RPC id.
    requests: HashMap<String, String>,
}

/// What an approval card for one of Codex's requests shows, and what its reply needs.
struct Ask {
    tool_name: String,
    input: Value,
    description: Option<String>,
    rules: Vec<String>,
    directories: Vec<String>,
    reply: Value,
    /// Allowed without asking (Satie calls that only read).
    harmless: bool,
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

/// The prompt, and where to find the attachments Codex can't take as images (PDFs, text files):
/// it reads them itself.
fn prompt_text(prompt: &str, files: &[SavedFile]) -> String {
    let paths: Vec<String> = files
        .iter()
        .filter(|f| f.kind != Kind::Image)
        .map(|f| format!("- {}", f.path))
        .collect();
    match (prompt.is_empty(), paths.is_empty()) {
        (_, true) => prompt.to_string(),
        (true, false) => format!("Attached files:\n{}", paths.join("\n")),
        (false, false) => format!("{prompt}\n\nAttached files:\n{}", paths.join("\n")),
    }
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
    pub fn new(turn: &Turn) -> Self {
        Self {
            prompt: turn.prompt.into(),
            files: turn.files.to_vec(),
            thread_id: turn.session_id.map(String::from),
            cwd: turn.cwd.map(String::from),
            allowed: turn.allowed_tools.to_vec(),
            choice: turn.model.map(String::from),
            mode: turn.permission_mode.map(String::from),
            own_sandbox: crate::wrapper::codex_sandbox(),
            effort: turn.effort.map(String::from),
            model: None,
            window_sent: false,
            turn_id: None,
            changes: HashMap::new(),
            requests: HashMap::new(),
        }
    }

    /// The first lines to send.
    pub fn opening(&self) -> Vec<String> {
        vec![request(
            INITIALIZE,
            "initialize",
            // The experimental API is what lets a turn run in plan mode.
            json!({
                "clientInfo": { "name": "bach", "title": "Bach", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true },
            }),
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
                // Also given when resuming: the session's mode or model may have changed, and a
                // thread first run by `codex exec` would otherwise keep its read-only sandbox.
                let (sandbox, approval) = preset(self.mode.as_deref());
                let sandbox = if self.own_sandbox { sandbox } else { "danger-full-access" };
                let mut params = json!({ "approvalPolicy": approval, "sandbox": sandbox });
                if let Some(cwd) = &self.cwd {
                    params["cwd"] = json!(cwd);
                }
                if let Some(model) = &self.choice {
                    params["model"] = json!(model);
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
                let text = prompt_text(&self.prompt, &self.files);
                if !text.is_empty() {
                    input.push(json!({ "type": "text", "text": text, "text_elements": [] }));
                }
                input.extend(
                    self.files
                        .iter()
                        .filter(|f| f.kind == Kind::Image)
                        .map(|f| json!({ "type": "localImage", "path": f.path })),
                );
                let mut params = json!({ "threadId": id, "input": input });
                if let Some(effort) = &self.effort {
                    params["effort"] = json!(effort);
                }
                // Always said, so leaving plan mode takes effect on a resumed thread too.
                if let Some(model) = &self.model {
                    let mode = if self.mode.as_deref() == Some("plan") { "plan" } else { "default" };
                    params["collaborationMode"] = json!({
                        "mode": mode,
                        "settings": { "model": model, "reasoning_effort": self.effort, "developer_instructions": null },
                    });
                }
                (
                    vec![AgentEvent::Session {
                        id: id.clone(),
                        model: self.model.clone(),
                    }],
                    vec![request(TURN, "turn/start", params)],
                )
            }
            Some(TURN) => {
                self.turn_id = result["turn"]["id"].as_str().map(String::from);
                (vec![], vec![])
            }
            _ => (vec![], vec![]),
        }
    }

    /// Something Codex needs answered before it carries on: an approval, a question for the
    /// user, or a form from an MCP server. Each becomes an approval card; what the reply needs
    /// is kept in `suggestions` (see [`answer`]).
    fn on_request(&mut self, id: &Value, method: &str, p: &Value) -> (Vec<AgentEvent>, Vec<String>) {
        let item_id = p["itemId"].as_str().map(String::from);
        // Unique within the session (the server's own ids restart with every process).
        let scope = p["approvalId"]
            .as_str()
            .or(item_id.as_deref())
            .or(p["turnId"].as_str())
            .unwrap_or_default();
        let request_id = format!("{scope}/{id}");
        let Some(ask) = self.ask(method, p) else {
            let message = format!("Bach can't answer `{method}` yet.");
            let error = json!({ "id": id, "error": { "code": -32601, "message": message } });
            return (vec![], vec![error.to_string()]);
        };
        let mut suggestions = ask.reply;
        suggestions["rpcId"] = id.clone();
        // Harmless, or covered by a rule approved earlier this session: allowed without asking.
        if ask.harmless || (!ask.rules.is_empty() && ask.rules.iter().all(|r| self.allowed.contains(r))) {
            let accept = answer(&suggestions, bach_protocol::Decision::Allow, None)
                .expect("allowing needs no answers");
            return (vec![], vec![accept]);
        }
        self.requests.insert(id.to_string(), request_id.clone());
        (
            vec![AgentEvent::Approval {
                request_id,
                tool_use_id: item_id,
                tool_name: ask.tool_name,
                input: ask.input,
                description: ask.description,
                reason: p["reason"].as_str().map(String::from),
                rules: ask.rules,
                directories: ask.directories,
                suggestions,
            }],
            vec![],
        )
    }

    fn ask(&self, method: &str, p: &Value) -> Option<Ask> {
        Some(match method {
            "item/commandExecution/requestApproval" => {
                let command = unwrap_shell(p["command"].as_str().unwrap_or_default());
                let cwd = s(&p["cwd"]);
                let mut input = json!({ "command": command });
                if !cwd.is_empty() {
                    input["cwd"] = json!(cwd);
                }
                Ask {
                    tool_name: "Shell".into(),
                    input,
                    description: None,
                    rules: vec![shell_rule(&command)],
                    // Kept only when outside the project (see `Runs`).
                    directories: [cwd].into_iter().filter(|d| !d.is_empty()).collect(),
                    reply: json!({
                        "kind": "command",
                        "amendment": p["proposedExecpolicyAmendment"],
                        "available": p["availableDecisions"],
                    }),
                    harmless: false,
                }
            }
            "item/fileChange/requestApproval" => {
                let changes = p["itemId"]
                    .as_str()
                    .and_then(|i| self.changes.get(i))
                    .cloned()
                    .unwrap_or(json!([]));
                Ask {
                    tool_name: "Edit".into(),
                    input: file_change_input(&changes),
                    description: None,
                    rules: vec![EDIT_RULE.to_string()],
                    directories: vec![],
                    reply: json!({ "kind": "fileChange" }),
                    harmless: false,
                }
            }
            // More access than the sandbox gives: the network, or folders outside the project.
            "item/permissions/requestApproval" => {
                let wanted = &p["permissions"];
                let list = |k: &str| -> Vec<String> {
                    wanted["fileSystem"][k]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| d.as_str().map(String::from))
                        .collect()
                };
                let (read, write) = (list("read"), list("write"));
                let network = wanted["network"]["enabled"] == true;
                let mut input = json!({});
                let mut asks = vec![];
                if network {
                    input["network"] = json!(true);
                    asks.push("use the network".to_string());
                }
                if !write.is_empty() {
                    asks.push(format!("write to {}", write.join(", ")));
                    input["write"] = json!(write);
                }
                if !read.is_empty() {
                    asks.push(format!("read {}", read.join(", ")));
                    input["read"] = json!(read);
                }
                Ask {
                    tool_name: "Permissions".into(),
                    input,
                    description: (!asks.is_empty()).then(|| format!("Let Codex {}", asks.join(" and "))),
                    // Granted for this turn only: Bach has no rule for it. The folders are in
                    // the description (the card's folder note is about single commands).
                    rules: vec![],
                    directories: vec![],
                    reply: json!({ "kind": "permissions", "permissions": granted(wanted) }),
                    harmless: false,
                }
            }
            // Codex's own questions for the user (its request_user_input tool).
            "item/tool/requestUserInput" => {
                let mut ids = serde_json::Map::new();
                let questions: Vec<Value> = p["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|q| {
                        let text = s(&q["question"]);
                        ids.insert(text.clone(), q["id"].clone());
                        json!({
                            "question": text,
                            "header": q["header"],
                            "options": q["options"].as_array().cloned().unwrap_or_default(),
                            "multiSelect": false,
                        })
                    })
                    .collect();
                Ask {
                    tool_name: QUESTION_TOOL.into(),
                    input: json!({ "questions": questions }),
                    description: None,
                    rules: vec![],
                    directories: vec![],
                    reply: json!({ "kind": "questions", "ids": ids }),
                    harmless: false,
                }
            }
            // A form an MCP server wants filled in, asked as questions, or a page to visit. Codex
            // also asks this way before it calls an MCP tool.
            "mcpServer/elicitation/request" => {
                let server = s(&p["serverName"]);
                let message = s(&p["message"]);
                let meta = &p["_meta"];
                if meta["codex_approval_kind"] == "mcp_tool_call" {
                    // Only the message names the tool: `…run tool "task_start"?`
                    let tool = message
                        .split('"')
                        .nth(1)
                        .filter(|t| !t.is_empty())
                        .unwrap_or("tool");
                    let name = format!("mcp__{server}__{tool}");
                    Ask {
                        harmless: server == "satie" && SATIE_READ_ONLY.contains(&tool),
                        rules: vec![name.clone()],
                        tool_name: name,
                        input: meta["tool_params"].clone(),
                        description: None,
                        directories: vec![],
                        reply: json!({ "kind": "mcpTool", "persist": meta["persist"] }),
                    }
                } else if p["mode"] == "url" {
                    Ask {
                        tool_name: "Open link".into(),
                        input: json!({ "path": p["url"], "server": server }),
                        description: Some(message),
                        rules: vec![],
                        directories: vec![],
                        reply: json!({ "kind": "elicitation", "fields": {} }),
                        harmless: false,
                    }
                } else {
                    let (questions, fields) = form_questions(&message, &p["requestedSchema"]);
                    Ask {
                        tool_name: QUESTION_TOOL.into(),
                        input: json!({ "questions": questions }),
                        description: None,
                        rules: vec![],
                        directories: vec![],
                        reply: json!({ "kind": "elicitation", "fields": fields }),
                        harmless: false,
                    }
                }
            }
            _ => return None,
        })
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
            // A plan mode turn's plan is its answer.
            ("item/completed", Some("agentMessage" | "plan")) => {
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
            // Answered, or no longer needed (the turn was interrupted, or a question timed out).
            // `Runs` drops it when it's one Bach already answered.
            ("serverRequest/resolved", _) => self
                .requests
                .remove(&p["requestId"].to_string())
                .map(|request_id| AgentEvent::ApprovalCancelled { request_id })
                .into_iter()
                .collect(),
            // Things still being written: the message, the reasoning, a command's output.
            ("item/agentMessage/delta", _)
            | ("item/plan/delta", _)
            | ("item/reasoning/summaryTextDelta", _)
            | ("item/reasoning/textDelta", _)
            | ("item/commandExecution/outputDelta", _) => {
                let kind = match method {
                    "item/agentMessage/delta" | "item/plan/delta" => DeltaKind::Text,
                    "item/commandExecution/outputDelta" => DeltaKind::Output,
                    _ => DeltaKind::Thinking,
                };
                let text = s(&p["delta"]);
                if text.is_empty() {
                    return vec![];
                }
                vec![AgentEvent::Delta {
                    id: s(&p["itemId"]),
                    kind,
                    text,
                }]
            }
            // A new part of the reasoning summary starts a new paragraph.
            ("item/reasoning/summaryPartAdded", _) if p["summaryIndex"].as_u64() > Some(0) => {
                vec![AgentEvent::Delta {
                    id: s(&p["itemId"]),
                    kind: DeltaKind::Thinking,
                    text: "\n\n".into(),
                }]
            }
            _ => vec![],
        }
    }
}

/// Asks a short-lived app server for the models the user's Codex can run (its `model/list`).
pub async fn list_models() -> Result<Vec<ModelInfo>, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let program = crate::wrapper::command("codex");
    let mut child = tokio::process::Command::new(&program[0])
        .args(&program[1..])
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("couldn't start `codex`: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let lines = [
        request(INITIALIZE, "initialize", json!({ "clientInfo": { "name": "bach", "title": "Bach", "version": env!("CARGO_PKG_VERSION") } })),
        json!({ "method": "initialized" }).to_string(),
        request(2, "model/list", json!({})),
    ];
    for line in lines {
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|e| format!("couldn't ask Codex for its models: {e}"))?;
    }
    let mut out = BufReader::new(child.stdout.take().expect("piped")).lines();
    let answer = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Ok(Some(line)) = out.next_line().await {
            let v: Value = serde_json::from_str(&line).unwrap_or_default();
            if v["id"] == 2 {
                return Some(v);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or("Codex didn't list its models.")?;
    if let Some(m) = answer["error"]["message"].as_str() {
        return Err(format!("Codex couldn't list its models: {m}"));
    }
    Ok(models_from(&answer["result"]))
}

/// The visible models in a `model/list` result.
fn models_from(result: &Value) -> Vec<ModelInfo> {
    result["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["hidden"] != true)
        .map(|m| ModelInfo {
            id: s(&m["model"]),
            name: m["displayName"].as_str().unwrap_or(m["model"].as_str().unwrap_or_default()).into(),
            description: s(&m["description"]),
            is_default: m["isDefault"] == true,
            efforts: m["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["reasoningEffort"].as_str().map(String::from))
                .collect(),
            default_effort: m["defaultReasoningEffort"].as_str().map(String::from),
        })
        .filter(|m| !m.id.is_empty())
        .collect()
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

/// The reply to an approval request, from what was kept of it (`suggestions`). `answers` are
/// the user's answers to questions, keyed by question text.
///
/// Bach remembers "for this session" rules itself and answers later requests they cover, so
/// a plain `accept` does when Codex doesn't offer `acceptForSession` (it lists what it takes in
/// `availableDecisions`, when it says).
pub fn answer(
    suggestions: &Value,
    decision: bach_protocol::Decision,
    answers: Option<&HashMap<String, String>>,
) -> Result<String, String> {
    use bach_protocol::Decision::*;
    let kind = suggestions["kind"].as_str().unwrap_or_default();
    // Questions, and forms with fields (a form that's only a link to visit has none).
    let takes_answers = kind == "questions"
        || (kind == "elicitation" && suggestions["fields"].as_object().is_some_and(|f| !f.is_empty()));
    if answers.is_some() && !takes_answers {
        return Err("That request doesn't take answers.".into());
    }
    let result = match (kind, decision) {
        ("questions", Deny) => json!({ "answers": {} }),
        ("questions", _) => {
            let answers = answers.ok_or("Answer the question first.")?;
            let ids = &suggestions["ids"];
            let mut out = serde_json::Map::new();
            for (question, answer) in answers {
                let id = ids[question]
                    .as_str()
                    .ok_or_else(|| format!("`{question}` isn't one of the questions asked."))?;
                out.insert(id.into(), json!({ "answers": [answer] }));
            }
            json!({ "answers": out })
        }
        ("mcpTool", Deny) => json!({ "action": "decline", "content": null, "_meta": null }),
        // "For this session" and "always" are remembered by Codex when it offers them.
        ("mcpTool", _) => {
            let persist = match decision {
                AllowSession => Some("session"),
                AllowAlways => Some("always"),
                _ => None,
            }
            .filter(|p| suggestions["persist"].as_array().is_some_and(|a| a.iter().any(|x| x == p)));
            let meta = persist.map(|p| json!({ "persist": p })).unwrap_or(Value::Null);
            json!({ "action": "accept", "content": {}, "_meta": meta })
        }
        ("elicitation", Deny) => json!({ "action": "decline", "content": null, "_meta": null }),
        ("elicitation", _) => {
            let fields = &suggestions["fields"];
            let mut content = serde_json::Map::new();
            for (question, answer) in answers.into_iter().flatten() {
                let field = fields
                    .get(question)
                    .ok_or_else(|| format!("`{question}` isn't one of the questions asked."))?;
                content.insert(s(&field["name"]), field_value(field, answer)?);
            }
            let content = if takes_answers { json!(content) } else { Value::Null };
            json!({ "action": "accept", "content": content, "_meta": null })
        }
        ("permissions", Deny) => json!({ "permissions": {}, "scope": "turn" }),
        ("permissions", _) => json!({ "permissions": suggestions["permissions"], "scope": "turn" }),
        _ => {
            let offered = |name: &str| match suggestions["available"].as_array() {
                Some(a) => a.iter().any(|d| d == name || d.get(name).is_some()),
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
            json!({ "decision": decision })
        }
    };
    Ok(reply(&suggestions["rpcId"], result))
}

/// The requested permissions as a grant: the same, without the unset parts.
fn granted(wanted: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if wanted["network"].is_object() {
        out.insert("network".into(), wanted["network"].clone());
    }
    if let Some(fs) = wanted["fileSystem"].as_object() {
        let fs: serde_json::Map<String, Value> =
            fs.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect();
        out.insert("fileSystem".into(), json!(fs));
    }
    json!(out)
}

/// An MCP form as questions for the question card, and for each question the field it fills
/// in (`name`, and the schema `type` to turn the answer back into).
fn form_questions(message: &str, schema: &Value) -> (Vec<Value>, Value) {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str())
        .collect();
    let mut fields = serde_json::Map::new();
    let mut questions = vec![];
    for (name, prop) in schema["properties"].as_object().into_iter().flatten() {
        let title = prop["title"].as_str().unwrap_or(name);
        let mut question = match prop["description"].as_str() {
            Some(d) => format!("{title}: {d}"),
            None => title.to_string(),
        };
        // The first question carries the server's message; question texts must be distinct.
        if questions.is_empty() && !message.is_empty() {
            question = format!("{message}\n\n{question}");
        }
        if !required.contains(&name.as_str()) {
            question.push_str(" (optional)");
        }
        let option = |label: &Value, description: &Value| {
            json!({ "label": label, "description": description.as_str().unwrap_or_default() })
        };
        let items = if prop["type"] == "array" { &prop["items"] } else { prop };
        let options: Vec<Value> = if prop["type"] == "boolean" {
            vec![option(&json!("Yes"), &Value::Null), option(&json!("No"), &Value::Null)]
        } else if let Some(values) = items["enum"].as_array() {
            let names = items["enumNames"].as_array();
            values
                .iter()
                .enumerate()
                .map(|(i, v)| option(names.and_then(|n| n.get(i)).unwrap_or(v), &Value::Null))
                .collect()
        } else if let Some(one_of) = items["oneOf"].as_array().or(items["anyOf"].as_array()) {
            one_of.iter().map(|o| option(&o["title"], &Value::Null)).collect()
        } else {
            vec![]
        };
        // Labels map back to values when answered.
        let values: Vec<Value> = if let Some(values) = items["enum"].as_array() {
            values.clone()
        } else if let Some(one_of) = items["oneOf"].as_array().or(items["anyOf"].as_array()) {
            one_of.iter().map(|o| o["const"].clone()).collect()
        } else {
            vec![]
        };
        let labels: Vec<Value> = options.iter().map(|o| o["label"].clone()).collect();
        fields.insert(
            question.clone(),
            json!({ "name": name, "type": prop["type"], "labels": labels, "values": values }),
        );
        questions.push(json!({
            "question": question,
            "header": title,
            "options": options,
            "multiSelect": prop["type"] == "array",
        }));
    }
    (questions, json!(fields))
}

/// An answer from the question card (option labels, joined with ", " when several are
/// picked) as the value the form field takes.
fn field_value(field: &Value, answer: &str) -> Result<Value, String> {
    let labels = field["labels"].as_array().cloned().unwrap_or_default();
    let values = field["values"].as_array().cloned().unwrap_or_default();
    // A label picked from the options stands for its value; anything else was typed in.
    let pick = |a: &str| -> Value {
        labels
            .iter()
            .position(|l| l == a)
            .and_then(|i| values.get(i).cloned())
            .unwrap_or_else(|| json!(a))
    };
    Ok(match field["type"].as_str() {
        Some("boolean") => json!(answer == "Yes"),
        Some("array") => json!(answer.split(", ").map(pick).collect::<Vec<_>>()),
        Some("number") => json!(answer
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("`{answer}` isn't a number."))?),
        Some("integer") => json!(answer
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("`{answer}` isn't a whole number."))?),
        _ => pick(answer),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use bach_protocol::Decision;

    #[test]
    fn attachments_other_than_images_are_named_in_the_text() {
        let file = |path: &str, kind| SavedFile { path: path.into(), mime: String::new(), kind };
        let files = [file("/t/1/a.png", Kind::Image), file("/t/2/b.pdf", Kind::Pdf), file("/t/3/c.txt", Kind::Text)];
        assert_eq!(prompt_text("read", &files), "read\n\nAttached files:\n- /t/2/b.pdf\n- /t/3/c.txt");
        assert_eq!(prompt_text("", &files[1..2]), "Attached files:\n- /t/2/b.pdf");
        assert_eq!(prompt_text("look", &files[..1]), "look");
    }

    fn conversation(allowed: &[&str]) -> Conversation {
        let allowed: Vec<String> = allowed.iter().map(|r| r.to_string()).collect();
        Conversation::new(&Turn {
            prompt: "hi",
            session_id: Some("t1"),
            allowed_tools: &allowed,
            ..Default::default()
        })
    }

    /// The approval a request from Codex becomes, and what the conversation sent back at once.
    fn ask(c: &mut Conversation, line: Value) -> (Option<AgentEvent>, Vec<String>) {
        let (events, replies) = c.on_line(&line);
        (events.into_iter().next(), replies)
    }

    fn suggestions(ev: &Option<AgentEvent>) -> &Value {
        match ev {
            Some(AgentEvent::Approval { suggestions, .. }) => suggestions,
            other => panic!("not an approval: {other:?}"),
        }
    }

    fn answered(suggestions: &Value, d: Decision, answers: &[(&str, &str)]) -> Result<Value, String> {
        let answers: HashMap<String, String> =
            answers.iter().map(|(q, a)| (q.to_string(), a.to_string())).collect();
        let line = answer(suggestions, d, (!answers.is_empty()).then_some(&answers))?;
        Ok(serde_json::from_str(&line).unwrap())
    }

    #[test]
    fn questions_are_answered_by_question_id() {
        let mut c = conversation(&[]);
        let (ev, _) = ask(&mut c, json!({ "id": 5, "method": "item/tool/requestUserInput", "params": {
            "threadId": "t1", "turnId": "u1", "itemId": "q1", "autoResolutionMs": null,
            "questions": [
                { "id": "colour", "header": "Colour", "question": "Which colour?", "isOther": true, "isSecret": false,
                  "options": [{ "label": "Red", "description": "warm" }, { "label": "Blue", "description": "cool" }] },
                { "id": "name", "header": "Name", "question": "What name?", "isOther": false, "isSecret": false, "options": null },
            ],
        }}));
        let Some(AgentEvent::Approval { tool_name, input, rules, request_id, .. }) = &ev else { panic!() };
        assert_eq!(tool_name, "AskUserQuestion");
        assert_eq!(request_id, "q1/5");
        assert!(rules.is_empty());
        assert_eq!(input["questions"][0]["options"][1]["label"], "Blue");
        assert_eq!(input["questions"][1]["options"], json!([]));

        let s = suggestions(&ev);
        let r = answered(s, Decision::Allow, &[("Which colour?", "Blue"), ("What name?", "Bach")]).unwrap();
        assert_eq!(r["id"], 5);
        assert_eq!(r["result"]["answers"]["colour"]["answers"], json!(["Blue"]));
        assert_eq!(r["result"]["answers"]["name"]["answers"], json!(["Bach"]));
        assert!(answered(s, Decision::Allow, &[]).is_err());
        assert!(answered(s, Decision::Allow, &[("Something else?", "x")]).is_err());
        assert_eq!(answered(s, Decision::Deny, &[]).unwrap()["result"], json!({ "answers": {} }));
    }

    #[test]
    fn extra_permissions_are_granted_for_the_turn() {
        // Having rules approved doesn't cover a request that has none.
        let mut c = conversation(&["Edit"]);
        let (ev, replies) = ask(&mut c, json!({ "id": 2, "method": "item/permissions/requestApproval", "params": {
            "threadId": "t1", "turnId": "u1", "itemId": "p1", "environmentId": null, "startedAtMs": 0,
            "cwd": "/p", "reason": "to fetch crates",
            "permissions": { "network": { "enabled": true }, "fileSystem": { "read": null, "write": ["/data"] } },
        }}));
        assert!(replies.is_empty());
        let Some(AgentEvent::Approval { tool_name, description, reason, .. }) = &ev else { panic!() };
        assert_eq!(tool_name, "Permissions");
        assert_eq!(description.as_deref(), Some("Let Codex use the network and write to /data"));
        assert_eq!(reason.as_deref(), Some("to fetch crates"));

        let s = suggestions(&ev);
        assert_eq!(
            answered(s, Decision::Allow, &[]).unwrap()["result"],
            json!({ "permissions": { "network": { "enabled": true }, "fileSystem": { "write": ["/data"] } }, "scope": "turn" })
        );
        assert_eq!(
            answered(s, Decision::Deny, &[]).unwrap()["result"],
            json!({ "permissions": {}, "scope": "turn" })
        );
    }

    #[test]
    fn mcp_forms_become_questions_and_typed_answers() {
        let mut c = conversation(&[]);
        let (ev, _) = ask(&mut c, json!({ "id": 9, "method": "mcpServer/elicitation/request", "params": {
            "threadId": "t1", "turnId": "u1", "serverName": "deploy", "mode": "form", "_meta": null,
            "message": "Deploy settings",
            "requestedSchema": { "type": "object", "required": ["env", "replicas"], "properties": {
                "env": { "type": "string", "title": "Environment", "enum": ["prod", "stage"], "enumNames": ["Production", "Staging"] },
                "replicas": { "type": "integer", "title": "Replicas" },
                "dry": { "type": "boolean", "title": "Dry run" },
                "regions": { "type": "array", "title": "Regions", "items": { "enum": ["eu", "us"] } },
            }},
        }}));
        let Some(AgentEvent::Approval { tool_name, input, request_id, .. }) = &ev else { panic!() };
        assert_eq!(tool_name, "AskUserQuestion");
        assert_eq!(request_id, "u1/9");
        let questions: Vec<&str> = input["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["question"].as_str().unwrap())
            .collect();
        // serde_json keeps object keys sorted.
        assert_eq!(questions, ["Deploy settings\n\nDry run (optional)", "Environment", "Regions (optional)", "Replicas"]);
        assert_eq!(input["questions"][1]["options"][0]["label"], "Production");
        assert_eq!(input["questions"][2]["multiSelect"], true);

        let s = suggestions(&ev);
        let r = answered(s, Decision::Allow, &[
            ("Deploy settings\n\nDry run (optional)", "Yes"),
            ("Environment", "Staging"),
            ("Regions (optional)", "eu, us"),
            ("Replicas", "3"),
        ])
        .unwrap();
        assert_eq!(r["result"]["action"], "accept");
        assert_eq!(r["result"]["content"], json!({ "dry": true, "env": "stage", "regions": ["eu", "us"], "replicas": 3 }));
        assert!(answered(s, Decision::Allow, &[("Replicas", "three")]).is_err());
        assert_eq!(answered(s, Decision::Deny, &[]).unwrap()["result"]["action"], "decline");
    }

    /// Codex asking before it calls an MCP tool (recorded from 0.146.0, trimmed).
    fn tool_call(id: u64, tool: &str) -> Value {
        json!({ "id": id, "method": "mcpServer/elicitation/request", "params": {
            "threadId": "t1", "turnId": "u1", "serverName": "satie", "mode": "form",
            "message": format!("Allow the satie MCP server to run tool \"{tool}\"?"),
            "requestedSchema": { "type": "object", "properties": {} },
            "_meta": { "codex_approval_kind": "mcp_tool_call", "persist": ["session", "always"],
                       "tool_params": { "command": "python3 -m http.server 3977", "port": 3977 } },
        }})
    }

    #[test]
    fn mcp_tool_calls_are_approvals() {
        let mut c = conversation(&[]);
        let (ev, replies) = ask(&mut c, tool_call(4, "task_start"));
        assert!(replies.is_empty());
        let Some(AgentEvent::Approval { tool_name, input, rules, .. }) = &ev else { panic!() };
        assert_eq!(tool_name, "mcp__satie__task_start");
        assert_eq!(input["port"], 3977);
        assert_eq!(rules, &["mcp__satie__task_start"]);

        let s = suggestions(&ev);
        let reply = |d| answered(s, d, &[]).unwrap()["result"].clone();
        assert_eq!(reply(Decision::Allow), json!({ "action": "accept", "content": {}, "_meta": null }));
        assert_eq!(reply(Decision::AllowSession)["_meta"], json!({ "persist": "session" }));
        assert_eq!(reply(Decision::AllowAlways)["_meta"], json!({ "persist": "always" }));
        assert_eq!(reply(Decision::Deny)["action"], "decline");
        assert!(answered(s, Decision::Allow, &[("x", "y")]).is_err());

        // Looking at tasks needs no card; nor does a tool approved earlier this session.
        for (mut c, tool) in [(conversation(&[]), "task_logs"), (conversation(&["mcp__satie__task_start"]), "task_start")] {
            let (ev, replies) = ask(&mut c, tool_call(5, tool));
            assert!(ev.is_none(), "{tool}");
            let r: Value = serde_json::from_str(&replies[0]).unwrap();
            assert_eq!(r, json!({ "id": 5, "result": { "action": "accept", "content": {}, "_meta": null } }));
        }
    }

    #[test]
    fn no_satie_no_mcp_server() {
        assert_eq!(args(None), ["app-server", "--enable", QUESTIONS_FEATURE]);
    }

    #[test]
    fn requests_codex_withdraws_are_cancelled() {
        let mut c = conversation(&[]);
        ask(&mut c, json!({ "id": 0, "method": "item/fileChange/requestApproval", "params": { "threadId": "t1", "turnId": "u1", "itemId": "f1" } }));
        let (ev, _) = c.on_line(&json!({ "method": "serverRequest/resolved", "params": { "threadId": "t1", "requestId": 0 } }));
        assert!(matches!(&ev[..], [AgentEvent::ApprovalCancelled { request_id }] if request_id == "f1/0"));
        // Only once, and not for requests it never asked.
        let (ev, _) = c.on_line(&json!({ "method": "serverRequest/resolved", "params": { "threadId": "t1", "requestId": 0 } }));
        assert!(ev.is_empty());
    }

    #[test]
    fn unknown_requests_get_an_error_not_silence() {
        let mut c = conversation(&[]);
        let (ev, replies) = ask(&mut c, json!({ "id": 3, "method": "item/somethingNew", "params": {} }));
        assert!(ev.is_none());
        let r: Value = serde_json::from_str(&replies[0]).unwrap();
        assert_eq!(r["id"], 3);
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn text_reasoning_and_output_stream() {
        let mut c = conversation(&[]);
        let mut delta = |method: &str, params: Value| match &c.on_line(&json!({ "method": method, "params": params })).0[..] {
            [AgentEvent::Delta { id, kind, text }] => Some((id.clone(), *kind, text.clone())),
            [] => None,
            other => panic!("{other:?}"),
        };
        let d = |id: &str, kind, text: &str| Some((id.to_string(), kind, text.to_string()));
        assert_eq!(delta("item/agentMessage/delta", json!({ "itemId": "m1", "delta": "Hel" })), d("m1", DeltaKind::Text, "Hel"));
        assert_eq!(delta("item/commandExecution/outputDelta", json!({ "itemId": "c1", "delta": "ok\n" })), d("c1", DeltaKind::Output, "ok\n"));
        assert_eq!(delta("item/reasoning/summaryTextDelta", json!({ "itemId": "r1", "delta": "Plan", "summaryIndex": 0 })), d("r1", DeltaKind::Thinking, "Plan"));
        // The first part needs no break; later ones start a paragraph.
        assert_eq!(delta("item/reasoning/summaryPartAdded", json!({ "itemId": "r1", "summaryIndex": 0 })), None);
        assert_eq!(delta("item/reasoning/summaryPartAdded", json!({ "itemId": "r1", "summaryIndex": 1 })), d("r1", DeltaKind::Thinking, "\n\n"));
        assert_eq!(delta("item/agentMessage/delta", json!({ "itemId": "m1", "delta": "" })), None);
    }

    /// The requests a new turn sends for `model` and `mode`: the thread's, then the turn's.
    fn opening(model: Option<&str>, mode: Option<&str>) -> (Value, Value) {
        let mut c = Conversation::new(&Turn { prompt: "hi", model, permission_mode: mode, ..Default::default() });
        let init: Value = serde_json::from_str(&c.opening()[0]).unwrap();
        assert_eq!(init["params"]["capabilities"]["experimentalApi"], true);
        let (_, sent) = c.on_line(&json!({ "id": 1, "result": {} }));
        let thread: Value = serde_json::from_str(&sent[1]).unwrap();
        let (_, sent) = c.on_line(&json!({ "id": 2, "result": { "thread": { "id": "t1" }, "model": "gpt-x" } }));
        (thread["params"].clone(), serde_json::from_str::<Value>(&sent[0]).unwrap()["params"].clone())
    }

    #[tokio::test]
    async fn leaves_a_wrappers_instructions_alone() {
        let dir = std::env::temp_dir().join(format!("bach-codex-args-{}", std::process::id()));
        let satie = satie::Satie::start("127.0.0.1:0".parse().unwrap(), dir.clone()).await.unwrap();
        let grant = satie.grant(satie::Scope::default());
        let has = |a: &[String], key: &str| a.iter().any(|x| x.starts_with(key));
        let direct = args_with(Some(&grant), false);
        assert!(has(&direct, "mcp_servers.satie.url=") && has(&direct, "developer_instructions="));
        let wrapped = args_with(Some(&grant), true);
        assert!(has(&wrapped, "mcp_servers.satie.url=") && !has(&wrapped, "developer_instructions="));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn permission_modes_and_models() {
        let modes = |mode| {
            let (thread, turn) = opening(None, mode);
            (
                thread["sandbox"].as_str().unwrap().to_string(),
                thread["approvalPolicy"].as_str().unwrap().to_string(),
                turn["collaborationMode"]["mode"].as_str().unwrap().to_string(),
            )
        };
        let m = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
        assert_eq!(modes(None), m("workspace-write", "on-request", "default"));
        // A mode left over from Claude Code means the default.
        assert_eq!(modes(Some("acceptEdits")), m("workspace-write", "on-request", "default"));
        assert_eq!(modes(Some("readOnly")), m("read-only", "on-request", "default"));
        assert_eq!(modes(Some("fullAccess")), m("danger-full-access", "never", "default"));
        assert_eq!(modes(Some("plan")), m("workspace-write", "on-request", "plan"));

        // A chosen model is asked for; plan mode names the thread's model either way.
        let (thread, turn) = opening(Some("gpt-y"), None);
        assert_eq!(thread["model"], "gpt-y");
        assert_eq!(turn["collaborationMode"]["settings"]["model"], "gpt-x");
        let (thread, _) = opening(None, None);
        assert!(thread.get("model").is_none());

        // Under a wrapper, Codex's own sandbox is off and the wrapper's stands.
        let mut c = Conversation::new(&Turn { prompt: "hi", ..Default::default() });
        c.own_sandbox = false;
        let (_, sent) = c.on_line(&json!({ "id": 1, "result": {} }));
        let thread: Value = serde_json::from_str(&sent[1]).unwrap();
        assert_eq!(thread["params"]["sandbox"], "danger-full-access");
        assert_eq!(thread["params"]["approvalPolicy"], "on-request");
    }

    #[test]
    fn effort_goes_with_the_turn() {
        let mut c = Conversation::new(&Turn { prompt: "hi", effort: Some("high"), permission_mode: Some("plan"), ..Default::default() });
        c.on_line(&json!({ "id": 1, "result": {} }));
        let (_, sent) = c.on_line(&json!({ "id": 2, "result": { "thread": { "id": "t1" }, "model": "gpt-x" } }));
        let turn: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(turn["params"]["effort"], "high");
        assert_eq!(turn["params"]["collaborationMode"]["settings"]["reasoning_effort"], "high");
        // None chosen: Codex's default.
        let (_, turn) = opening(None, None);
        assert!(turn.get("effort").is_none());
    }

    #[test]
    fn lists_visible_models() {
        let models = models_from(&json!({ "data": [
            { "model": "gpt-a", "displayName": "GPT A", "description": "fast", "hidden": false, "isDefault": true,
              "supportedReasoningEfforts": [{ "reasoningEffort": "low", "description": "" }, { "reasoningEffort": "high", "description": "" }],
              "defaultReasoningEffort": "low" },
            { "model": "gpt-old", "displayName": "Old", "description": "", "hidden": true, "isDefault": false },
            { "model": "gpt-b", "displayName": null, "description": "big", "hidden": false, "isDefault": false },
        ]}));
        let got: Vec<_> = models.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.is_default)).collect();
        assert_eq!(got, [("gpt-a", "GPT A", true), ("gpt-b", "gpt-b", false)]);
        assert_eq!((models[0].efforts.clone(), models[0].default_effort.as_deref()), (vec!["low".to_string(), "high".into()], Some("low")));
        assert!(models[1].efforts.is_empty());
    }

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
