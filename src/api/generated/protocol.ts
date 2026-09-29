// Generated from crates/bach-protocol by `cargo test -p bach-protocol`. Don't edit.

/** Must match the server's `hello`; see `fingerprint` in bach-protocol. */
export const PROTOCOL = "77823c4af29bd037";

/**
 * Agent-independent events the UI renders.
 */
export type AgentEvent = { "type": "session", id: string, 
/**
 * The model the agent reports using for this run.
 */
model?: string, } | { "type": "text", text: string, parent?: string, } | { "type": "thinking", text: string, } | { "type": "tool_use", id: string, name: string, input: JsonValue, parent?: string, } | { "type": "tool_result", id: string, output: string, isError: boolean, 
/**
 * Images the tool returned (e.g. a browser screenshot), as `data:` URLs.
 */
images?: Array<string>, parent?: string, } | { "type": "approval", requestId: string, toolUseId: string | null, toolName: string, input: JsonValue, description: string | null, reason: string | null, 
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
activity?: string, toolUses?: number, tokens?: number, durationMs?: number, summary?: string, background?: boolean, } | { "type": "context", used: number, } | { "type": "context_windows", windows: { [key in string]: number }, } | { "type": "limits", usage: PlanUsage, } | { "type": "done", costUsd: number | null, isError: boolean, } | { "type": "error", message: string, } | { "type": "cancelled" } | { "type": "raw", line: string, };

export type AgentInfo = { kind: AgentKind, name: string, installed: boolean, };

export type AgentKind = "claude" | "codex" | "opencode";

/**
 * Answers an approval request (or a question) the session's agent is waiting on.
 */
export type AnswerApprovalArgs = { sessionId: string, requestId: string, decision: Decision, message?: string, 
/**
 * For a question from the agent: the chosen answer per question text.
 */
answers?: { [key in string]: string }, };

/**
 * Every command fails with one of these.
 */
export type ApiError = { code: ErrorCode, 
/**
 * Written for the user.
 */
message: string, };

/**
 * A command sent over the WebSocket bridge. `id` is echoed back in the reply.
 */
export type ClientFrame = { id: number, cmd: string, args?: JsonValue, };

/**
 * Ends the terminal's shell (and what runs in it) and forgets the terminal.
 */
export type CloseTerminalArgs = { terminalId: string, };

/**
 * One process of a process-compose project.
 */
export type ComposeProcess = { name: string, 
/**
 * process-compose's own word for it: Running, Completed, Pending, Restarting, Disabled, …
 */
status: string, running: boolean, 
/**
 * Whether its readiness probe passes; None when it has no probe.
 */
ready: boolean | null, restarts: number, 
/**
 * Only meaningful once it has stopped running.
 */
exitCode: number, pid: number, 
/**
 * TCP ports it (or anything it started) listens on.
 */
ports: Array<number>, };

/**
 * Where the desktop app's agents run.
 */
export type Connection = { "mode": "local" } | { "mode": "ssh", 
/**
 * As you would pass it to `ssh`: a `~/.ssh/config` alias, `host`, or `user@host`.
 */
host: string, 
/**
 * How to run bach-server there (default `bach-server`).
 */
command: string, };

export type ConnectionState = "connecting" | "connected" | "disconnected";

/**
 * How the desktop app's connection is doing. Sent on the `bach-connection` channel as it changes.
 */
export type ConnectionStatus = { connection: Connection, state: ConnectionState, 
/**
 * Why it isn't connected.
 */
error: string | null, 
/**
 * Whether it will try again by itself.
 */
retrying: boolean, 
/**
 * The server is there but speaks another protocol than this app: restarting it (or the app)
 * is the way out.
 */
incompatible: boolean, 
/**
 * The server's version, once connected.
 */
version: string | null, };

/**
 * How much of the model's context window a session's conversation fills.
 */
export type ContextUsage = { 
/**
 * Tokens in the conversation after the agent's latest message.
 */
used: number, 
/**
 * The model's context window, once the agent has reported it.
 */
window: number | null, };

/**
 * How the user answered an approval request.
 */
export type Decision = "allow" | "allow_session" | "allow_always" | "deny";

/**
 * Deletes a session and stops its run (its background tasks keep going).
 */
export type DeleteSessionArgs = { sessionId: string, };

export type DirEntry = { name: string, path: string, 
/**
 * Contains a `.git`, i.e. probably a project root.
 */
git: boolean, };

