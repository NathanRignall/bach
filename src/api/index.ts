// The typed client for Bach's backend. Commands, their arguments and results, and every type they
// use are generated from Rust (crates/bach-protocol) into ./generated/protocol.ts.
import { remoteUrl } from "./backend";
import type { Commands, RunEvent, ServerEvent, TaskEvent } from "./generated/protocol";
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

/** Events from agent runs. */
export const onRunEvent = (cb: (e: RunEvent) => void) =>
  onEvent((e) => {
    if (e.topic === "run") cb(e.data);
  });

// Shorthands for the commands the UI uses most.
export const listAgents = () => call("list_agents");
export const startRun = (args: Args<"start_run">) => call("start_run", args);
export const respondApproval = (args: Args<"respond_approval">) => call("respond_approval", args);
export const cancelRun = (runId: string) => call("cancel_run", { runId });

export const listDir = (path?: string, showHidden = false) => call("list_dir", { path, showHidden });
export const gitInfo = (path: string) => call("git_info", { path });
export const prepareWorkspace = (args: Args<"prepare_workspace">) => call("prepare_workspace", args);
export const listWorktrees = () => call("list_worktrees");
export const removeWorktree = (args: Args<"remove_worktree">) => call("remove_worktree", args);

export const listTasks = () => call("list_tasks");
export const taskLogs = (taskId: string, lines = 200) => call("task_logs", { taskId, lines });
export const taskLogChunk = (taskId: string, from?: number, maxBytes?: number) =>
  call("task_log_chunk", { taskId, from, maxBytes });
export const stopTask = (taskId: string) => call("stop_task", { taskId });
export const removeTask = (taskId: string) => call("remove_task", { taskId });
export const startTask = (args: Args<"start_task">) => call("start_task", args);

/** Sessions are saved by whichever backend is in use (local app data, or the bach-server host). */
export const listSessions = () => call("list_sessions");
export const saveSession = (session: object) => call("save_session", { session: session as Args<"save_session">["session"] });
export const deleteSession = (sessionId: string) => call("delete_session", { sessionId });
