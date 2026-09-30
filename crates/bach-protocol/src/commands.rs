//! The command table: every command's name, arguments and result, in one place.
//!
//! [`commands!`] turns the table into the [`Request`] enum, the [`Handler`] trait a backend
//! implements (one method per command, so a missing one is a compile error), [`Request::dispatch`],
//! and the TypeScript `Commands` map the frontend's `call()` is typed with.
//!
//! Naming: commands and enum values are `snake_case`, fields are `camelCase`. Each command's
//! arguments are a struct named after it with an `Args` suffix; its doc comment documents the
//! command.
use crate::{
    AgentInfo, AgentKind, ApiError, ModelInfo, Decision, DirListing, GitDiff, GitInfo, PlanUsage, Session,
    SessionLog, TerminalInfo, TerminalSnapshot, WorktreeEntry,
};
use satie_protocol::{
    ListTasksArgs, LogChunk, RemoveTaskArgs, StartTaskArgs, StopTaskArgs, Task, TaskLogChunkArgs,
    TaskLogsArgs, TaskProcessArgs, TaskView,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use std::collections::HashMap;
use ts_rs::{Config, TypeVisitor, TS};

macro_rules! commands {
    ($( $name:ident($args:ident) -> $out:ty; )*) => {
        /// A parsed command, ready to [`dispatch`](Request::dispatch).
        #[derive(Debug)]
        pub enum Request {
            $( $args($args), )*
        }

        /// What a backend implements: one method per command.
        pub trait Handler: Sync {
            $( fn $name(&self, args: $args)
                -> impl std::future::Future<Output = Result<$out, ApiError>> + Send; )*
        }

        impl Request {
            /// Parses a command from its name and JSON arguments (`null` counts as `{}`).
            pub fn parse(cmd: &str, args: Value) -> Result<Self, ApiError> {
                let args = if args.is_null() { Value::Object(Default::default()) } else { args };
                match cmd {
                    $( c if c == stringify!($name) => Ok(Request::$args(parse_args(cmd, args)?)), )*
                    _ => Err(ApiError::invalid(format!("Unknown command `{cmd}`."))),
                }
            }

            pub fn name(&self) -> &'static str {
                match self { $( Request::$args(_) => stringify!($name), )* }
            }

            /// Runs the command on `handler` and serializes its result.
            pub async fn dispatch<H: Handler>(self, handler: &H) -> Result<Value, ApiError> {
                match self {
                    $( Request::$args(args) => Ok(serde_json::to_value(handler.$name(args).await?)
                        .expect("command results serialize")), )*
                }
            }
        }

        pub(crate) fn visit_types(v: &mut impl TypeVisitor) {
            $( v.visit::<$args>(); v.visit::<$out>(); )*
        }

        /// `export type Commands = { name: { args: ...; output: ... }; ... }`
        pub(crate) fn typescript_map(cfg: &Config) -> String {
            let mut ts = String::from("export type Commands = {\n");
            $(
                if let Some(docs) = <$args as TS>::docs() {
                    for line in docs.lines() {
                        ts += &format!("  {line}\n");
                    }
                }
                ts += &format!(
                    "  {}: {{ args: {}; output: {} }};\n",
                    stringify!($name),
                    <$args as TS>::name(cfg),
                    <$out as TS>::name(cfg),
                );
            )*
            ts + "};\n"
        }
    };
}

fn parse_args<T: DeserializeOwned>(cmd: &str, args: Value) -> Result<T, ApiError> {
    serde_json::from_value(args)
        .map_err(|e| ApiError::invalid(format!("Bad arguments for `{cmd}`: {e}")))
}

commands! {
    list_agents(ListAgentsArgs) -> Vec<AgentInfo>;
    set_agent_wrapper(SetAgentWrapperArgs) -> ();
    list_models(ListModelsArgs) -> Vec<ModelInfo>;
    get_usage(GetUsageArgs) -> Option<PlanUsage>;

    list_sessions(ListSessionsArgs) -> Vec<Session>;
    get_session(GetSessionArgs) -> SessionLog;
    start_session(StartSessionArgs) -> Session;
    send_message(SendMessageArgs) -> Session;
    send_queued(SendQueuedArgs) -> Session;
    remove_queued(RemoveQueuedArgs) -> Session;
    stop_session(StopSessionArgs) -> ();
    answer_approval(AnswerApprovalArgs) -> ();
    update_session(UpdateSessionArgs) -> Session;
    delete_session(DeleteSessionArgs) -> ();

    list_dir(ListDirArgs) -> DirListing;
    read_image(ReadImageArgs) -> String;
    git_info(GitInfoArgs) -> GitInfo;
    git_diff(GitDiffArgs) -> GitDiff;
    list_worktrees(ListWorktreesArgs) -> Vec<WorktreeEntry>;
    remove_worktree(RemoveWorktreeArgs) -> ();

    list_tasks(ListTasksArgs) -> Vec<TaskView>;
    task_logs(TaskLogsArgs) -> String;
    task_log_chunk(TaskLogChunkArgs) -> LogChunk;
    stop_task(StopTaskArgs) -> Task;
    remove_task(RemoveTaskArgs) -> ();
    start_task(StartTaskArgs) -> Task;
    task_process(TaskProcessArgs) -> ();

    list_terminals(ListTerminalsArgs) -> Vec<TerminalInfo>;
    open_terminal(OpenTerminalArgs) -> TerminalInfo;
    terminal_snapshot(TerminalSnapshotArgs) -> TerminalSnapshot;
    terminal_input(TerminalInputArgs) -> ();
    resize_terminal(ResizeTerminalArgs) -> ();
    close_terminal(CloseTerminalArgs) -> ();
}

