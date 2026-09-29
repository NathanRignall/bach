use super::AgentEvent;
use crate::satie::SatieArgs;
use serde_json::Value;

/// The prompt goes over stdin (see [`user_message`]) so the process can also receive answers
/// to its permission requests (`--permission-prompt-tool stdio`).
pub fn args(
    session_id: Option<&str>,
    model: Option<&str>,
    allowed_tools: &[String],
    satie: Option<&SatieArgs>,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(String::from)
    .into();
    if let Some(id) = session_id {
        a.push("--resume".into());
        a.push(id.into());
    }
    if let Some(m) = model {
        a.push("--model".into());
        a.push(m.into());
    }
    if let Some(satie) = satie {
        a.extend(["--mcp-config".into(), satie.mcp_config.clone()]);
        a.extend(["--append-system-prompt".into(), satie.system_prompt.clone()]);
        a.extend(["--settings".into(), satie.settings.clone()]);
    }
    // Reading a task's state or output changes nothing, so those never need a card; starting
    // and stopping still do.
    let read_only = satie
        .iter()
        .flat_map(|_| ["mcp__satie__task_list", "mcp__satie__task_logs"]);
    let allowed: Vec<String> = allowed_tools
        .iter()
        .cloned()
        .chain(read_only.map(String::from))
        .collect();
    if !allowed.is_empty() {
        a.push("--allowedTools".into());
        a.extend(allowed);
    }
    a
}

/// One line of stream-json input: a user turn.
pub fn user_message(prompt: &str) -> String {
    serde_json::json!({ "type": "user", "message": { "role": "user", "content": prompt } })
        .to_string()
}

/// Folders the agent offers to add access to (a command reaching outside the project).
fn directories_from(suggestions: &Value) -> Vec<String> {
    suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["type"] == "addDirectories")
        .flat_map(|s| s["directories"].as_array().into_iter().flatten())
        .filter_map(|d| d.as_str().map(str::to_string))
        .collect()
}

/// `Bash(tmux ls *)`-style rule strings from the agent's suggested permission updates.
fn rules_from(suggestions: &Value) -> Vec<String> {
    suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["type"] == "addRules" && s["behavior"] == "allow")
        .flat_map(|s| s["rules"].as_array().into_iter().flatten())
        .filter_map(|r| {
            let tool = r["toolName"].as_str()?;
            Some(match r["ruleContent"].as_str() {
                Some(c) => format!("{tool}({c})"),
                None => tool.to_string(),
            })
        })
        .collect()
}

/// Tool results are either a string or a list of content blocks; keep the readable text.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            if texts.is_empty() {
                content.to_string()
            } else {
                texts.join("\n")
            }
        }
        other => other.to_string(),
    }
}

pub fn parse(v: &Value) -> Vec<AgentEvent> {
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    let opt = |x: &Value| x.as_str().map(str::to_string);
    let num = |x: &Value| x.as_u64();
    match v["type"].as_str() {
        Some("system") => match v["subtype"].as_str() {
            Some("init") => vec![AgentEvent::Session {
                id: s(&v["session_id"]),
                model: opt(&v["model"]),
            }],
            // Sub-agent / background task lifecycle, keyed by the tool call that spawned it.
            Some(kind @ ("task_started" | "task_progress" | "task_notification")) => {
                let Some(id) = opt(&v["tool_use_id"]) else {
                    return vec![];
                };
                let usage = &v["usage"];
                vec![AgentEvent::Task {
                    id,
                    status: match kind {
                        "task_started" => Some("running".into()),
                        "task_notification" => opt(&v["status"]),
                        _ => None,
                    },
                    title: (kind == "task_started")
                        .then(|| opt(&v["description"]))
                        .flatten(),
                    agent_type: opt(&v["subagent_type"]),
                    activity: (kind == "task_progress")
                        .then(|| opt(&v["description"]))
                        .flatten(),
                    tool_uses: num(&usage["tool_uses"]),
                    tokens: num(&usage["total_tokens"]),
                    duration_ms: num(&usage["duration_ms"]),
                    summary: opt(&v["summary"]),
                    background: v["is_backgrounded"].as_bool(),
                }]
            }
            _ => vec![],
        },
        Some(kind @ ("assistant" | "user")) => {
            // Set for everything a sub-agent does; the id is the tool call that spawned it.
            let parent = opt(&v["parent_tool_use_id"]);
            v["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| match block["type"].as_str()? {
                    // "user" text is just the prompt handed to a sub-agent.
                    "text" if kind == "assistant" => Some(AgentEvent::Text {
                        text: s(&block["text"]),
                        parent: parent.clone(),
                    }),
                    // Redacted thinking arrives empty; a sub-agent's thinking is noise.
                    "thinking" if parent.is_none() => Some(s(&block["thinking"]))
                        .filter(|t| !t.is_empty())
                        .map(|text| AgentEvent::Thinking { text }),
                    "tool_use" => Some(AgentEvent::ToolUse {
                        id: s(&block["id"]),
                        name: s(&block["name"]),
                        input: block["input"].clone(),
                        parent: parent.clone(),
                    }),
                    "tool_result" => Some(AgentEvent::ToolResult {
                        id: s(&block["tool_use_id"]),
                        output: content_text(&block["content"]),
                        is_error: block["is_error"].as_bool().unwrap_or(false),
                        parent: parent.clone(),
                    }),
                    _ => None,
                })
                .collect()
        }
        Some("control_request") if v["request"]["subtype"] == "can_use_tool" => {
            let r = &v["request"];
            vec![AgentEvent::Approval {
                request_id: s(&v["request_id"]),
                tool_use_id: opt(&r["tool_use_id"]),
                tool_name: s(&r["tool_name"]),
                input: r["input"].clone(),
                description: opt(&r["description"]),
                reason: opt(&r["decision_reason"]),
                rules: rules_from(&r["permission_suggestions"]),
                directories: directories_from(&r["permission_suggestions"]),
                suggestions: r["permission_suggestions"].clone(),
            }]
        }
        Some("control_cancel_request") => vec![AgentEvent::ApprovalCancelled {
            request_id: s(&v["request_id"]),
        }],
        Some("result") => vec![AgentEvent::Done {
            cost_usd: v["total_cost_usd"].as_f64(),
            is_error: v["is_error"].as_bool().unwrap_or(false),
        }],
        _ => vec![],
    }
}
