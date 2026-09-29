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
    AgentInfo, AgentKind, ApiError, Decision, DirListing, GitInfo, Workspace, WorktreeEntry,
};
use satie_protocol::{
    ListTasksArgs, LogChunk, RemoveTaskArgs, StartTaskArgs, StopTaskArgs, Task, TaskLogChunkArgs,
    TaskLogsArgs, TaskView,
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
    start_run(StartRunArgs) -> String;
    cancel_run(CancelRunArgs) -> ();
    respond_approval(RespondApprovalArgs) -> ();

    list_dir(ListDirArgs) -> DirListing;
    git_info(GitInfoArgs) -> GitInfo;
    prepare_workspace(PrepareWorkspaceArgs) -> Workspace;
    list_worktrees(ListWorktreesArgs) -> Vec<WorktreeEntry>;
    remove_worktree(RemoveWorktreeArgs) -> ();

    list_tasks(ListTasksArgs) -> Vec<TaskView>;
    task_logs(TaskLogsArgs) -> String;
    task_log_chunk(TaskLogChunkArgs) -> LogChunk;
    stop_task(StopTaskArgs) -> Task;
    remove_task(RemoveTaskArgs) -> ();
    start_task(StartTaskArgs) -> Task;

    list_sessions(ListSessionsArgs) -> Vec<Value>;
    save_session(SaveSessionArgs) -> ();
    delete_session(DeleteSessionArgs) -> ();
}

// ---------------------------------------------------------------------------------------------
// Agents and runs
// ---------------------------------------------------------------------------------------------

/// The agent CLIs Bach knows, and which are installed on the backend host.
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListAgentsArgs {}

/// Starts one turn of an agent and returns its run id. Its events arrive as `run` events.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct StartRunArgs {
    pub agent: AgentKind,
    pub prompt: String,
    pub cwd: Option<String>,
    /// The agent's own session id, to continue an earlier conversation.
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Permission rules approved earlier in the session.
    pub allowed_tools: Option<Vec<String>>,
    /// The UI session this run belongs to, so deleting it stops the run.
    pub session_key: Option<String>,
}

/// Stops a run. Stopping one that already ended does nothing.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CancelRunArgs {
    pub run_id: String,
}

/// Answers an approval request (or a question) a run is waiting on.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct RespondApprovalArgs {
    pub run_id: String,
    pub request_id: String,
    pub decision: Decision,
    pub message: Option<String>,
    /// For a question from the agent: the chosen answer per question text.
    pub answers: Option<HashMap<String, String>>,
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

/// Branches and state of the repository containing `path`.
#[derive(Debug, Deserialize, TS)]
pub struct GitInfoArgs {
    pub path: String,
}

/// Readies where a new session runs: switches `cwd` to `branch`, or (with `worktree`) creates
/// `newBranch` from `branch` in an isolated worktree.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct PrepareWorkspaceArgs {
    pub cwd: String,
    pub branch: Option<String>,
    pub worktree: Option<bool>,
    pub new_branch: Option<String>,
}

/// Worktrees Bach created on the backend host.
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListWorktreesArgs {}

/// Removes a worktree Bach created.
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
// Sessions
// ---------------------------------------------------------------------------------------------

/// Saved sessions, most recently saved first.
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListSessionsArgs {}

/// Saves (inserts or replaces) a session. Sessions are the frontend's JSON; only `id` is read.
#[derive(Debug, Deserialize, TS)]
pub struct SaveSessionArgs {
    pub session: Value,
}

/// Deletes a session and stops its runs (its background tasks keep going).
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub session_id: String,
}
