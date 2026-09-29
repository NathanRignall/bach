// The typed client for Bach's backend. Commands, their arguments and results, and every type they
// use are generated from Rust (crates/bach-protocol) into ./generated/protocol.ts.
import { remoteUrl } from "./backend";
import type { Commands, ServerEvent, SessionEvent, TaskEvent } from "./generated/protocol";
import { SocketTransport, TauriTransport, type Transport } from "./transport";

export * from "./generated/protocol";
export { ApiError } from "./transport";
export { canSwitchBackend, remoteUrl, setBackend, taskHost } from "./backend";

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

export const listSessions = () => call("list_sessions");
export const getSession = (sessionId: string, afterSeq?: number) => call("get_session", { sessionId, afterSeq });
export const startSession = (args: Args<"start_session">) => call("start_session", args);
export const sendMessage = (sessionId: string, prompt: string) => call("send_message", { sessionId, prompt });
export const stopSession = (sessionId: string) => call("stop_session", { sessionId });
export const answerApproval = (args: Args<"answer_approval">) => call("answer_approval", args);
export const updateSession = (args: Args<"update_session">) => call("update_session", args);
export const deleteSession = (sessionId: string) => call("delete_session", { sessionId });

export const listDir = (path?: string, showHidden = false) => call("list_dir", { path, showHidden });
export const gitInfo = (path: string) => call("git_info", { path });
export const listWorktrees = () => call("list_worktrees");
export const removeWorktree = (args: Args<"remove_worktree">) => call("remove_worktree", args);

export const listTasks = () => call("list_tasks");
export const taskLogs = (taskId: string, lines = 200) => call("task_logs", { taskId, lines });
export const taskLogChunk = (taskId: string, from?: number, maxBytes?: number) =>
  call("task_log_chunk", { taskId, from, maxBytes });
export const stopTask = (taskId: string) => call("stop_task", { taskId });
export const removeTask = (taskId: string) => call("remove_task", { taskId });
export const startTask = (args: Args<"start_task">) => call("start_task", args);
