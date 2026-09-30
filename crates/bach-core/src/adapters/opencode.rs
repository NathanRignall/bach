//! opencode through its server (`opencode serve`): one server for every project, a session per
//! Bach session, a prompt per turn, and events from its stream. This module only translates:
//! events into [`AgentEvent`]s, approval answers into HTTP replies. Talking to the server is
//! `crate::opencode_server`. Shapes recorded from opencode 1.15.10
//! (tests/fixtures/opencode_*.jsonl).
use super::{mode, AgentEvent, MCP_READ_ONLY, MCP_SERVER};
use crate::attachments::SavedFile;
use bach_protocol::{Decision, DeltaKind, ModelInfo};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The permission modes Bach offers for opencode (see [`permission_rules`]). Anything else (none,
/// or a mode left over from another agent) is "auto". opencode has no sandbox, so even "auto"
/// asks before shell commands.
pub fn permission_modes() -> Vec<bach_protocol::PermissionModeInfo> {
    vec![
        mode("auto", "Auto", "Edit the project; ask before shell commands", true),
        mode("manual", "Manual", "Ask before edits and shell commands", false),
        mode("plan", "Plan", "Create a plan before making changes", false),
        mode("fullAccess", "Full access", "Run everything without asking", false),
    ]
}

/// Questions show on the question card, as Claude Code's do.
const QUESTION_TOOL: &str = "AskUserQuestion";

/// The session's permission rules for a mode, then the rules approved earlier in the session
/// (`bash(git status *)`). opencode goes by the last rule that matches, after its agent's own
/// (which allow everything but a few things, like folders outside the project).
///
/// opencode has no sandbox, so "auto" asks before shell commands; edits in the project are
/// fine. "plan" runs opencode's plan agent with the same rules. Bach's MCP tools that only look
/// never ask, as with the other agents.
pub fn permission_rules(mode: Option<&str>, allowed: &[String]) -> Value {
    let rule = |permission: &str, pattern: &str, action: &str| {
        json!({ "permission": permission, "pattern": pattern, "action": action })
    };
    let mut rules = match mode {
        Some("fullAccess") => vec![rule("*", "*", "allow")],
        Some("manual") => vec![rule("edit", "*", "ask"), rule("bash", "*", "ask")],
        _ => vec![rule("bash", "*", "ask")],
    };
    rules.extend(MCP_READ_ONLY.map(|t| rule(&format!("{}{t}", mcp_prefix(MCP_SERVER)), "*", "allow")));
    rules.extend(allowed.iter().filter_map(|r| {
        let (permission, pattern) = parse_rule(r)?;
        Some(rule(permission, pattern, "allow"))
    }));
    json!(rules)
}

/// opencode's agent for a mode: its plan agent plans instead of changing things.
pub fn agent(mode: Option<&str>) -> &'static str {
    if mode == Some("plan") {
        "plan"
    } else {
        "build"
    }
}

/// `bash(git status *)` -> (`bash`, `git status *`)
fn parse_rule(rule: &str) -> Option<(&str, &str)> {
    let (permission, rest) = rule.split_once('(')?;
    Some((permission, rest.strip_suffix(')')?))
}

fn format_rule(permission: &str, pattern: &str) -> String {
    format!("{permission}({pattern})")
}

/// `provider/model` -> the prompt's `model`.
pub fn model(choice: &str) -> Option<Value> {
    let (provider, model) = choice.split_once('/')?;
    Some(json!({ "providerID": provider, "modelID": model }))
}

/// The prompt's parts: the text, then the attachments (by path).
pub fn prompt_parts(prompt: &str, files: &[SavedFile]) -> Value {
    let mut parts = vec![];
    if !prompt.is_empty() {
        parts.push(json!({ "type": "text", "text": prompt }));
    }
    for f in files {
        let name = f.path.rsplit('/').next().unwrap_or_default();
        parts.push(json!({ "type": "file", "mime": f.mime, "filename": name, "url": format!("file://{}", f.path) }));
    }
    json!(parts)
}

