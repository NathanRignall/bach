//! Stub: surfaces raw JSON until the `opencode run --format json` schema is mapped.
use super::AgentEvent;
use serde_json::Value;

pub fn args(prompt: &str, session_id: Option<&str>, _model: Option<&str>) -> Vec<String> {
    let mut a = vec!["run".into(), "--format".into(), "json".into()];
    if let Some(id) = session_id {
        a.push("--session".into());
        a.push(id.into());
    }
    a.push(prompt.into());
    a
}

pub fn parse(v: &Value) -> Vec<AgentEvent> {
    vec![AgentEvent::Raw {
        line: v.to_string(),
    }]
}
