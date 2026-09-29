//! One adapter per agent CLI. Each knows how to build the headless command and how to
//! translate one line of the CLI's JSON stream into normalized [`AgentEvent`]s.

mod claude;
mod codex;
mod opencode;

use crate::satie::SatieArgs;
pub use bach_protocol::{AgentEvent, AgentKind};
use serde_json::Value;

/// How to drive each agent's CLI.
pub trait AgentCli: Copy {
    fn binary(self) -> &'static str;

    /// Arguments for a headless, JSON-streaming invocation.
    ///
    /// `model` is only honoured by Claude Code so far (`--model`, e.g. `opus` or a full id).
    ///
    /// `satie` adds Bach's background-task launcher as an MCP server, with guidance and a hook
    /// steering the agent to it (Claude Code only).
    ///
    /// `allowed_tools` are permission rules (e.g. `Bash(tmux ls *)`) approved earlier in the
    /// session; only Claude Code takes them.
    fn args(
        self,
        prompt: &str,
        session_id: Option<&str>,
        model: Option<&str>,
        allowed_tools: &[String],
        satie: Option<&SatieArgs>,
    ) -> Vec<String>;

    /// For agents driven over stdin (Claude Code, so it can ask for approvals): the first
    /// line to send. Those agents stay open for control messages until the run ends.
    fn stdin_prompt(self, prompt: &str) -> Option<String>;

    fn parse_line(self, line: &str) -> Vec<AgentEvent>;
}

impl AgentCli for AgentKind {
    fn binary(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Opencode => "opencode",
        }
    }

    fn args(
        self,
        prompt: &str,
        session_id: Option<&str>,
        model: Option<&str>,
        allowed_tools: &[String],
        satie: Option<&SatieArgs>,
    ) -> Vec<String> {
        match self {
            AgentKind::Claude => claude::args(session_id, model, allowed_tools, satie),
            AgentKind::Codex => codex::args(prompt, session_id, model),
            AgentKind::Opencode => opencode::args(prompt, session_id, model),
        }
    }

    fn stdin_prompt(self, prompt: &str) -> Option<String> {
        match self {
            AgentKind::Claude => Some(claude::user_message(prompt)),
            _ => None,
        }
    }

    fn parse_line(self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![AgentEvent::Raw {
                line: line.to_string(),
            }];
        };
        match self {
            AgentKind::Claude => claude::parse(&v),
            AgentKind::Codex => codex::parse(&v),
            AgentKind::Opencode => opencode::parse(&v),
        }
    }
}

/// The agent CLIs Bach knows, and whether each is on the backend host's PATH.
pub fn list_agents() -> Vec<bach_protocol::AgentInfo> {
    AgentKind::ALL
        .iter()
        .map(|&kind| bach_protocol::AgentInfo {
            kind,
            name: kind.display_name().into(),
            installed: which::which(kind.binary()).is_ok(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream_maps_to_events() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"abc"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]}}"#,
            r#"{"type":"result","is_error":false,"total_cost_usd":0.01}"#,
        ];
        let events: Vec<_> = lines
            .iter()
            .flat_map(|l| AgentKind::Claude.parse_line(l))
            .collect();
        assert!(matches!(&events[0], AgentEvent::Session { id, .. } if id == "abc"));
        assert!(matches!(&events[1], AgentEvent::Text { text, .. } if text == "hi"));
        assert!(matches!(&events[2], AgentEvent::ToolUse { name, .. } if name == "Bash"));
        assert!(matches!(&events[3], AgentEvent::ToolResult { output, .. } if output == "ok"));
        assert!(matches!(&events[4], AgentEvent::Done { .. }));
    }
}
