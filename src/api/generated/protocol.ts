// Generated from crates/bach-protocol by `cargo test -p bach-protocol`. Don't edit.

export type AgentInfo = { kind: AgentKind, name: string, installed: boolean, };

export type AgentKind = "claude" | "codex" | "opencode";

/**
 * Every command fails with one of these.
 */
export type ApiError = { code: ErrorCode, 
/**
 * Written for the user.
 */
message: string, };

/**
 * Stops a run. Stopping one that already ended does nothing.
 */
export type CancelRunArgs = { runId: string, };

/**
 * A command sent over the WebSocket bridge. `id` is echoed back in the reply.
 */
export type ClientFrame = { id: number, cmd: string, args?: JsonValue, };

/**
 * How the user answered an approval request.
 */
export type Decision = "allow" | "allow_session" | "allow_always" | "deny";

/**
 * Deletes a session and stops its runs (its background tasks keep going).
 */
export type DeleteSessionArgs = { sessionId: string, };

export type DirEntry = { name: string, path: string, 
/**
 * Contains a `.git`, i.e. probably a project root.
 */
git: boolean, };

export type DirListing = { path: string, parent: string | null, home: string, entries: Array<DirEntry>, };

/**
 * What kind of failure an [`ApiError`] is, for code that reacts to it. People read `message`.
 */
export type ErrorCode = "invalid" | "not_found" | "unavailable" | "failed";

export type GitInfo = { isRepo: boolean, root: string | null, 
/**
 * None when HEAD is detached.
 */
current: string | null, 
/**
 * Local branches, most recently committed first.
 */
branches: Array<string>, 
/**
 * Tracked files have uncommitted changes (untracked files are ignored).
 */
dirty: boolean, };

/**
 * Branches and state of the repository containing `path`.
 */
export type GitInfoArgs = { path: string, };

export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;

/**
 * The agent CLIs Bach knows, and which are installed on the backend host.
 */
export type ListAgentsArgs = Record<symbol, never>;

/**
 * Lists sub-directories on the backend host, for the folder picker.
 */
export type ListDirArgs = { 
/**
 * Defaults to the home directory.
 */
path?: string, showHidden?: boolean, };

/**
 * Saved sessions, most recently saved first.
 */
export type ListSessionsArgs = Record<symbol, never>;

/**
 * Every task, oldest first.
 */
export type ListTasksArgs = Record<symbol, never>;

/**
 * Worktrees Bach created on the backend host.
 */
export type ListWorktreesArgs = Record<symbol, never>;

/**
 * A piece of a log file.
 */
export type LogChunk = { text: string, 
/**
 * Where `text` starts in the file.
 */
offset: number, 
/**
 * Where to continue from next time.
 */
next: number, 
/**
 * The file's size when read.
 */
size: number, 
/**
 * Output was skipped, or the file was truncated, so this isn't a continuation of the last chunk.
 */
restarted: boolean, };

/**
 * Readies where a new session runs: switches `cwd` to `branch`, or (with `worktree`) creates
 * `newBranch` from `branch` in an isolated worktree.
 */
export type PrepareWorkspaceArgs = { cwd: string, branch?: string, worktree?: boolean, newBranch?: string, };

/**
 * Forgets a finished task and deletes its log.
 */
export type RemoveTaskArgs = { taskId: string, };

/**
 * Removes a worktree Bach created.
 */
export type RemoveWorktreeArgs = { path: string, 
/**
 * Remove it even with uncommitted changes.
 */
discard?: boolean, 
/**
 * Also delete its branch.
 */
deleteBranch?: boolean, };

/**
 * Answers an approval request (or a question) a run is waiting on.
 */
export type RespondApprovalArgs = { runId: string, requestId: string, decision: Decision, message?: string, 
/**
 * For a question from the agent: the chosen answer per question text.
 */
answers?: { [key in string]: string }, };

/**
 * One event from an agent run.
 */