export type DirListing = { path: string, parent: string | null, home: string, entries: Array<DirEntry>, };

/**
 * One step of a session's transcript.
 */
export type Entry = { "type": "user", text: string, } | { "type": "agent", runId: string, event: AgentEvent, } | { "type": "decision", requestId: string, decision: Decision, answers?: { [key in string]: string }, } | { "type": "failed", message: string, retryText?: string, } | { "type": "imported", blocks: JsonValue, };

/**
 * What kind of failure an [`ApiError`] is, for code that reacts to it. People read `message`.
 */
export type ErrorCode = "invalid" | "not_found" | "unavailable" | "failed";

export type ExitStatus = { code: number | null, };

/**
 * Port forwarding in the desktop app. Sent on the `bach-forwards` channel as it changes.
 */
export type Forwarding = { 
/**
 * Only connections over SSH forward; on this computer, ports are already local.
 */
available: boolean, 
/**
 * Forward the ports background tasks listen on without being asked.
 */
auto: boolean, forwards: Array<PortForward>, 
/**
 * The latest thing that went wrong, if anything.
 */
error: string | null, };

/**
 * A session and its transcript, or only the entries after `afterSeq`.
 */
export type GetSessionArgs = { sessionId: string, afterSeq?: number, };

/**
 * The account's usage limits as last reported by an agent run (none before the first run).
 */
export type GetUsageArgs = Record<symbol, never>;

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
 * One usage-limit window, e.g. the 5-hour or the weekly limit.
 */
export type LimitWindow = { 
/**
 * Share used, 0.0 to 1.0.
 */
utilization: number, 
/**
 * When the window resets (ms since the epoch).
 */
resetsAt: number | null, };

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
 * Every session, most recently active first (without transcripts).
 */
export type ListSessionsArgs = Record<symbol, never>;

/**
 * Every task, oldest first.
 */
export type ListTasksArgs = Record<symbol, never>;

/**
 * Terminals the backend keeps, oldest first (including ones whose shell has ended).
 */
export type ListTerminalsArgs = Record<symbol, never>;

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

export type LogEntry = { 
/**
 * 1, 2, 3, ... within its session.
 */
seq: number, 
/**
 * When it was recorded (ms since the epoch).
 */
at: number, entry: Entry, };

/**
 * Starts your shell in a terminal on the backend host.
 */
export type OpenTerminalArgs = { 
/**
 * Defaults to the home directory.
 */
cwd?: string, cols: number, rows: number, };

/**
 * The account's usage limits as the agent last reported them. Only updated while agents run.
 */
export type PlanUsage = { 
/**
 * `allowed`, or why requests are being refused.
 */
status: string, 
/**
 * By the agent's name for the window: `five_hour`, `seven_day`, ...
 */
windows: { [key in string]: LimitWindow }, 
/**
 * When this was reported (ms since the epoch).
 */
observedAt: number, };

/**
 * A port on the agents' machine reachable on this computer.
 */
export type PortForward = { 
/**
 * The port there.
 */
remote: number, 
/**
 * Where it is here (`localhost:<local>`); the same number when that was free.
 */
local: number, 
/**
 * Forwarded because a background task listens on it; ends when no running task does.
 */
auto: boolean, };

/**
 * What to do to one process of a process-compose task.
 */
export type ProcessAction = "start" | "stop" | "restart";

/**
 * Forgets a finished task and deletes its log.
 */
export type RemoveTaskArgs = { taskId: string, };

/**
 * Removes a worktree Bach created. Sessions that ran in it can still be read, not continued.
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

export type ResizeTerminalArgs = { terminalId: string, cols: number, rows: number, };

/**
 * Sends a message to a session's agent. The session must not be running.
 */
export type SendMessageArgs = { sessionId: string, prompt: string, };

/**
 * Everything the backend pushes to clients, whichever transport carries it.
 */
export type ServerEvent = { "topic": "session", "data": SessionEvent } | { "topic": "task", "data": TaskEvent } | { "topic": "usage", "data": PlanUsage } | { "topic": "terminal", "data": TerminalEvent };

/**
 * What the WebSocket bridge (and `bach-server attach`) sends: first a `hello`, then replies to
 * commands, and events.
 */
export type ServerFrame = { "kind": "hello", protocol: string, version: string, } | { "kind": "reply", id: number, result: JsonValue, } | { "kind": "error", id: number, error: ApiError, } | { "kind": "event", event: ServerEvent, };

