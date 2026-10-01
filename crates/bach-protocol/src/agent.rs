use serde::{Deserialize, Serialize};
use crate::PlanUsage;
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
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
}

/// A model an agent can run, for the model picker.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// What to pass as the session's model choice.
    pub id: String,
    pub name: String,
    pub description: String,
    /// The one the agent uses when none is chosen.
    pub is_default: bool,
    /// Its thinking effort levels, least first (none if it can't be set).
    pub efforts: Vec<String>,
    /// The level it uses when none is chosen, if known.
    pub default_effort: Option<String>,
}

/// A skill or slash command an agent offers, for the composer's `/` menu. Sending `/name` (with
/// any arguments after it) runs it.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    /// What follows the `/`.
    pub name: String,
    pub description: String,
    /// What it takes after its name (`[file]`), empty for nothing.
    pub argument_hint: String,
}

/// A permission mode an agent can run in, for the mode picker.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionModeInfo {
    /// What to pass as the session's permission mode.
    pub id: String,
    pub name: String,
    /// What it lets the agent do, as it behaves on this server (Codex's modes mean less under a
    /// wrapper).
    pub description: String,
    /// The one the agent runs in when none is chosen.
    pub is_default: bool,
}

/// What a [`AgentEvent::Delta`] adds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    /// The message being written.
    Text,
    /// The agent's reasoning.
    Thinking,
    /// The output of a running tool call (`id` is the call's).
    Output,
}

/// A command bach-server starts the agent CLIs through, taking their command line after it:
/// `sandbox` runs Claude Code as `sandbox claude …`. Fixed when the server starts
/// (`BACH_AGENT_WRAPPER`); `""` starts them directly.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentWrapper {
    pub command: String,
    /// Codex's own sandbox inside the wrapper's (`BACH_AGENT_WRAPPER_CODEX_SANDBOX`). Off, the
    /// wrapper's sandbox is the only one (many can't have Codex's inside them), and Codex's modes
    /// are kept by asking instead (see [`AgentInfo::permission_modes`]).
    pub codex_sandbox: bool,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub name: String,
    pub installed: bool,
    /// The modes a session can choose, in picker order; the only ones a run accepts.
    pub permission_modes: Vec<PermissionModeInfo>,
}

/// How the user answered an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Just this once.
    Allow,
    /// Also for the rest of this run; the UI remembers the rule for later runs.
    AllowSession,
    /// Save the agent's suggested rule to the project's settings.
    AllowAlways,
    Deny,
}

/// Agent-independent events the UI renders.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    /// The agent's own session id, usable to resume the conversation.
    Session {
        id: String,
        /// The model the agent reports using for this run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        model: Option<String>,
    },
    /// `parent` is the tool call (a sub-agent) this output belongs to, if any.
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        parent: Option<String>,
    },
    Thinking { text: String },
    /// More of something still being written: a message, the reasoning, or a tool's output
    /// (`id` is the message's, or the tool call's). Only for showing it live: the whole of it
    /// follows as `Text`, `Thinking` or `ToolResult`, and deltas aren't kept in the transcript.
    Delta { id: String, kind: DeltaKind, text: String },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        parent: Option<String>,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
        /// Images the tool returned (e.g. a browser screenshot), as `data:` URLs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(optional, as = "Option<Vec<String>>")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
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
        /// Never leaves the backend.
        #[serde(skip)]
        suggestions: Value,
    },
    /// The agent no longer needs an answer to this approval.
    ApprovalCancelled { request_id: String },
    /// Progress of a sub-agent / background task started by the tool call `id`.
    /// Only the fields that changed are set.
    #[ts(optional_fields)]
    Task {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
        /// What it is doing right now.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activity: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_uses: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<bool>,
    },
    /// How full the context is after one of the agent's own messages (tokens).
    Context { used: u64 },
    /// Each model's context window (tokens), reported at the end of a turn.
    ContextWindows { windows: BTreeMap<String, u64> },
    /// The account's usage limits. Not part of the transcript: the backend keeps the latest.
    Limits { usage: PlanUsage },
    Done { cost_usd: Option<f64>, is_error: bool },
    Error { message: String },
    /// The user stopped the run. Ends the run like `Done` does.
    Cancelled,
    /// A line we could not parse as JSON or don't understand yet.
    Raw { line: String },
}
