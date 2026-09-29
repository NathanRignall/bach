use super::AgentEvent;
use serde_json::Value;

pub fn args(prompt: &str, session_id: Option<&str>) -> Vec<String> {
    let mut a = vec![
        "-p".into(),
        prompt.into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if let Some(id) = session_id {
        a.push("--resume".into());
        a.push(id.into());
    }
    a
}

pub fn parse(v: &Value) -> Vec<AgentEvent> {
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            vec![AgentEvent::Session { id: s(&v["session_id"]) }]
        }
        Some("assistant") | Some("user") => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| match block["type"].as_str()? {
                "text" => Some(AgentEvent::Text { text: s(&block["text"]) }),
                "thinking" => Some(AgentEvent::Thinking { text: s(&block["thinking"]) }),
                "tool_use" => Some(AgentEvent::ToolUse {
                    id: s(&block["id"]),
                    name: s(&block["name"]),
                    input: block["input"].clone(),
                }),
                "tool_result" => Some(AgentEvent::ToolResult {
                    id: s(&block["tool_use_id"]),
                    output: match &block["content"] {
                        Value::String(t) => t.clone(),
                        other => other.to_string(),
                    },
                    is_error: block["is_error"].as_bool().unwrap_or(false),
                }),
                _ => None,
            })
            .collect(),
        Some("result") => vec![AgentEvent::Done {
            cost_usd: v["total_cost_usd"].as_f64(),
            is_error: v["is_error"].as_bool().unwrap_or(false),
        }],
        _ => vec![],
    }
}
