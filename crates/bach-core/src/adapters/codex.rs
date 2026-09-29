//! Best-effort: based on `codex exec --json` item events; verify against a real install.
use super::AgentEvent;
use serde_json::Value;

pub fn args(prompt: &str, session_id: Option<&str>, _model: Option<&str>) -> Vec<String> {
    match session_id {
        Some(id) => vec![
            "exec".into(),
            "resume".into(),
            id.into(),
            "--json".into(),
            prompt.into(),
        ],
        None => vec!["exec".into(), "--json".into(), prompt.into()],
    }
}

pub fn parse(v: &Value) -> Vec<AgentEvent> {
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    match v["type"].as_str() {
        Some("thread.started") => vec![AgentEvent::Session {
            id: s(&v["thread_id"]),
            model: None,
        }],
        Some("item.completed") => {
            let item = &v["item"];
            match item["type"].as_str() {
                Some("agent_message") => vec![AgentEvent::Text {
                    text: s(&item["text"]),
                    parent: None,
                }],
                Some("reasoning") => vec![AgentEvent::Thinking {
                    text: s(&item["text"]),
                }],
                Some("command_execution") => {
                    let id = s(&item["id"]);
                    vec![
                        AgentEvent::ToolUse {
                            id: id.clone(),
                            name: "shell".into(),
                            input: serde_json::json!({ "command": item["command"] }),
                            parent: None,
                        },
                        AgentEvent::ToolResult {
                            id,
                            output: s(&item["aggregated_output"]),
                            is_error: item["exit_code"].as_i64().is_some_and(|c| c != 0),
                            images: vec![],
                            parent: None,
                        },
                    ]
                }
                _ => vec![],
            }
        }
        Some("turn.completed") => vec![AgentEvent::Done {
            cost_usd: None,
            is_error: false,
        }],
        Some("turn.failed") | Some("error") => vec![AgentEvent::Error {
            message: v["error"]["message"]
                .as_str()
                .or(v["message"].as_str())
                .unwrap_or("error")
                .into(),
        }],
        _ => vec![],
    }
}