/// The models of `GET /config/providers`, as `provider/model`. `default` is the configured
/// model (`GET /config`'s `model`), if any.
pub fn models_from(providers: &Value, default: Option<&str>) -> Vec<ModelInfo> {
    let mut out = vec![];
    for p in providers["providers"].as_array().into_iter().flatten() {
        let provider = p["name"].as_str().unwrap_or_default();
        let Some(models) = p["models"].as_object() else { continue };
        for (id, m) in models {
            let id = format!("{}/{id}", p["id"].as_str().unwrap_or_default());
            out.push(ModelInfo {
                is_default: default == Some(id.as_str()),
                name: m["name"].as_str().unwrap_or(&id).to_string(),
                description: provider.to_string(),
                efforts: variants(&m["variants"]),
                default_effort: None,
                id,
            });
        }
    }
    out
}

/// A model's variants (opencode's thinking levels), least first. Their names vary by provider
/// (`minimal`, `low`, `high`, …); unknown ones go last.
fn variants(v: &Value) -> Vec<String> {
    const ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
    let mut names: Vec<String> = v.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
    names.sort_by_key(|n| ORDER.iter().position(|o| o == n).unwrap_or(ORDER.len()));
    names
}

/// A reply to the server: `POST path` with `body`.
#[derive(Debug, PartialEq)]
pub struct Reply {
    pub path: String,
    pub body: Value,
}

/// One turn of an opencode session, from its events.
pub struct Stream {
    session_id: String,
    /// Session rules approved earlier: requests they cover are allowed without asking.
    allowed: Vec<String>,
    /// Once the prompt has made the session busy, going idle ends the turn.
    busy: bool,
    /// The user's messages (their parts are the prompt, not the agent's output).
    user_messages: HashSet<String>,
    /// The type of each part seen, for telling text deltas from reasoning ones.
    part_types: HashMap<String, String>,
    /// Text and reasoning parts, by part id, in order, until their message completes.
    pending: Vec<(String, String, String)>,
    /// Tool calls shown so far, and the output of running commands shown so far.
    tools: HashSet<String>,
    outputs: HashMap<String, usize>,
    /// Requests shown as cards, by their id.
    asked: HashSet<String>,
    /// The MCP servers opencode has, whose tools it names `<server>_<tool>`.
    mcp_servers: Vec<String>,
    /// Arguments of the tool calls seen, by call id, for their approval cards.
    inputs: HashMap<String, Value>,
    /// MCP tool approvals waiting for their call's arguments, which opencode sends just after
    /// asking.
    waiting: Vec<AgentEvent>,
    cost: f64,
    failed: bool,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

impl Stream {
    /// `mcp_servers` are the names of opencode's MCP servers (its `GET /mcp`).
    pub fn new(session_id: &str, allowed: &[String], mcp_servers: &[String]) -> Self {
        Self {
            session_id: session_id.into(),
            allowed: allowed.to_vec(),
            busy: false,
            user_messages: HashSet::new(),
            part_types: HashMap::new(),
            pending: vec![],
            tools: HashSet::new(),
            outputs: HashMap::new(),
            asked: HashSet::new(),
            mcp_servers: mcp_servers.to_vec(),
            inputs: HashMap::new(),
            waiting: vec![],
            cost: 0.0,
            failed: false,
        }
    }