// ---------------------------------------------------------------------------------------------
// Agents and sessions
// ---------------------------------------------------------------------------------------------

/// The agent CLIs Bach knows, and which are installed on the backend host.
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListAgentsArgs {}

/// Starts the agent CLIs through `wrapper` from now on, a command that takes the agent's command
/// line after it: `sandbox` runs Claude Code as `sandbox claude …`. `""` starts them directly.
#[derive(Debug, Deserialize, TS)]
pub struct SetAgentWrapperArgs {
    pub wrapper: String,
}

/// The models an agent can run. Codex and opencode are asked for their current lists (kept for
/// a while); Claude Code's are its aliases. opencode's depend on the project folder (`cwd`),
/// which may add providers of its own.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsArgs {
    pub agent: AgentKind,
    #[serde(default)]
    #[ts(optional)]
    pub cwd: Option<String>,
}

/// The account's usage limits as last reported by an agent run (none before the first run).
#[derive(Debug, Default, Deserialize, TS)]
pub struct GetUsageArgs {}

/// Every session, most recently active first (without transcripts).
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListSessionsArgs {}

/// A session and its transcript, or only the entries after `afterSeq`.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct GetSessionArgs {
    pub session_id: String,
    pub after_seq: Option<u64>,
}

/// Creates a session and sends its first message. Readies where it runs first: switches `cwd` to
/// `branch`, or (with `worktree`) creates a new branch from it in an isolated worktree. Nothing is
/// saved if that, or starting the agent, fails.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct StartSessionArgs {
    pub agent: AgentKind,
    pub cwd: String,
    pub branch: Option<String>,
    pub worktree: Option<bool>,
    /// The worktree's branch name; made up from the prompt when not given.
    pub new_branch: Option<String>,
    pub model_choice: Option<String>,
    pub permission_mode: Option<String>,
    /// Thinking effort (one of the model's levels); `""` for the model's default.
    pub effort: Option<String>,
    pub prompt: String,
    /// Images, PDFs and text files sent with the prompt, as `data:` URLs (see `Entry::User`).
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>")]
    pub images: Vec<String>,
}

/// Sends a message to a session's agent; while it is busy, the message is queued instead.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct SendMessageArgs {
    pub session_id: String,
    pub prompt: String,
    /// Images, PDFs and text files sent with the prompt, as `data:` URLs (see `Entry::User`).
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>")]
    pub images: Vec<String>,
}

/// Sends a queued message now. The session must not be running.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SendQueuedArgs {
    pub session_id: String,
    pub message_id: String,
}

/// Takes a message out of the queue without sending it.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemoveQueuedArgs {
    pub session_id: String,
    pub message_id: String,
}

/// Stops the session's agent run, if one is going.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StopSessionArgs {
    pub session_id: String,
}

/// Answers an approval request (or a question) the session's agent is waiting on.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AnswerApprovalArgs {
    pub session_id: String,
    pub request_id: String,
    pub decision: Decision,
    pub message: Option<String>,
    /// For a question from the agent: the chosen answer per question text.
    pub answers: Option<HashMap<String, String>>,
}

/// Renames a session, archives or restores it, or changes the model or permission mode its next
/// messages use (`""` for the default). Archiving stops its run.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct UpdateSessionArgs {
    pub session_id: String,
    pub title: Option<String>,
    pub model_choice: Option<String>,
    pub permission_mode: Option<String>,
    /// Thinking effort (one of the model's levels); `""` for the model's default.
    pub effort: Option<String>,
    pub archived: Option<bool>,
}

/// Deletes a session and stops its run (its background tasks keep going).
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub session_id: String,
}

// ---------------------------------------------------------------------------------------------
// Folders, git and worktrees
// ---------------------------------------------------------------------------------------------

/// Lists sub-directories on the backend host, for the folder picker.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ListDirArgs {
    /// Defaults to the home directory.
    pub path: Option<String>,
    pub show_hidden: Option<bool>,
}

/// An image file on the backend host as a `data:` URL, so the UI can show images an agent
/// links to by path. Only image files, up to 25 MB.
#[derive(Debug, Deserialize, TS)]
pub struct ReadImageArgs {
    /// Absolute, or starting with `~`.
    pub path: String,
}

/// Branches and state of the repository containing `path`.
#[derive(Debug, Deserialize, TS)]
pub struct GitInfoArgs {
    pub path: String,
}

/// The changes in the checkout at `path`: uncommitted ones (including untracked files), or with
/// `baseBranch`, everything since the branch left it (committed or not).
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct GitDiffArgs {
    pub path: String,
    pub base_branch: Option<String>,
}

/// Worktrees Bach created on the backend host.
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListWorktreesArgs {}

/// Removes a worktree Bach created. Sessions that ran in it can still be read, not continued.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct RemoveWorktreeArgs {
    pub path: String,
    /// Remove it even with uncommitted changes.
    pub discard: Option<bool>,
    /// Also delete its branch.
    pub delete_branch: Option<bool>,
}

// ---------------------------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------------------------

/// Terminals the backend keeps, oldest first (including ones whose shell has ended).
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListTerminalsArgs {}

/// Starts your shell in a terminal on the backend host.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct OpenTerminalArgs {
    /// Defaults to the home directory.
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

/// A terminal and its recent output.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshotArgs {
    pub terminal_id: String,
}

/// Typed (or pasted) text for the terminal.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputArgs {
    pub terminal_id: String,
    pub data: String,
}

#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ResizeTerminalArgs {
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

/// Ends the terminal's shell (and what runs in it) and forgets the terminal.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CloseTerminalArgs {
    pub terminal_id: String,
}