export type Session = { id: string, title: string, 
/**
 * The user named it, so the first prompt shouldn't.
 */
titleEdited: boolean, agent: AgentKind, 
/**
 * The project folder; sessions are grouped by it.
 */
cwd: string, 
/**
 * The git branch it was started on (or, with `worktree`, branched from).
 */
branch: string | null, 
/**
 * Runs in an isolated git worktree on its own branch.
 */
worktree: boolean, 
/**
 * Model choice for Claude Code: an alias like `opus`, or none for the default.
 */
modelChoice: string | null, 
/**
 * Permission mode for Claude Code (`acceptEdits`, `auto`, `plan`, …), or none for its
 * default: asking before anything that isn't read-only.
 */
permissionMode: string | null, 
/**
 * Where the agent runs (a worktree, or `cwd`).
 */
workdir: string | null, 
/**
 * The branch it runs on.
 */
gitBranch: string | null, 
/**
 * Its worktree was removed: it can be read but not continued.
 */
workdirRemoved: boolean, 
/**
 * The agent's own session id, to resume the conversation.
 */
agentSessionId: string | null, 
/**
 * Permission rules approved "for this session"; every later run gets them.
 */
allowRules: Array<string>, 
/**
 * The model the agent reported using on its latest run.
 */
model: string | null, 
/**
 * How full the conversation's context is, as of the agent's latest message.
 */
context: ContextUsage | null, 
/**
 * Put away: kept with its transcript, but out of the sidebar's main list.
 */
archived: boolean, 
/**
 * The agent run in progress, if any.
 */
runId: string | null, 
/**
 * Approval requests the running agent is waiting on.
 */
openApprovals: Array<string>, createdAt: number, 
/**
 * Last activity; sessions are listed most recent first.
 */
updatedAt: number, 
/**
 * `seq` of the latest transcript entry (0 when there are none).
 */
lastSeq: number, };

/**
 * A change to sessions, pushed to every client.
 */
export type SessionEvent = { "type": "changed", session: Session, } | { "type": "entry", sessionId: string, entry: LogEntry, } | { "type": "deleted", sessionId: string, };

/**
 * A session with (part of) its transcript.
 */
export type SessionLog = { session: Session, entries: Array<LogEntry>, };

/**
 * Creates a session and sends its first message. Readies where it runs first: switches `cwd` to
 * `branch`, or (with `worktree`) creates a new branch from it in an isolated worktree. Nothing is
 * saved if that, or starting the agent, fails.
 */
export type StartSessionArgs = { agent: AgentKind, cwd: string, branch?: string, worktree?: boolean, 
/**
 * The worktree's branch name; made up from the prompt when not given.
 */
newBranch?: string, modelChoice?: string, permissionMode?: string, prompt: string, };

/**
 * Starts a command by hand, as a task of the project in `cwd`.
 */
export type StartTaskArgs = { command: string, cwd: string, name?: string, };

/**
 * Stops the session's agent run, if one is going.
 */
export type StopSessionArgs = { sessionId: string, };

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
expectedPorts: Array<number>, 
/**
 * For a process-compose project started by Satie: its compose file.
 */
composeFile: string | null, };

/**
 * A change to the task list.
 */
export type TaskEvent = { "type": "changed", task: TaskView, } | { "type": "removed", id: string, };

/**
 * A piece of a task's log: from a byte offset, or (without one) its tail.
 */
export type TaskLogChunkArgs = { taskId: string, 
/**
 * One process of a process-compose task; by default the task's whole output.
 */
process?: string, from?: number, 
/**
 * At most this much (default 512 KiB).
 */
maxBytes?: number, };

/**
 * The last lines of a task's log.
 */
export type TaskLogsArgs = { taskId: string, 
/**
 * One process of a process-compose task; by default the task's whole output.
 */
process?: string, 
/**
 * How many lines (default 200, at most 2000).
 */
lines?: number, };

/**
 * Starts, stops or restarts one process of a process-compose task.
 */
export type TaskProcessArgs = { taskId: string, process: string, action: ProcessAction, };

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
processes: Array<string>, 
/**
 * The processes of a process-compose project running in the task, as its API reports them
 * (the last report, once the task has ended). None for any other task.
 */
compose: Array<ComposeProcess> | null, id: string, name: string, command: string, 
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
expectedPorts: Array<number>, 
/**
 * For a process-compose project started by Satie: its compose file.
 */
composeFile: string | null, };

