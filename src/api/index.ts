// The typed client for Bach's backend. Commands, their arguments and results, and every type they
// use are generated from Rust (crates/bach-protocol) into ./generated/protocol.ts.
import { remoteUrl } from "./backend";
import type { AgentKind, Commands, PlanUsage, ProcessAction, ServerEvent, SessionEvent, TaskEvent, TerminalEvent } from "./generated/protocol";
import { SocketTransport, TauriTransport, type Transport } from "./transport";

export * from "./generated/protocol";
export { ApiError } from "./transport";
export {
  forwardPort,
  getConnection,
  getForwarding,
  inTauri,
  macTitleBar,
  onConnection,
  onForwarding,
  openPort,
  openUrl,
  relaunchApp,
  remoteUrl,
  restartServer,
  setAutoForward,
  setConnection,
  sshHosts,
  stopForward,
  taskHost,
} from "./backend";

const transport: Transport = remoteUrl ? new SocketTransport(remoteUrl) : new TauriTransport();

export type CommandName = keyof Commands;
export type Args<K extends CommandName> = Commands[K]["args"];
export type Output<K extends CommandName> = Commands[K]["output"];

/** Runs a backend command. Commands whose arguments are all optional can be called without any. */
export function call<K extends CommandName>(
  cmd: K,
  ...args: {} extends Args<K> ? [args?: Args<K>] : [args: Args<K>]
): Promise<Output<K>> {
  return transport.call(cmd, args[0] ?? {}) as Promise<Output<K>>;
}

/** Every event the backend pushes. Returns an unsubscribe function. */
export const onEvent = (cb: (e: ServerEvent) => void) => transport.subscribe(cb);

/** Called when the connection to a remote backend came back; refetch anything followed by events. */
export const onReconnect = (cb: () => void) => transport.onReconnect(cb);

/** New readings of the account's usage limits. */
export const onUsage = (cb: (u: PlanUsage) => void) =>
  onEvent((e) => {
    if (e.topic === "usage") cb(e.data);
  });

/** Terminals opening, closing, and their output. */
export const onTerminalEvent = (cb: (e: TerminalEvent) => void) =>
  onEvent((e) => {
    if (e.topic === "terminal") cb(e.data);
  });

/** Changes to background tasks. */
export const onTaskEvent = (cb: (e: TaskEvent) => void) =>
  onEvent((e) => {
    if (e.topic === "task") cb(e.data);
  });

/** Changes to sessions and their transcripts. */
export const onSessionEvent = (cb: (e: SessionEvent) => void) =>
  onEvent((e) => {
    if (e.topic === "session") cb(e.data);
  });

// Shorthands for the commands the UI uses most.
export const listAgents = () => call("list_agents");
export const listModels = (agent: AgentKind, cwd?: string) => call("list_models", { agent, cwd });
export const getUsage = () => call("get_usage");

export const listSessions = () => call("list_sessions");
export const getSession = (sessionId: string, afterSeq?: number) => call("get_session", { sessionId, afterSeq });
export const startSession = (args: Args<"start_session">) => call("start_session", args);
export const sendMessage = (sessionId: string, prompt: string, images: string[] = []) =>
  call("send_message", { sessionId, prompt, images });
export const sendQueued = (sessionId: string, messageId: string) => call("send_queued", { sessionId, messageId });
export const removeQueued = (sessionId: string, messageId: string) => call("remove_queued", { sessionId, messageId });
export const stopSession = (sessionId: string) => call("stop_session", { sessionId });
export const answerApproval = (args: Args<"answer_approval">) => call("answer_approval", args);
export const markSeen = (sessionId: string) => call("mark_seen", { sessionId });
export const updateSession = (args: Args<"update_session">) => call("update_session", args);
export const deleteSession = (sessionId: string) => call("delete_session", { sessionId });

export const listDir = (path?: string, showHidden = false) => call("list_dir", { path, showHidden });
/** An image file on the backend host as a `data:` URL. */
export const readImage = (path: string) => call("read_image", { path });
/** Every file in the session's folder that git doesn't ignore, relative to it. */
export const listFiles = (sessionId: string) => call("list_files", { sessionId });
/** A file in the session's folder; `path` is relative to it. */
export const readFile = (sessionId: string, path: string) => call("read_file", { sessionId, path });
export const gitInfo = (path: string) => call("git_info", { path });
export const gitDiff = (path: string, baseBranch?: string, commit?: string) => call("git_diff", { path, baseBranch, commit });
export const gitLog = (args: Args<"git_log">) => call("git_log", args);
export const gitStatus = (path: string) => call("git_status", { path });
export const gitStage = (path: string, files: string[], stage: boolean) => call("git_stage", { path, files, stage });
export const gitStageHunk = (args: Args<"git_stage_hunk">) => call("git_stage_hunk", args);
export const gitCommit = (path: string, message: string) => call("git_commit", { path, message });
export const gitPush = (path: string) => call("git_push", { path });
export const listWorktrees = () => call("list_worktrees");
export const removeWorktree = (args: Args<"remove_worktree">) => call("remove_worktree", args);

export const listTasks = () => call("list_tasks");
/** A task's output; with `process`, just that process's (for a process-compose task). */
export const taskLogs = (taskId: string, lines = 200, process?: string) => call("task_logs", { taskId, lines, process });
export const taskLogChunk = (taskId: string, from?: number, maxBytes?: number, process?: string) =>
  call("task_log_chunk", { taskId, from, maxBytes, process });
export const taskProcess = (taskId: string, process: string, action: ProcessAction) => call("task_process", { taskId, process, action });
export const stopTask = (taskId: string) => call("stop_task", { taskId });
export const removeTask = (taskId: string) => call("remove_task", { taskId });

export const listTerminals = () => call("list_terminals");
export const openTerminal = (args: Args<"open_terminal">) => call("open_terminal", args);
export const terminalSnapshot = (terminalId: string) => call("terminal_snapshot", { terminalId });
export const terminalInput = (terminalId: string, data: string) => call("terminal_input", { terminalId, data });
export const resizeTerminal = (terminalId: string, cols: number, rows: number) => call("resize_terminal", { terminalId, cols, rows });
export const closeTerminal = (terminalId: string) => call("close_terminal", { terminalId });
