//! One adapter per agent CLI. Each knows how to build the headless command and how to
//! translate one line of the CLI's JSON stream into normalized [`AgentEvent`]s.

mod claude;
mod codex;
mod opencode;

use satie::Grant;
pub use bach_protocol::{AgentEvent, AgentKind};
use serde_json::Value;

/// How to drive each agent's CLI.
pub trait AgentCli: Copy {
    fn binary(self) -> &'static str;

    /// Arguments for a headless, JSON-streaming invocation.
    ///
    /// `model` is only honoured by Claude Code so far (`--model`, e.g. `opus` or a full id),
    /// as is `permission_mode` (`--permission-mode`, e.g. `acceptEdits` or `auto`).
    ///
    /// `satie` adds Bach's background-task launcher as an MCP server, with guidance and a hook
    /// steering the agent to it (Claude Code only).
    ///
    /// `allowed_tools` are permission rules (e.g. `Bash(tmux ls *)`) approved earlier in the
    /// session; only Claude Code takes them.
    ///
    /// `image_files` are paths of images to attach, for agents that take them as files (see
    /// [`AgentCli::images_as_files`]).
    fn args(
        self,
        prompt: &str,
        image_files: &[String],
        session_id: Option<&str>,
        model: Option<&str>,
        permission_mode: Option<&str>,
        allowed_tools: &[String],
        satie: Option<&Grant>,
    ) -> Vec<String>;

    /// For agents driven over stdin (Claude Code, so it can ask for approvals): the first
    /// line to send, with `images` (`data:` URLs) alongside the text. Those agents stay open
    /// for control messages until the run ends.
    fn stdin_prompt(self, prompt: &str, images: &[String]) -> Option<String>;

    /// Whether images must be written to files and passed by path (Codex, opencode) rather
    /// than sent inline with the prompt (Claude Code).
    fn images_as_files(self) -> bool;

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
        image_files: &[String],
        session_id: Option<&str>,
        model: Option<&str>,
        permission_mode: Option<&str>,
        allowed_tools: &[String],
        satie: Option<&Grant>,
    ) -> Vec<String> {
        match self {
            AgentKind::Claude => {
                claude::args(session_id, model, permission_mode, allowed_tools, satie)
            }
            AgentKind::Codex => codex::args(),
            AgentKind::Opencode => opencode::args(prompt, image_files, session_id, model),
        }
    }

    fn stdin_prompt(self, prompt: &str, images: &[String]) -> Option<String> {
        match self {
            AgentKind::Claude => Some(claude::user_message(prompt, images)),
            // Codex gets its prompt as part of a conversation (see [`Conversation`]).
            _ => None,
        }
    }

    fn images_as_files(self) -> bool {
        self != AgentKind::Claude
    }

    fn parse_line(self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![AgentEvent::Raw {
                line: line.to_string(),
            }];
        };
        match self {
            AgentKind::Claude => claude::parse(&v),
            // Codex's lines only make sense within their conversation; this reads one alone.
            AgentKind::Codex => codex::Conversation::new(&Turn::default()).on_line(&v).0,
            AgentKind::Opencode => opencode::parse(&v),
        }
    }
}

/// What one turn of an agent is asked to do, and how.
#[derive(Default)]
pub struct Turn<'a> {
    pub prompt: &'a str,
    /// `data:` URLs (Claude Code).
    pub images: &'a [String],
    /// The same images saved as files (the others).
    pub image_files: &'a [String],
    /// The agent's own session id, to continue.
    pub session_id: Option<&'a str>,
    pub cwd: Option<&'a str>,
    /// Permission rules approved earlier in the session.
    pub allowed_tools: &'a [String],
    pub model: Option<&'a str>,
    pub permission_mode: Option<&'a str>,
}

/// One run's exchange with the agent process over stdin/stdout.
pub enum Conversation {
    /// The agent writes events and reads nothing, or only answers, from Bach (Claude Code,
    /// opencode).
    Lines(AgentKind),
    Codex(Box<codex::Conversation>),
}

impl Conversation {
    /// The conversation, and the first lines to send.
    pub fn new(agent: AgentKind, turn: &Turn) -> (Self, Vec<String>) {
        match agent {
            AgentKind::Codex => {
                let c = codex::Conversation::new(turn);
                let opening = c.opening();
                (Self::Codex(Box::new(c)), opening)
            }
            _ => (
                Self::Lines(agent),
                agent.stdin_prompt(turn.prompt, turn.images).into_iter().collect(),
            ),
        }
    }

    /// Whether stdin stays open for the conversation (otherwise the agent gets none).
    pub fn uses_stdin(&self) -> bool {
        match self {
            Self::Lines(agent) => agent.stdin_prompt("", &[]).is_some(),
            Self::Codex(_) => true,
        }
    }

