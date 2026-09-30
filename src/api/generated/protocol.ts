// Generated from crates/bach-protocol by `cargo test -p bach-protocol`. Don't edit.

/** Must match the server's `hello`; see `fingerprint` in bach-protocol. */
export const PROTOCOL = "382c021c2ad258d2";

/**
 * Agent-independent events the UI renders.
 */
export type AgentEvent = { "type": "session", id: string, 
/**
 * The model the agent reports using for this run.
 */
model?: string, } | { "type": "text", text: string, parent?: string, } | { "type": "thinking", text: string, } | { "type": "delta", id: string, kind: DeltaKind, text: string, } | { "type": "tool_use", id: string, name: string, input: JsonValue, parent?: string, } | { "type": "tool_result", id: string, output: string, isError: boolean, 
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

export type AgentInfo = { kind: AgentKind, name: string, installed: boolean, 
/**
 * The modes a session can choose, in picker order; the only ones a run accepts.
 */
permissionModes: Array<PermissionModeInfo>, };

export type AgentKind = "claude" | "codex" | "opencode";

/**
 * A command bach-server starts the agent CLIs through, taking their command line after it:
 * `sandbox` runs Claude Code as `sandbox claude …`. Fixed when the server starts
 * (`BACH_AGENT_WRAPPER`); `""` starts them directly.
 */
export type AgentWrapper = { command: string, 
/**
 * Codex's own sandbox inside the wrapper's (`BACH_AGENT_WRAPPER_CODEX_SANDBOX`). Off, the
 * wrapper's sandbox is the only one (many can't have Codex's inside them), and Codex's modes
 * are kept by asking instead (see [`AgentInfo::permission_modes`]).
 */
codexSandbox: boolean, };

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
command: string, 
/**
 * Given to bach-server there when the app starts or restarts it. An empty command leaves
 * the server's own (`BACH_AGENT_WRAPPER`): the app never takes a wrapper away.
 */
wrapper: AgentWrapper, };

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
version: string | null, 
/**
 * How the server starts agents, once connected.
 */
wrapper: AgentWrapper | null, 
/**
 * The server was started with another agent wrapper than the one set here (and only a new
 * server takes it): new agent turns are refused until it restarts.
 */
wrapperMismatch: boolean, };

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

/**
 * What a [`AgentEvent::Delta`] adds to.
 */
export type DeltaKind = "text" | "thinking" | "output";

export type DiffHunk = { 
/**
 * The `@@ -a,b +c,d @@ context` line.
 */
header: string, lines: Array<DiffLine>, };

export type DiffLine = { kind: LineKind, text: string, 
/**
 * Line number in the old file (none for added lines).
 */
old: number | null, 
/**
 * Line number in the new file (none for deleted lines).
 */
new: number | null, 
/**
 * The file doesn't end with a newline after this line.
 */
noNewline: boolean, };

export type DirEntry = { name: string, path: string, 
/**
 * Contains a `.git`, i.e. probably a project root.
 */
git: boolean, };

export type DirListing = { path: string, parent: string | null, home: string, entries: Array<DirEntry>, };

/**
 * One step of a session's transcript.
 */
export type Entry = { "type": "user", text: string, 
/**
 * Images, PDFs and text files sent with it, as `data:` URLs (the name is from when only
 * images could be). A file's name is a parameter: `data:application/pdf;name=a.pdf;base64,…`.
 */
images?: Array<string>, } | { "type": "agent", runId: string, event: AgentEvent, } | { "type": "decision", requestId: string, decision: Decision, answers?: { [key in string]: string }, } | { "type": "failed", message: string, retryText?: string, } | { "type": "imported", blocks: JsonValue, };

/**
 * What kind of failure an [`ApiError`] is, for code that reacts to it. People read `message`.
 */
export type ErrorCode = "invalid" | "not_found" | "unavailable" | "failed";

export type ExitStatus = { code: number | null, };

export type FileDiff = { path: string, 
/**
 * Where a renamed file was.
 */
oldPath: string | null, status: FileStatus, 
/**
 * Not yet tracked by git.
 */
untracked: boolean, binary: boolean, additions: number, deletions: number, 
/**
 * Empty for binary files, and for files too large to show (`omitted`).
 */
hunks: Array<DiffHunk>, omitted: boolean, };

export type FileStatus = "added" | "modified" | "deleted" | "renamed";

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

/**
 * The changes in a checkout, file by file.
 */
export type GitDiff = { 
/**
 * The commit the working tree is compared with (abbreviated), or none before the first commit.
 */
base: string | null, files: Array<FileDiff>, 
/**
 * Some files' lines were left out to keep the diff a reasonable size.
 */
truncated: boolean, };

/**
 * The changes in the checkout at `path`: uncommitted ones (including untracked files), or with
 * `baseBranch`, everything since the branch left it (committed or not).
 */
export type GitDiffArgs = { path: string, baseBranch?: string, };

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

export type LineKind = "context" | "add" | "delete";

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
 * The models an agent can run. Codex and opencode are asked for their current lists (kept for
 * a while); Claude Code's are its aliases. opencode's depend on the project folder (`cwd`),
 * which may add providers of its own.
 */
export type ListModelsArgs = { agent: AgentKind, cwd?: string, };

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
 * A model an agent can run, for the model picker.
 */
export type ModelInfo = { 
/**
 * What to pass as the session's model choice.
 */
id: string, name: string, description: string, 
/**
 * The one the agent uses when none is chosen.
 */
isDefault: boolean, 
/**
 * Its thinking effort levels, least first (none if it can't be set).
 */
efforts: Array<string>, 
/**
 * The level it uses when none is chosen, if known.
 */
defaultEffort: string | null, };