    /// One event from the server (`{ type, properties }`): the events to show, and replies to
    /// send at once (requests covered by session rules).
    pub fn on_event(&mut self, e: &Value) -> (Vec<AgentEvent>, Vec<Reply>) {
        let p = &e["properties"];
        let session = p["sessionID"]
            .as_str()
            .or(p["info"]["sessionID"].as_str())
            .or(p["part"]["sessionID"].as_str());
        if session != Some(self.session_id.as_str()) {
            return (vec![], vec![]);
        }
        match e["type"].as_str().unwrap_or_default() {
            "message.updated" => (self.on_message(&p["info"]), vec![]),
            "message.part.updated" => (self.on_part(&p["part"]), vec![]),
            "message.part.delta" => {
                let part = s(&p["partID"]);
                let kind = match self.part_types.get(&part).map(String::as_str) {
                    Some("text") => DeltaKind::Text,
                    Some("reasoning") => DeltaKind::Thinking,
                    _ => return (vec![], vec![]),
                };
                let text = s(&p["delta"]);
                if p["field"] != "text" || text.is_empty() {
                    return (vec![], vec![]);
                }
                (vec![AgentEvent::Delta { id: part, kind, text }], vec![])
            }
            "permission.asked" => self.on_permission(p),
            "question.asked" => (self.on_question(p), vec![]),
            // Answered, or no longer needed; `Runs` drops the ones Bach answered.
            "permission.replied" | "question.replied" | "question.rejected" => {
                let id = s(&p["requestID"]);
                self.waiting.retain(|a| !matches!(a, AgentEvent::Approval { request_id, .. } if *request_id == id));
                if self.asked.remove(&id) {
                    (vec![AgentEvent::ApprovalCancelled { request_id: id }], vec![])
                } else {
                    (vec![], vec![])
                }
            }
            // The model the session runs, as `provider/model`.
            "session.next.model.switched" => {
                let m = &p["model"];
                let model = format!("{}/{}", s(&m["providerID"]), s(&m["id"]));
                (vec![AgentEvent::Session { id: self.session_id.clone(), model: Some(model) }], vec![])
            }
            "session.status" => {
                if p["status"]["type"] == "busy" {
                    self.busy = true;
                }
                (vec![], vec![])
            }
            "session.error" => {
                self.failed = true;
                let error = &p["error"];
                // Stopping the run is reported by Bach itself.
                if error["name"] == "MessageAbortedError" {
                    return (vec![], vec![]);
                }
                let message = error["data"]["message"]
                    .as_str()
                    .or(error["name"].as_str())
                    .unwrap_or("opencode reported an error");
                (vec![AgentEvent::Error { message: message.into() }], vec![])
            }
            "session.idle" if self.busy => {
                self.busy = false;
                let mut events = std::mem::take(&mut self.waiting);
                events.extend(self.flush(None));
                events.push(AgentEvent::Done {
                    cost_usd: Some(self.cost),
                    is_error: self.failed,
                });
                (events, vec![])
            }
            _ => (vec![], vec![]),
        }
    }

    fn on_message(&mut self, info: &Value) -> Vec<AgentEvent> {
        let id = s(&info["id"]);
        if info["role"] == "user" {
            self.user_messages.insert(id);
            return vec![];
        }
        let mut events = vec![];
        if info["time"]["completed"].is_number() {
            events.extend(self.flush(Some(&id)));
        }
        if let Some(error) = info["error"].as_object() {
            if error.get("name").and_then(Value::as_str) != Some("MessageAbortedError") {
                self.failed = true;
                let message = info["error"]["data"]["message"]
                    .as_str()
                    .or(info["error"]["name"].as_str())
                    .unwrap_or("opencode reported an error");
                events.push(AgentEvent::Error { message: message.into() });
            }
        }
        events
    }

    /// The finished text and reasoning of message `id` (or all of it).
    fn flush(&mut self, message: Option<&str>) -> Vec<AgentEvent> {
        let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(_, m, _)| message.is_none_or(|id| id == m));
        self.pending = keep;
        done.into_iter()
            .filter_map(|(part, _, text)| {
                if text.trim().is_empty() {
                    return None;
                }
                Some(match self.part_types.get(&part).map(String::as_str) {
                    Some("reasoning") => AgentEvent::Thinking { text },
                    _ => AgentEvent::Text { text, parent: None },
                })
            })
            .collect()
    }

    fn on_part(&mut self, part: &Value) -> Vec<AgentEvent> {
        let id = s(&part["id"]);
        let kind = s(&part["type"]);
        let message = s(&part["messageID"]);
        self.part_types.insert(id.clone(), kind.clone());
        if self.user_messages.contains(&message) {
            return vec![];
        }
        match kind.as_str() {
            "text" | "reasoning" => {
                let text = s(&part["text"]);
                match self.pending.iter_mut().find(|(p, _, _)| *p == id) {
                    Some(entry) => entry.2 = text,
                    None => self.pending.push((id, message, text)),
                }
                vec![]
            }
            "tool" => self.on_tool(part),
            "step-finish" => {
                self.cost += part["cost"].as_f64().unwrap_or(0.0);
                part["tokens"]["total"]
                    .as_u64()
                    .map(|used| AgentEvent::Context { used })
                    .into_iter()
                    .collect()
            }
            _ => vec![],
        }
    }