export type RunEvent = { runId: string, } & ({ "type": "session", id: string, 
/**
 * The model the agent reports using for this run.
 */
model?: string, } | { "type": "text", text: string, parent?: string, } | { "type": "thinking", text: string, } | { "type": "tool_use", id: string, name: string, input: JsonValue, parent?: string, } | { "type": "tool_result", id: string, output: string, isError: boolean, parent?: string, } | { "type": "approval", requestId: string, toolUseId: string | null, toolName: string, input: JsonValue, description: string | null, reason: string | null, 
/**
 * Permission rules an "allow for this session / always" answer would add.
 */
rules: Array<string>, 
/**
 * Folders outside the project this would also reach into.
 */
directories: Array<string>, } | { "type": "approval_cancelled", requestId: string, } | { "type": "task", id: string, status?: string, title?: string, agentType?: string, 
/**
 * What it is doing right now.
 */
activity?: string, toolUses?: number, tokens?: number, durationMs?: number, summary?: string, background?: boolean, } | { "type": "done", costUsd: number | null, isError: boolean, } | { "type": "error", message: string, } | { "type": "cancelled" } | { "type": "raw", line: string, });

/**
 * Saves (inserts or replaces) a session. Sessions are the frontend's JSON; only `id` is read.
 */
export type SaveSessionArgs = { session: JsonValue, };

/**
 * Everything the backend pushes to clients, whichever transport carries it.
 */
export type ServerEvent = { "topic": "run", "data": RunEvent } | { "topic": "task", "data": TaskEvent };

/**
 * What the WebSocket bridge sends: replies to commands, and events.
 */
export type ServerFrame = { "kind": "reply", id: number, result: JsonValue, } | { "kind": "error", id: number, error: ApiError, } | { "kind": "event", event: ServerEvent, };

/**
 * Starts one turn of an agent and returns its run id. Its events arrive as `run` events.
 */
export type StartRunArgs = { agent: AgentKind, prompt: string, cwd?: string, 
/**
 * The agent's own session id, to continue an earlier conversation.
 */
sessionId?: string, model?: string, 
/**
 * Permission rules approved earlier in the session.
 */
allowedTools?: Array<string>, 
/**
 * The UI session this run belongs to, so deleting it stops the run.
 */
sessionKey?: string, };

/**
 * Starts a command by hand, as a task of the project in `cwd`.
 */
export type StartTaskArgs = { command: string, cwd: string, name?: string, };

/**
 * Stops a task and everything it started.
 */
export type StopTaskArgs = { taskId: string, };

/**
 * A background process as Satie records it.
 */
export type Task = { id: string, name: string, command: string, 
/**
 * Where it runs.
 */
cwd: string, 
/**
 * The project (agent run folder) that started it; agents only see their own project's tasks.
 */
project: string | null, 
/**
 * Who started it, in the embedding app's terms (Bach: the agent run).
 */
owner: string | null, pid: number, 
/**
 * Process start time from `/proc`, so a reused pid isn't mistaken for this task.
 */
startTicks: number | null, startedAt: number, endedAt: number | null, status: TaskStatus, exitCode: number | null, logPath: string, 
/**
 * Ports the task should come up on; a missing one is a sign a part of it failed.
 */
expectedPorts: Array<number>, };

/**
 * A change to the task list.
 */
export type TaskEvent = { "type": "changed", task: TaskView, } | { "type": "removed", id: string, };

/**
 * A piece of a task's log: from a byte offset, or (without one) its tail.
 */
export type TaskLogChunkArgs = { taskId: string, from?: number, 
/**
 * At most this much (default 512 KiB).
 */
maxBytes?: number, };

/**
 * The last lines of a task's log.
 */
export type TaskLogsArgs = { taskId: string, 
/**
 * How many lines (default 200, at most 2000).
 */
lines?: number, };

export type TaskStatus = "running" | "exited" | "failed" | "stopped" | "lost";

/**
 * A task plus what is only known by looking at the machine right now.
 */