/**
 * Starts your shell in a terminal on the backend host.
 */
export type OpenTerminalArgs = { 
/**
 * Defaults to the home directory.
 */
cwd?: string, cols: number, rows: number, };

/**
 * A permission mode an agent can run in, for the mode picker.
 */
export type PermissionModeInfo = { 
/**
 * What to pass as the session's permission mode.
 */
id: string, name: string, 
/**
 * What it lets the agent do, as it behaves on this server (Codex's modes mean less under a
 * wrapper).
 */
description: string, 
/**
 * The one the agent runs in when none is chosen.
 */
isDefault: boolean, };

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
 * A message waiting for the agent to finish its current run.
 */
export type QueuedMessage = { id: string, text: string, 
/**
 * Images, PDFs and text files sent with it, as `data:` URLs (the name is from when only
 * images could be). A file's name is a parameter: `data:application/pdf;name=a.pdf;base64,…`.
 */
images?: Array<string>, };

/**
 * An image file on the backend host as a `data:` URL, so the UI can show images an agent
 * links to by path. Only image files, up to 25 MB.
 */
export type ReadImageArgs = { 
/**
 * Absolute, or starting with `~`.
 */
path: string, };

/**
 * Takes a message out of the queue without sending it.
 */
export type RemoveQueuedArgs = { sessionId: string, messageId: string, };

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
 * Sends a message to a session's agent; while it is busy, the message is queued instead.
 */
export type SendMessageArgs = { sessionId: string, prompt: string, 
/**
 * Images, PDFs and text files sent with the prompt, as `data:` URLs (see `Entry::User`).
 */
images?: Array<string>, };

/**
 * Sends a queued message now. The session must not be running.
 */
export type SendQueuedArgs = { sessionId: string, messageId: string, };

/**
 * Everything the backend pushes to clients, whichever transport carries it.
 */
export type ServerEvent = { "topic": "session", "data": SessionEvent } | { "topic": "task", "data": TaskEvent } | { "topic": "usage", "data": PlanUsage } | { "topic": "terminal", "data": TerminalEvent };

/**
 * What the WebSocket bridge (and `bach-server attach`) sends: first a `hello`, then replies to
 * commands, and events.
 */
export type ServerFrame = { "kind": "hello", protocol: string, version: string, 
/**
 * How the server starts agents; see [`AgentWrapper`].
 */
wrapper: AgentWrapper, } | { "kind": "reply", id: number, result: JsonValue, } | { "kind": "error", id: number, error: ApiError, } | { "kind": "event", event: ServerEvent, };

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
 * How hard the agent thinks: one of its model's effort levels (`low`, `high`, …), or none
 * for the model's default.
 */
effort: string | null, 
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
openApprovals: Array<string>, 
/**
 * Messages sent while the agent was busy, oldest first. The next one goes when a run
 * finishes cleanly; a stopped or failed run leaves them waiting.
 */
queued: Array<QueuedMessage>, createdAt: number, 
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
export type SessionEvent = { "type": "changed", session: Session, } | { "type": "entry", sessionId: string, entry: LogEntry, } | { "type": "delta", sessionId: string, runId: string, id: string, kind: DeltaKind, text: string, } | { "type": "deleted", sessionId: string, };

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
newBranch?: string, modelChoice?: string, permissionMode?: string, 
/**
 * Thinking effort (one of the model's levels); `""` for the model's default.
 */
effort?: string, prompt: string, 
/**
 * Images, PDFs and text files sent with the prompt, as `data:` URLs (see `Entry::User`).
 */
images?: Array<string>, };

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
composeFile: string | null, 
/**
 * Whether the user is meant to open it (a dev server they browse to) rather than only the
 * agent using it (a server for tests). Only interactive tasks' ports are forwarded automatically.
 */
interactive: boolean, };

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
composeFile: string | null, 
/**
 * Whether the user is meant to open it (a dev server they browse to) rather than only the
 * agent using it (a server for tests). Only interactive tasks' ports are forwarded automatically.
 */
interactive: boolean, };

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
export type UpdateSessionArgs = { sessionId: string, title?: string, modelChoice?: string, permissionMode?: string, 
/**
 * Thinking effort (one of the model's levels); `""` for the model's default.
 */
effort?: string, archived?: boolean, };

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
   * The models an agent can run. Codex and opencode are asked for their current lists (kept for
   * a while); Claude Code's are its aliases. opencode's depend on the project folder (`cwd`),
   * which may add providers of its own.
   */
  list_models: { args: ListModelsArgs; output: Array<ModelInfo> };
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
   * Sends a message to a session's agent; while it is busy, the message is queued instead.
   */
  send_message: { args: SendMessageArgs; output: Session };
  /**
   * Sends a queued message now. The session must not be running.
   */
  send_queued: { args: SendQueuedArgs; output: Session };
  /**
   * Takes a message out of the queue without sending it.
   */
  remove_queued: { args: RemoveQueuedArgs; output: Session };
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
   * An image file on the backend host as a `data:` URL, so the UI can show images an agent
   * links to by path. Only image files, up to 25 MB.
   */
  read_image: { args: ReadImageArgs; output: string };
  /**
   * Branches and state of the repository containing `path`.
   */
  git_info: { args: GitInfoArgs; output: GitInfo };
  /**
   * The changes in the checkout at `path`: uncommitted ones (including untracked files), or with
   * `baseBranch`, everything since the branch left it (committed or not).
   */
  git_diff: { args: GitDiffArgs; output: GitDiff };
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