    fn on_tool(&mut self, part: &Value) -> Vec<AgentEvent> {
        let tool = s(&part["tool"]);
        // Questions show on their own card.
        if tool == "question" {
            return vec![];
        }
        let id = s(&part["callID"]);
        let state = &part["state"];
        let status = state["status"].as_str().unwrap_or_default();
        let mut events = vec![];
        if status != "pending" {
            self.inputs.insert(id.clone(), state["input"].clone());
            events.extend(self.release(&id));
        }
        if status != "pending" && self.tools.insert(id.clone()) {
            events.push(AgentEvent::ToolUse {
                id: id.clone(),
                name: self.tool_name(&tool),
                input: state["input"].clone(),
                parent: None,
            });
        }
        match status {
            // A command's output so far, as it grows.
            "running" => {
                if let Some(output) = state["metadata"]["output"].as_str() {
                    let seen = self.outputs.entry(id.clone()).or_default();
                    if output.len() > *seen && output.is_char_boundary(*seen) {
                        events.push(AgentEvent::Delta {
                            id,
                            kind: DeltaKind::Output,
                            text: output[*seen..].to_string(),
                        });
                        *seen = output.len();
                    }
                }
            }
            "completed" | "error" => {
                self.outputs.remove(&id);
                let failed = status == "error";
                events.push(AgentEvent::ToolResult {
                    id,
                    output: if failed { s(&state["error"]) } else { s(&state["output"]) },
                    is_error: failed,
                    images: vec![],
                    parent: None,
                });
            }
            _ => {}
        }
        events
    }