export type TaskView = { 
/**
 * TCP ports the task's processes are listening on (recognisable ones only).
 */
ports: Array<number>, 
/**
 * Expected ports that are listening right now.
 */
upPorts: Array<number>, 
/**
 * Expected ports that are not listening, once the task has had a moment to start.
 */
missingPorts: Array<number>, 
/**
 * What looks wrong: ports already taken by something else, expected ports that never came up.
 */
problems: Array<string>, 
/**
 * What is running in it, e.g. `workerd ×8`.
 */
processes: Array<string>, id: string, name: string, command: string, 
/**
 * Where it runs.
 */
cwd: string, 
/**
 * The project (agent run folder) that started it; agents only see their own project's tasks.
 */
project: string | null, 
/**
 * Who started it, in the embedding app's terms (Bach: the agent run).
 */
owner: string | null, pid: number, 
/**
 * Process start time from `/proc`, so a reused pid isn't mistaken for this task.
 */
startTicks: number | null, startedAt: number, endedAt: number | null, status: TaskStatus, exitCode: number | null, logPath: string, 
/**
 * Ports the task should come up on; a missing one is a sign a part of it failed.
 */
expectedPorts: Array<number>, };

/**
 * Where a session's agent runs.
 */
export type Workspace = { 
/**
 * The directory the agent should run in.
 */
workdir: string, branch: string | null, worktree: boolean, };

/**
 * A worktree Bach created.
 */
export type WorktreeEntry = { path: string, 
/**
 * Folder name of the repository it belongs to.
 */
repo: string, branch: string | null, 
/**
 * Uncommitted changes, including untracked files: removing the worktree would lose them.
 */
dirty: boolean, 
/**
 * Commits on the branch that exist on no other local or remote branch.
 */
unmerged: number, };

export type Commands = {
  /**
   * The agent CLIs Bach knows, and which are installed on the backend host.
   */
  list_agents: { args: ListAgentsArgs; output: Array<AgentInfo> };
  /**
   * Starts one turn of an agent and returns its run id. Its events arrive as `run` events.
   */
  start_run: { args: StartRunArgs; output: string };
  /**
   * Stops a run. Stopping one that already ended does nothing.
   */
  cancel_run: { args: CancelRunArgs; output: null };
  /**
   * Answers an approval request (or a question) a run is waiting on.
   */
  respond_approval: { args: RespondApprovalArgs; output: null };
  /**
   * Lists sub-directories on the backend host, for the folder picker.
   */
  list_dir: { args: ListDirArgs; output: DirListing };
  /**
   * Branches and state of the repository containing `path`.
   */
  git_info: { args: GitInfoArgs; output: GitInfo };
  /**
   * Readies where a new session runs: switches `cwd` to `branch`, or (with `worktree`) creates
   * `newBranch` from `branch` in an isolated worktree.
   */
  prepare_workspace: { args: PrepareWorkspaceArgs; output: Workspace };
  /**
   * Worktrees Bach created on the backend host.
   */
  list_worktrees: { args: ListWorktreesArgs; output: Array<WorktreeEntry> };
  /**
   * Removes a worktree Bach created.
   */
  remove_worktree: { args: RemoveWorktreeArgs; output: null };
  /**
   * Every task, oldest first.
   */
  list_tasks: { args: ListTasksArgs; output: Array<TaskView> };
  /**
   * The last lines of a task's log.
   */
  task_logs: { args: TaskLogsArgs; output: string };
  /**
   * A piece of a task's log: from a byte offset, or (without one) its tail.
   */
  task_log_chunk: { args: TaskLogChunkArgs; output: LogChunk };
  /**
   * Stops a task and everything it started.
   */
  stop_task: { args: StopTaskArgs; output: Task };
  /**
   * Forgets a finished task and deletes its log.
   */
  remove_task: { args: RemoveTaskArgs; output: null };
  /**
   * Starts a command by hand, as a task of the project in `cwd`.
   */
  start_task: { args: StartTaskArgs; output: Task };
  /**
   * Saved sessions, most recently saved first.
   */
  list_sessions: { args: ListSessionsArgs; output: Array<JsonValue> };
  /**
   * Saves (inserts or replaces) a session. Sessions are the frontend's JSON; only `id` is read.
   */
  save_session: { args: SaveSessionArgs; output: null };
  /**
   * Deletes a session and stops its runs (its background tasks keep going).
   */
  delete_session: { args: DeleteSessionArgs; output: null };
};
