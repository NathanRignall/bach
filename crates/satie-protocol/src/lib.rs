//! Wire types for Satie, the background-task launcher: what a task looks like, and the arguments
//! of the commands that manage tasks. Plain data (serde + ts-rs), so any frontend or transport
//! can use them without pulling in the launcher itself.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    /// Finished with exit code 0.
    Exited,
    /// Finished with a non-zero exit code.
    Failed,
    /// Stopped on request.
    Stopped,
    /// Gone without leaving an exit code (killed from outside, machine rebooted).
    Lost,
}

/// A background process as Satie records it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    /// Where it runs.
    pub cwd: String,
    /// The project (agent run folder) that started it; agents only see their own project's tasks.
    pub project: Option<String>,
    /// Who started it, in the embedding app's terms (Bach: the agent run).
    pub owner: Option<String>,
    pub pid: u32,
    /// Process start time from `/proc`, so a reused pid isn't mistaken for this task.
    pub start_ticks: Option<u64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub log_path: String,
    /// Ports the task should come up on; a missing one is a sign a part of it failed.
    #[serde(default)]
    pub expected_ports: Vec<u16>,
}

/// A task plus what is only known by looking at the machine right now.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    /// TCP ports the task's processes are listening on (recognisable ones only).
    pub ports: Vec<u16>,
    /// Expected ports that are listening right now.
    pub up_ports: Vec<u16>,
    /// Expected ports that are not listening, once the task has had a moment to start.
    pub missing_ports: Vec<u16>,
    /// What looks wrong: ports already taken by something else, expected ports that never came up.
    pub problems: Vec<String>,
    /// What is running in it, e.g. `workerd ×8`.
    pub processes: Vec<String>,
}

/// A change to the task list.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum TaskEvent {
    /// A task started, or something about it changed (status, ports, problems, processes).
    Changed { task: TaskView },
    Removed { id: String },
}

/// A piece of a log file.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogChunk {
    pub text: String,
    /// Where `text` starts in the file.
    pub offset: u64,
    /// Where to continue from next time.
    pub next: u64,
    /// The file's size when read.
    pub size: u64,
    /// Output was skipped, or the file was truncated, so this isn't a continuation of the last chunk.
    pub restarted: bool,
}

// ---------------------------------------------------------------------------------------------
// Command arguments
// ---------------------------------------------------------------------------------------------

/// Every task, oldest first.
#[derive(Debug, Default, Deserialize, TS)]
pub struct ListTasksArgs {}

/// The last lines of a task's log.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct TaskLogsArgs {
    pub task_id: String,
    /// How many lines (default 200, at most 2000).
    pub lines: Option<usize>,
}

/// A piece of a task's log: from a byte offset, or (without one) its tail.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct TaskLogChunkArgs {
    pub task_id: String,
    pub from: Option<u64>,
    /// At most this much (default 512 KiB).
    pub max_bytes: Option<u64>,
}

/// Stops a task and everything it started.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StopTaskArgs {
    pub task_id: String,
}

/// Forgets a finished task and deletes its log.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemoveTaskArgs {
    pub task_id: String,
}

/// Starts a command by hand, as a task of the project in `cwd`.
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct StartTaskArgs {
    pub command: String,
    pub cwd: String,
    pub name: Option<String>,
}