    fn on_permission(&mut self, p: &Value) -> (Vec<AgentEvent>, Vec<Reply>) {
        let id = s(&p["id"]);
        let permission = s(&p["permission"]);
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        };
        let patterns = strings(&p["patterns"]);
        let rules: Vec<String> = strings(&p["always"])
            .iter()
            .map(|pattern| format_rule(&permission, pattern))
            .collect();
        let reply = json!({ "kind": "permission", "id": id });
        if !rules.is_empty() && rules.iter().all(|r| self.allowed.contains(r)) {
            let accept = answer(&reply, Decision::Allow, None, None).expect("allowing needs no answers");
            return (vec![], vec![accept]);
        }
        let meta = &p["metadata"];
        let (input, directories) = match permission.as_str() {
            "bash" => (json!({ "command": patterns.join("\n") }), vec![]),
            "edit" => (
                json!({ "file_path": meta["filepath"].as_str().map(String::from).unwrap_or_else(|| patterns.join(", ")), "diff": meta["diff"] }),
                vec![],
            ),
            // Folders outside the project; the card says so.
            "external_directory" => (json!({ "path": patterns.join(", ") }), patterns.clone()),
            // Anything else (an MCP tool) is shown with its call's arguments.
            _ => (Value::Null, vec![]),
        };
        self.asked.insert(id.clone());
        let call = p["tool"]["callID"].as_str().map(String::from);
        let approval = AgentEvent::Approval {
            request_id: id,
            tool_use_id: call.clone(),
            tool_name: self.tool_name(&permission),
            input,
            description: None,
            reason: None,
            rules,
            directories,
            suggestions: reply,
        };
        // Without a call there are no arguments to wait for: it goes now, and only it (others may
        // still be waiting for theirs).
        let Some(call) = call else { return (vec![approval], vec![]) };
        self.waiting.push(approval);
        (self.release(&call), vec![])
    }

    /// Approvals for call `call` that can be shown now: with the call's arguments filled in,
    /// once they are known (at once, for requests that describe themselves).
    fn release(&mut self, call: &str) -> Vec<AgentEvent> {
        let known = self.inputs.get(call).cloned();
        let (ready, keep) = std::mem::take(&mut self.waiting).into_iter().partition(|a| match a {
            AgentEvent::Approval { tool_use_id, input, .. } => {
                tool_use_id.as_deref() == Some(call) && (!input.is_null() || known.is_some())
            }
            _ => true,
        });
        self.waiting = keep;
        ready
            .into_iter()
            .map(|mut a| {
                if let AgentEvent::Approval { input, .. } = &mut a {
                    if input.is_null() {
                        *input = known.clone().unwrap_or_default();
                    }
                }
                a
            })
            .collect()
    }

    /// opencode's tool names, as Bach shows them: the shell as Codex's is, MCP tools as the
    /// other agents name them (`playwright_browser_navigate` ->
    /// `mcp__playwright__browser_navigate`), the rest as they are.
    fn tool_name(&self, tool: &str) -> String {
        match tool {
            "bash" => "Shell".into(),
            "edit" => "Edit".into(),
            "write" => "Write".into(),
            "read" => "Read".into(),
            other => self
                .mcp_servers
                .iter()
                // The longest name first, in case one server's name starts with another's.
                .filter_map(|server| Some((server, other.strip_prefix(&mcp_prefix(server))?)))
                .max_by_key(|(server, _)| server.len())
                .map(|(server, t)| format!("mcp__{server}__{t}"))
                .unwrap_or_else(|| other.into()),
        }
    }

    fn on_question(&mut self, p: &Value) -> Vec<AgentEvent> {
        let id = s(&p["id"]);
        let asked: Vec<&Value> = p["questions"].as_array().into_iter().flatten().collect();
        let questions: Vec<Value> = asked
            .iter()
            .map(|q| {
                json!({
                    "question": q["question"],
                    "header": q["header"],
                    "options": q["options"].as_array().cloned().unwrap_or_default(),
                    "multiSelect": q["multiple"] == true,
                })
            })
            .collect();
        // Answers go back in the order asked.
        let order: Vec<Value> = asked.iter().map(|q| q["question"].clone()).collect();
        self.asked.insert(id.clone());
        vec![AgentEvent::Approval {
            request_id: id.clone(),
            tool_use_id: p["tool"]["callID"].as_str().map(String::from),
            tool_name: QUESTION_TOOL.into(),
            input: json!({ "questions": questions }),
            description: None,
            reason: None,
            rules: vec![],
            directories: vec![],
            suggestions: json!({ "kind": "question", "id": id, "order": order }),
        }]
    }
}

/// How opencode starts the names of an MCP server's tools: `chrome-devtools_`.
fn mcp_prefix(server: &str) -> String {
    let safe: String = server
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    format!("{safe}_")
}

