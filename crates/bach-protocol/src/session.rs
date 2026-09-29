//! Sessions: a conversation with one agent in one project folder. The backend keeps each one as
//! [`Session`] (what it is and where it stands) plus an append-only transcript of [`LogEntry`]s,
//! which clients fold into whatever they render.
use crate::{AgentEvent, AgentKind, ContextUsage, Decision};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    /// The user named it, so the first prompt shouldn't.
    #[serde(default)]
    pub title_edited: bool,
    pub agent: AgentKind,
    /// The project folder; sessions are grouped by it.
    pub cwd: String,
    /// The git branch it was started on (or, with `worktree`, branched from).
    #[serde(default)]
    pub branch: Option<String>,
    /// Runs in an isolated git worktree on its own branch.
    #[serde(default)]
    pub worktree: bool,
    /// Model choice for Claude Code: an alias like `opus`, or none for the default.
    #[serde(default)]
    pub model_choice: Option<String>,
    /// Permission mode for Claude Code (`acceptEdits`, `auto`, `plan`, …), or none for its
    /// default: asking before anything that isn't read-only.
    #[serde(default)]
    pub permission_mode: Option<String>,

    /// Where the agent runs (a worktree, or `cwd`).
    #[serde(default)]
    pub workdir: Option<String>,
    /// The branch it runs on.
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Its worktree was removed: it can be read but not continued.
    #[serde(default)]
    pub workdir_removed: bool,
    /// The agent's own session id, to resume the conversation.
    #[serde(default)]
    pub agent_session_id: Option<String>,
    /// Permission rules approved "for this session"; every later run gets them.
    #[serde(default)]
    pub allow_rules: Vec<String>,
    /// The model the agent reported using on its latest run.
    #[serde(default)]
    pub model: Option<String>,
    /// How full the conversation's context is, as of the agent's latest message.
    #[serde(default)]
    pub context: Option<ContextUsage>,

    /// Put away: kept with its transcript, but out of the sidebar's main list.
    #[serde(default)]
    pub archived: bool,

    /// The agent run in progress, if any.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Approval requests the running agent is waiting on.
    #[serde(default)]
    pub open_approvals: Vec<String>,

    pub created_at: i64,
    /// Last activity; sessions are listed most recent first.
    pub updated_at: i64,
    /// `seq` of the latest transcript entry (0 when there are none).
    #[serde(default)]
    pub last_seq: u64,
}

/// One step of a session's transcript.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Entry {
    /// What the user sent.
    User { text: String },
    /// Something the agent did during run `runId`.
    Agent { run_id: String, event: AgentEvent },
    /// The user answered an approval request (or a question).
    Decision {
        request_id: String,
        decision: Decision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        answers: Option<HashMap<String, String>>,
    },
    /// A message couldn't be handed to the agent. `retryText` is the message, to send again.
    Failed {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        retry_text: Option<String>,
    },
    /// A transcript saved by an older Bach, as the blocks it rendered.
    Imported { blocks: Value },
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// 1, 2, 3, ... within its session.
    pub seq: u64,
    /// When it was recorded (ms since the epoch).
    pub at: i64,
    pub entry: Entry,
}

/// A session with (part of) its transcript.
#[derive(Clone, Debug, Serialize, TS)]
pub struct SessionLog {
    pub session: Session,
    pub entries: Vec<LogEntry>,
}

/// A change to sessions, pushed to every client.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum SessionEvent {
    /// A session was created, or its details changed.
    Changed { session: Session },
    /// A new transcript entry.
    Entry { session_id: String, entry: LogEntry },
    Deleted { session_id: String },
}