    /// One line from the agent: the events to show and the lines to send back.
    pub fn on_line(&mut self, line: &str) -> (Vec<AgentEvent>, Vec<String>) {
        match self {
            Self::Lines(agent) => (agent.parse_line(line), vec![]),
            Self::Codex(c) => match serde_json::from_str::<Value>(line) {
                Ok(v) => c.on_line(&v),
                Err(_) => (
                    vec![AgentEvent::Raw {
                        line: line.to_string(),
                    }],
                    vec![],
                ),
            },
        }
    }

    /// A line asking the agent to stop the turn, for agents that take one.
    pub fn interrupt(&self) -> Option<String> {
        match self {
            Self::Lines(_) => None,
            Self::Codex(c) => c.interrupt(),
        }
    }
}

/// The reply to a Codex approval request (see [`codex::answer`]).
pub fn codex_answer(
    suggestions: &Value,
    decision: bach_protocol::Decision,
    answers: Option<&std::collections::HashMap<String, String>>,
) -> Result<String, String> {
    codex::answer(suggestions, decision, answers)
}

/// The permission modes a session of `agent` can choose (besides the agent's default).
pub fn permission_modes(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        AgentKind::Claude => crate::runs::PERMISSION_MODES,
        AgentKind::Codex => codex::PERMISSION_MODES,
        AgentKind::Opencode => &[],
    }
}

/// The models `agent` can run, for the model picker.
pub async fn list_models(agent: AgentKind) -> Result<Vec<bach_protocol::ModelInfo>, String> {
    match agent {
        AgentKind::Claude => Ok(claude::models()),
        AgentKind::Codex => codex::list_models().await,
        AgentKind::Opencode => Ok(vec![]),
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

    #[test]
    fn claude_prompt_images_become_image_blocks() {
        let plain: Value =
            serde_json::from_str(&AgentKind::Claude.stdin_prompt("hi", &[]).unwrap()).unwrap();
        assert_eq!(plain["message"]["content"], "hi");

        let images = ["data:image/png;base64,AAAA".to_string()];
        let line = AgentKind::Claude.stdin_prompt("look", &images).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        let content = &v["message"]["content"];
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[0]["source"]["data"], "AAAA");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "look");

        // Images alone need no empty text block.
        let line = AgentKind::Claude.stdin_prompt("", &images).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["message"]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn codex_and_opencode_take_images_as_files() {
        let files = ["/tmp/a.png".to_string(), "/tmp/b.jpg".to_string()];
        let opencode = AgentKind::Opencode.args("hi", &files, Some("s1"), None, None, &[], None);
        assert_eq!(
            opencode,
            [
                "run",
                "--format",
                "json",
                "--session",
                "s1",
                "--file=/tmp/a.png",
                "--file=/tmp/b.jpg",
                "--",
                "hi"
            ]
        );
        assert!(AgentKind::Codex.images_as_files() && AgentKind::Opencode.images_as_files());
        assert!(!AgentKind::Claude.images_as_files());
    }

    #[test]
    fn claude_text_streams_but_sub_agents_dont() {
        // Recorded from `claude -p --include-partial-messages`.
        let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":" I'm Claude"}},"session_id":"s","parent_tool_use_id":null,"uuid":"u"}"#;
        assert!(matches!(
            &AgentKind::Claude.parse_line(delta)[..],
            [AgentEvent::Delta { id, kind: bach_protocol::DeltaKind::Text, text }] if id == "1" && text == " I'm Claude"
        ));
        let sub = delta.replace(r#""parent_tool_use_id":null"#, r#""parent_tool_use_id":"t1""#);
        assert!(AgentKind::Claude.parse_line(&sub).is_empty());
        // Thinking streams too, when its text is given at all.
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}},"parent_tool_use_id":null}"#;
        assert!(AgentKind::Claude.parse_line(thinking).is_empty());
        let thinking = thinking.replace(r#""thinking":"""#, r#""thinking":"Hmm""#);
        assert!(matches!(
            &AgentKind::Claude.parse_line(&thinking)[..],
            [AgentEvent::Delta { kind: bach_protocol::DeltaKind::Thinking, text, .. }] if text == "Hmm"
        ));
        assert!(AgentKind::Claude
            .args("hi", &[], None, None, None, &[], None)
            .contains(&"--include-partial-messages".to_string()));
    }

    #[test]
    fn claude_tool_result_images_become_data_urls() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"shot"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]}}"#;
        let events = AgentKind::Claude.parse_line(line);
        assert!(matches!(
            &events[0],
            AgentEvent::ToolResult { output, images, .. }
                if output == "shot" && images == &["data:image/png;base64,AAAA"]
        ));
        // An image-only result has no text to show.
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]}}"#;
        assert!(matches!(
            &AgentKind::Claude.parse_line(line)[0],
            AgentEvent::ToolResult { output, images, .. } if output.is_empty() && images.len() == 1
        ));
    }
}