/// The reply to an approval request, from what was kept of it (`suggestions`). `answers` are
/// the user's answers to questions, keyed by question text (several picks joined by ", ").
///
/// opencode's "always" lasts as long as its server; Bach also keeps the rules for the session
/// and gives them to opencode each turn.
pub fn answer(
    suggestions: &Value,
    decision: Decision,
    message: Option<String>,
    answers: Option<&HashMap<String, String>>,
) -> Result<Reply, String> {
    let id = s(&suggestions["id"]);
    if suggestions["kind"] == "question" {
        if decision == Decision::Deny {
            return Ok(Reply { path: format!("/question/{id}/reject"), body: json!({}) });
        }
        let answers = answers.ok_or("Answer the question first.")?;
        let order: Vec<&str> = suggestions["order"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if let Some(unknown) = answers.keys().find(|k| !order.contains(&k.as_str())) {
            return Err(format!("`{unknown}` isn't one of the questions asked."));
        }
        let answers: Vec<Vec<&str>> = order
            .iter()
            .map(|q| {
                answers
                    .get(*q)
                    .map(|a| a.split(", ").filter(|x| !x.is_empty()).collect())
                    .unwrap_or_default()
            })
            .collect();
        return Ok(Reply { path: format!("/question/{id}/reply"), body: json!({ "answers": answers }) });
    }
    if answers.is_some() {
        return Err("That request doesn't take answers.".into());
    }
    let mut body = json!({
        "reply": match decision {
            Decision::Allow => "once",
            Decision::AllowSession | Decision::AllowAlways => "always",
            Decision::Deny => "reject",
        },
    });
    if let (Decision::Deny, Some(m)) = (decision, message) {
        body["message"] = json!(m);
    }
    Ok(Reply { path: format!("/permission/{id}/reply"), body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::Kind;

    #[test]
    fn modes_become_rules_with_session_rules_last() {
        let rules = permission_rules(Some("manual"), &["bash(git status *)".into(), "nonsense".into()]);
        assert_eq!(
            rules,
            json!([
                { "permission": "edit", "pattern": "*", "action": "ask" },
                { "permission": "bash", "pattern": "*", "action": "ask" },
                { "permission": "bach_task_list", "pattern": "*", "action": "allow" },
                { "permission": "bach_task_logs", "pattern": "*", "action": "allow" },
                { "permission": "bach_port_info", "pattern": "*", "action": "allow" },
                { "permission": "bach_http_check", "pattern": "*", "action": "allow" },
                { "permission": "bash", "pattern": "git status *", "action": "allow" },
            ])
        );
        assert_eq!(permission_rules(None, &[])[0], json!({ "permission": "bash", "pattern": "*", "action": "ask" }));
        assert!(!permission_rules(None, &[]).to_string().contains("bach_task_start"), "starting still asks");
        assert_eq!(permission_rules(Some("acceptEdits"), &[]), permission_rules(None, &[]));
        assert_eq!(permission_rules(Some("fullAccess"), &[])[0]["action"], "allow");
        assert_eq!((agent(Some("plan")), agent(None)), ("plan", "build"));
    }

    #[test]
    fn models_are_provider_slash_model() {
        assert_eq!(model("google/gemini-x"), Some(json!({ "providerID": "google", "modelID": "gemini-x" })));
        assert_eq!(model("opus"), None);
        let list = models_from(
            &json!({ "providers": [{ "id": "google", "name": "Google", "models": { "gemini-x": { "name": "Gemini X" } } }] }),
            Some("google/gemini-x"),
        );
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].id.as_str(), list[0].name.as_str(), list[0].is_default), ("google/gemini-x", "Gemini X", true));
    }

    #[test]
    fn variants_are_effort_levels_least_first() {
        let list = models_from(
            &json!({ "providers": [{ "id": "google", "name": "Google", "models": {
                "g": { "name": "G", "variants": { "high": {}, "turbo": {}, "minimal": {} } },
                "plain": { "name": "Plain" },
            } }] }),
            None,
        );
        assert_eq!(list[0].efforts, ["minimal", "high", "turbo"]);
        assert!(list[1].efforts.is_empty());
    }

    #[test]
    fn attachments_go_as_file_urls() {
        let file = |path: &str, mime: &str, kind| SavedFile { path: path.into(), mime: mime.into(), kind };
        let parts = prompt_parts(
            "look",
            &[file("/tmp/1/a.png", "image/png", Kind::Image), file("/tmp/2/b c.pdf", "application/pdf", Kind::Pdf)],
        );
        assert_eq!(parts[1], json!({ "type": "file", "mime": "image/png", "filename": "a.png", "url": "file:///tmp/1/a.png" }));
        assert_eq!(parts[2], json!({ "type": "file", "mime": "application/pdf", "filename": "b c.pdf", "url": "file:///tmp/2/b c.pdf" }));
        assert_eq!(prompt_parts("", &[]), json!([]));
    }

    #[test]
    fn mcp_approvals_wait_for_their_arguments_and_use_mcp_names() {
        // The order opencode 1.18 sends them in: the call (no arguments yet), the permission
        // request (no arguments), then the call running with its arguments.
        let servers = ["playwright".to_string(), "chrome-devtools".to_string(), "bach".to_string()];
        let mut stream = Stream::new("ses_1", &[], &servers);
        let tool = |status: &str, input: Value| {
            json!({ "type": "message.part.updated", "properties": { "part": {
                "sessionID": "ses_1", "id": "prt_1", "messageID": "msg_1", "type": "tool",
                "tool": "playwright_browser_navigate", "callID": "call_1",
                "state": { "status": status, "input": input },
            }}})
        };
        assert!(stream.on_event(&tool("pending", json!({}))).0.is_empty());
        let asked = json!({ "type": "permission.asked", "properties": {
            "id": "per_1", "sessionID": "ses_1", "permission": "playwright_browser_navigate",
            "patterns": ["*"], "metadata": {}, "always": ["*"],
            "tool": { "messageID": "msg_1", "callID": "call_1" },
        }});
        assert!(stream.on_event(&asked).0.is_empty(), "shown before its arguments are known");
        let (ev, _) = stream.on_event(&tool("running", json!({ "url": "http://127.0.0.1:5173" })));
        match &ev[0] {
            AgentEvent::Approval { tool_name, input, rules, .. } => {
                assert_eq!(tool_name, "mcp__playwright__browser_navigate");
                assert_eq!(input, &json!({ "url": "http://127.0.0.1:5173" }));
                assert_eq!(rules, &["playwright_browser_navigate(*)"]);
            }
            other => panic!("expected the approval, got {other:?}"),
        }
        assert!(matches!(&ev[1], AgentEvent::ToolUse { name, .. } if name == "mcp__playwright__browser_navigate"));

        // A permission without a call goes at once, leaving others waiting for their arguments.
        let mut stream = Stream::new("ses_1", &[], &servers);
        assert!(stream.on_event(&tool("pending", json!({}))).0.is_empty());
        assert!(stream.on_event(&asked).0.is_empty());
        let (ev, _) = stream.on_event(&json!({ "type": "permission.asked", "properties": {
            "id": "per_2", "sessionID": "ses_1", "permission": "bash",
            "patterns": ["ls"], "metadata": {}, "always": ["ls"],
        }}));
        assert!(matches!(&ev[..], [AgentEvent::Approval { request_id, .. }] if request_id == "per_2"), "{ev:?}");
        let (ev, _) = stream.on_event(&tool("running", json!({ "url": "http://127.0.0.1:5173" })));
        assert!(matches!(&ev[0], AgentEvent::Approval { request_id, input, .. }
            if request_id == "per_1" && input["url"] == "http://127.0.0.1:5173"), "{ev:?}");

        assert_eq!(stream.tool_name("chrome-devtools_new_page"), "mcp__chrome-devtools__new_page");
        assert_eq!(stream.tool_name("bach_task_start"), "mcp__bach__task_start");
        assert_eq!(stream.tool_name("webfetch"), "webfetch");
        assert_eq!(stream.tool_name("bash"), "Shell");
    }

    #[test]
    fn answers() {
        let q = json!({ "kind": "question", "id": "que_1", "order": ["Colour?", "Sizes?"] });
        let a = HashMap::from([("Colour?".to_string(), "Red".to_string()), ("Sizes?".to_string(), "S, M".to_string())]);
        assert_eq!(
            answer(&q, Decision::Allow, None, Some(&a)).unwrap(),
            Reply { path: "/question/que_1/reply".into(), body: json!({ "answers": [["Red"], ["S", "M"]] }) }
        );
        assert_eq!(answer(&q, Decision::Deny, None, None).unwrap().path, "/question/que_1/reject");
        assert!(answer(&q, Decision::Allow, None, None).is_err());

        let p = json!({ "kind": "permission", "id": "per_1" });
        let reply = |d, m: Option<&str>| answer(&p, d, m.map(String::from), None).unwrap();
        assert_eq!(reply(Decision::Allow, None).body, json!({ "reply": "once" }));
        assert_eq!(reply(Decision::AllowSession, None).body, json!({ "reply": "always" }));
        assert_eq!(reply(Decision::Deny, Some("no")).body, json!({ "reply": "reject", "message": "no" }));
        assert_eq!(reply(Decision::Allow, None).path, "/permission/per_1/reply");
        assert!(answer(&p, Decision::Allow, None, Some(&a)).is_err());
    }
}
