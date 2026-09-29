//! One adapter per agent CLI. Each knows how to build the headless command and how to
//! translate one line of the CLI's JSON stream into normalized [`AgentEvent`]s.

mod claude;
mod codex;
mod opencode;

use crate::satie::SatieArgs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Opencode,
}

impl AgentKind {
    pub const ALL: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode];

    pub fn display_name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Opencode => "opencode",
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Opencode => "opencode",
        }
    }

    /// Arguments for a headless, JSON-streaming invocation.
    ///
    /// `model` is only honoured by Claude Code so far (`--model`, e.g. `opus` or a full id).
    ///
    /// `satie` adds Bach's background-task launcher as an MCP server, with guidance and a hook
    /// steering the agent to it (Claude Code only).
    ///
    /// `allowed_tools` are permission rules (e.g. `Bash(tmux ls *)`) approved earlier in the
    /// session; only Claude Code takes them.
    pub fn args(
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

    /// For agents driven over stdin (Claude Code, so it can ask for approvals): the first
    /// line to send. Those agents stay open for control messages until the run ends.
    pub fn stdin_prompt(self, prompt: &str) -> Option<String> {
        match self {
            AgentKind::Claude => Some(claude::user_message(prompt)),
            _ => None,
        }
    }

    pub fn parse_line(self, line: &str) -> Vec<AgentEvent> {
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

/// Agent-independent events the UI renders.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent's own session id, usable to resume the conversation.
    Session {
        id: String,
        /// The model the agent reports using for this run.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// `parent` is the tool call (a sub-agent) this output belongs to, if any.
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// The agent wants to use a tool that needs the user's approval and waits for an answer.
    Approval {
        request_id: String,
        tool_use_id: Option<String>,
        tool_name: String,
        input: Value,
        description: Option<String>,
        reason: Option<String>,
        /// Permission rules an "allow for this session / always" answer would add.
        rules: Vec<String>,
        /// Folders outside the project this would also reach into.
        directories: Vec<String>,
        /// The agent's own suggested permission updates; echoed back when the user opts in.
        #[serde(skip)]
        suggestions: Value,
    },
    /// The agent no longer needs an answer to this approval.
    ApprovalCancelled {
        request_id: String,
    },
    /// Progress of a sub-agent / background task started by the tool call `id`.
    /// Only the fields that changed are set.
    Task {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
        /// What it is doing right now.
        #[serde(skip_serializing_if = "Option::is_none")]
        activity: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_uses: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        background: Option<bool>,
    },
    Done {
        cost_usd: Option<f64>,
        is_error: bool,
    },
    Error {
        message: String,
    },
    /// The user stopped the run. Ends the run like `Done` does.
    Cancelled,
    /// A line we could not parse as JSON or don't understand yet.
    Raw {
        line: String,
    },
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