export type TerminalEvent = { "type": "opened", terminal: TerminalInfo, } | { "type": "output", terminalId: string, seq: number, data: string, } | { "type": "exited", terminalId: string, status: ExitStatus, } | { "type": "closed", terminalId: string, };

export type TerminalInfo = { id: string, 
/**
 * The folder it started in.
 */
cwd: string, 
/**
 * The shell it runs, e.g. `zsh`.
 */
shell: string, cols: number, rows: number, createdAt: number, 
/**
 * Set once the shell has ended (its exit code, if it had one). It stays listed until closed.
 */
exited: ExitStatus | null, };

/**
 * Typed (or pasted) text for the terminal.
 */
export type TerminalInputArgs = { terminalId: string, data: string, };

/**
 * A terminal and its recent output, to draw it from scratch.
 */
export type TerminalSnapshot = { terminal: TerminalInfo, 
/**
 * The scrollback kept by the backend, base64.
 */
data: string, 
/**
 * The `seq` of the last output included; later output events follow on from it.
 */
seq: number, };

/**
 * A terminal and its recent output.
 */
export type TerminalSnapshotArgs = { terminalId: string, };

/**
 * Renames a session, archives or restores it, or changes the model or permission mode its next
 * messages use (`""` for the default). Archiving stops its run.
 */
export type UpdateSessionArgs = { sessionId: string, title?: string, modelChoice?: string, permissionMode?: string, archived?: boolean, };

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
   * The account's usage limits as last reported by an agent run (none before the first run).
   */
  get_usage: { args: GetUsageArgs; output: PlanUsage | null };
  /**
   * Every session, most recently active first (without transcripts).
   */
  list_sessions: { args: ListSessionsArgs; output: Array<Session> };
  /**
   * A session and its transcript, or only the entries after `afterSeq`.
   */
  get_session: { args: GetSessionArgs; output: SessionLog };
  /**
   * Creates a session and sends its first message. Readies where it runs first: switches `cwd` to
   * `branch`, or (with `worktree`) creates a new branch from it in an isolated worktree. Nothing is
   * saved if that, or starting the agent, fails.
   */
  start_session: { args: StartSessionArgs; output: Session };
  /**
   * Sends a message to a session's agent. The session must not be running.
   */
  send_message: { args: SendMessageArgs; output: Session };
  /**
   * Stops the session's agent run, if one is going.
   */
  stop_session: { args: StopSessionArgs; output: null };
  /**
   * Answers an approval request (or a question) the session's agent is waiting on.
   */
  answer_approval: { args: AnswerApprovalArgs; output: null };
  /**
   * Renames a session, archives or restores it, or changes the model or permission mode its next
   * messages use (`""` for the default). Archiving stops its run.
   */
  update_session: { args: UpdateSessionArgs; output: Session };
  /**
   * Deletes a session and stops its run (its background tasks keep going).
   */
  delete_session: { args: DeleteSessionArgs; output: null };
  /**
   * Lists sub-directories on the backend host, for the folder picker.
   */
  list_dir: { args: ListDirArgs; output: DirListing };
  /**
   * Branches and state of the repository containing `path`.
   */
  git_info: { args: GitInfoArgs; output: GitInfo };
  /**
   * Worktrees Bach created on the backend host.
   */
  list_worktrees: { args: ListWorktreesArgs; output: Array<WorktreeEntry> };
  /**
   * Removes a worktree Bach created. Sessions that ran in it can still be read, not continued.
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
   * Starts, stops or restarts one process of a process-compose task.
   */
  task_process: { args: TaskProcessArgs; output: null };
  /**
   * Terminals the backend keeps, oldest first (including ones whose shell has ended).
   */
  list_terminals: { args: ListTerminalsArgs; output: Array<TerminalInfo> };
  /**
   * Starts your shell in a terminal on the backend host.
   */
  open_terminal: { args: OpenTerminalArgs; output: TerminalInfo };
  /**
   * A terminal and its recent output.
   */
  terminal_snapshot: { args: TerminalSnapshotArgs; output: TerminalSnapshot };
  /**
   * Typed (or pasted) text for the terminal.
   */
  terminal_input: { args: TerminalInputArgs; output: null };
  resize_terminal: { args: ResizeTerminalArgs; output: null };
  /**
   * Ends the terminal's shell (and what runs in it) and forgets the terminal.
   */
  close_terminal: { args: CloseTerminalArgs; output: null };
};
