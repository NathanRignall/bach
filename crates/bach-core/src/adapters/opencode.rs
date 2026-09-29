//! Stub: surfaces raw JSON until the `opencode run --format json` schema is mapped.
use super::AgentEvent;
use serde_json::Value;

/// `images` are files to attach. `--file` takes several values, so `--` ends the options before
/// the message (which also keeps a message starting with `-` from reading as a flag).
pub fn args(
    prompt: &str,
    images: &[String],
    session_id: Option<&str>,
    _model: Option<&str>,
) -> Vec<String> {
    let mut a = vec!["run".into(), "--format".into(), "json".into()];
    if let Some(id) = session_id {
        a.push("--session".into());
        a.push(id.into());
    }
    a.extend(images.iter().map(|f| format!("--file={f}")));
    a.push("--".into());
    a.push(prompt.into());
    a
}

pub fn parse(v: &Value) -> Vec<AgentEvent> {
    vec![AgentEvent::Raw {
        line: v.to_string(),
    }]
}
