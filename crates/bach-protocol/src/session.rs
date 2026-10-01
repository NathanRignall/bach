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
    /// How hard the agent thinks: one of its model's effort levels (`low`, `high`, …), or none
    /// for the model's default.
    #[serde(default)]
    pub effort: Option<String>,

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
    /// How the latest run ended, until someone has looked at the session: what "done" and
    /// "failed" mean in a session list. Set when a run ends (not when the user stopped it),
    /// cleared by `mark_seen` or by the next run.
    #[serde(default)]
    pub unseen: Option<RunOutcome>,
    /// Messages sent while the agent was busy, oldest first. The next one goes when a run
    /// finishes cleanly; a stopped or failed run leaves them waiting.
    #[serde(default)]
    pub queued: Vec<QueuedMessage>,

    pub created_at: i64,
    /// Last activity; sessions are listed most recent first.
    pub updated_at: i64,
    /// `seq` of the latest transcript entry (0 when there are none).
    #[serde(default)]
    pub last_seq: u64,
}

/// How an agent run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Done,
    Failed,
}

/// A message waiting for the agent to finish its current run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    /// Images, PDFs and text files sent with it, as `data:` URLs (the name is from when only
    /// images could be). A file's name is a parameter: `data:application/pdf;name=a.pdf;base64,…`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub images: Vec<String>,
}

/// One step of a session's transcript.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Entry {
    /// What the user sent.
    User {
        text: String,
        /// Images, PDFs and text files sent with it, as `data:` URLs (the name is from when only
        /// images could be). A file's name is a parameter: `data:application/pdf;name=a.pdf;base64,…`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(optional, as = "Option<Vec<String>>")]
        images: Vec<String>,
    },
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

/// What a search found in one session.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub session_id: String,
    /// The session's title matches the query.
    pub title_match: bool,
    /// The best matches in its transcript, best first (a few, not all).
    pub hits: Vec<SearchHit>,
    /// How many transcript entries match in all.
    pub hit_count: u32,
}

/// A transcript entry that matches a search.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// The matching entry's `seq`.
    pub seq: u64,
    pub kind: SearchKind,
    /// A short stretch of the entry around the match.
    pub snippet: Vec<SnippetPart>,
}

/// What a search hit is part of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// A message from the user.
    User,
    /// A reply from the agent.
    Agent,
    /// The input of a tool call the agent made.
    Tool,
}

/// A piece of a snippet: `matched` ones are what the query found.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct SnippetPart {
    pub text: String,
    pub matched: bool,
}

/// A change to sessions, pushed to every client.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum SessionEvent {
    /// A session was created, or its details changed.
    Changed { session: Session },
    /// A new transcript entry.
    Entry { session_id: String, entry: LogEntry },
    /// More of something the agent is writing (see [`AgentEvent::Delta`]). Not part of the
    /// transcript: the finished message, reasoning or tool result arrives as an entry.
    Delta {
        session_id: String,
        run_id: String,
        id: String,
        kind: crate::DeltaKind,
        text: String,
    },
    Deleted { session_id: String },
}
