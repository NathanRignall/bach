import { useEffect, useState } from "react";
import { AlertTriangle, Ban, CheckCircle2, ScrollText, Square, Trash2, X, XCircle } from "lucide-react";
import { Forwarding, TaskStatus, TaskView, listTasks, onReconnect, onTaskEvent, removeTask, stopTask } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { ComposeProcesses } from "./ComposeProcesses";
import { LogViewer } from "./LogViewer";
import { ResizeHandle } from "./ResizeHandle";
import { PortLink, useForwarding } from "./Ports";
import { projectName } from "@/session";

/** Background tasks on the backend host: listed once, then kept current by the backend's task events. */
export function useTasks() {
  const [tasks, setTasks] = useState<TaskView[]>([]);
  const [error, setError] = useState<string>();
  const refresh = () =>
    listTasks()
      .then((t) => (setTasks(t), setError(undefined)))
      .catch((e) => setError(String(e.message ?? e)));
  useEffect(() => {
    const unTasks = onTaskEvent((e) =>
      setTasks((all) => {
        if (e.type === "removed") return all.filter((t) => t.id !== e.id);
        const i = all.findIndex((t) => t.id === e.task.id);
        // New tasks go first, like the list (newest first).
        return i < 0 ? [e.task, ...all] : all.map((t, j) => (j === i ? e.task : t));
      }),
    );
    const unReconnect = onReconnect(() => void refresh());
    void refresh();
    return () => (unTasks(), unReconnect());
  }, []);
  return { tasks, error, refresh };
}

/** The current time, ticking every `ms` while `on`. */
function useNow(on: boolean, ms = 1000) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [on, ms]);
  return now;
}

/** A dev stack can listen on many ports; show the first few and say how many more. */
const MAX_PORTS = 6;

function duration(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

export const STATUS: Record<TaskStatus, string> = {
  running: "Running",
  exited: "Finished",
  failed: "Failed",
  stopped: "Stopped",
  lost: "Lost",
};

export function StatusIcon({ task }: { task: TaskView }) {
  switch (task.status) {
    case "running":
      return <Spinner className="size-4 text-primary" aria-label="Running" />;
    case "exited":
      return <CheckCircle2 className="size-4 text-muted-foreground" aria-label="Finished" />;
    case "failed":
      return <XCircle className="size-4 text-destructive" aria-label="Failed" />;
    case "stopped":
      return <Ban className="size-4 text-muted-foreground" aria-label="Stopped" />;
    case "lost":
      return <AlertTriangle className="size-4 text-muted-foreground" aria-label="Lost" />;
  }
}

function TaskCard({
  task,
  forwarding,
  onChanged,
  onOpenViewer,
}: {
  task: TaskView;
  forwarding: Forwarding | null;
  onChanged: () => void;
  onOpenViewer: (process?: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const running = task.status === "running";
  const now = useNow(running);
  const act = async (f: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await f();
      setError(undefined);
    } catch (e) {
      setError(String((e as Error).message ?? e));
    }
    setBusy(false);
    onChanged();
  };

  return (
    <li className="flex flex-col gap-2 rounded-xl border bg-card p-3 text-sm">
      <div className="flex items-start gap-2">
        <span className="mt-0.5">
          <StatusIcon task={task} />
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{task.name}</p>
          {task.command !== task.name && (
            <p className="truncate font-mono text-[11px] text-muted-foreground select-text" title={task.command}>
              {task.command}
            </p>
          )}
        </div>
        <span className="shrink-0 text-xs text-muted-foreground">
          {STATUS[task.status]}
          {task.exitCode !== null && task.status === "failed" ? ` (${task.exitCode})` : ""}
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
        {task.project && <Badge variant="outline">{projectName(task.project)}</Badge>}
        <span className="tabular-nums" title={new Date(task.startedAt).toLocaleString()}>
          {running ? `up ${duration(now - task.startedAt)}` : `ran ${duration((task.endedAt ?? now) - task.startedAt)}`}
        </span>
        {task.ports.slice(0, MAX_PORTS).map((p) => (
          <PortLink key={p} port={p} forwarding={forwarding} />
        ))}
        {task.ports.length > MAX_PORTS && (
          <span className="text-xs" title={task.ports.slice(MAX_PORTS).map((p) => `:${p}`).join(" ")}>
            +{task.ports.length - MAX_PORTS} more
          </span>
        )}
      </div>

      {task.missingPorts.length > 0 && (
        <p className="text-xs text-destructive">Not listening: {task.missingPorts.map((p) => `:${p}`).join(" ")}</p>
      )}
      {task.problems.map((p) => (
        <p key={p} className="flex items-start gap-1.5 text-xs text-destructive">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span className="min-w-0 break-words">{p}</span>
        </p>
      ))}
      {task.compose ? (
        <ComposeProcesses
          taskId={task.id}
          processes={task.compose}
          running={running}
          forwarding={forwarding}
          onShowLogs={onOpenViewer}
        />
      ) : (
        running &&
        task.processes.length > 0 && (
          <p className="truncate text-[11px] text-muted-foreground" title={task.processes.join(", ")}>
            {task.processes.join(" · ")}
          </p>
        )
      )}

      <div className="flex items-center gap-1.5">
        <Button variant="outline" size="xs" onClick={() => onOpenViewer()} aria-label={`Open ${task.name} log`} title="Open the log">
          <ScrollText data-icon="inline-start" />
          Logs
        </Button>
        {running ? (
          <Button variant="outline" size="xs" disabled={busy} onClick={() => void act(() => stopTask(task.id))}>
            {busy ? <Spinner data-icon="inline-start" /> : <Square data-icon="inline-start" className="fill-current" />}
            Stop
          </Button>
        ) : (
          <Button variant="ghost" size="xs" disabled={busy} onClick={() => void act(() => removeTask(task.id))}>
            <Trash2 data-icon="inline-start" />
            Remove
          </Button>
        )}
      </div>
      {error && <p className="text-xs text-destructive">{error}</p>}
    </li>
  );
}

export const TASKS_WIDTH = { default: 384, min: 320, max: 900 };

interface Props {
  width: number;
  onWidth: (w: number) => void;
  tasks: TaskView[];
  error?: string;
  refresh: () => void;
  onClose: () => void;
  /** The task (and compose process) whose log is open, if any. */
  viewing?: TaskLogView;
  onViewing: (v?: TaskLogView) => void;
}

export interface TaskLogView {
  id: string;
  process?: string;
}

/** Everything Satie is running (or has run) on the backend host. */
export function TasksPanel({ width, onWidth, tasks, error, refresh, onClose, viewing, onViewing: setViewing }: Props) {
  const viewed = tasks.find((t) => t.id === viewing?.id);
  const forwarding = useForwarding();
  const finished = tasks.filter((t) => t.status !== "running");

  return (
    <aside style={{ width }} className="relative flex shrink-0 flex-col border-l bg-background" aria-label="Background tasks">
      <ResizeHandle width={width} onWidth={onWidth} min={TASKS_WIDTH.min} max={TASKS_WIDTH.max} edge="left" reset={TASKS_WIDTH.default} label="Resize background tasks" />
      <header data-tauri-drag-region className="flex h-(--title-bar-height) shrink-0 items-center gap-2 border-b px-4">
        <h2 className="flex-1 text-sm font-semibold">Background tasks</h2>
        {finished.length > 0 && (
          <Button
            variant="ghost"
            size="xs"
            className="text-muted-foreground"
            title="Remove every task that isn't running"
            onClick={() => void Promise.allSettled(finished.map((t) => removeTask(t.id))).then(refresh)}
          >
            <Trash2 data-icon="inline-start" />
            Clear finished
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" aria-label="Close tasks panel" onClick={onClose}>
          <X />
        </Button>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {error && <p className="mb-2 text-xs text-destructive">{error}</p>}
        {tasks.length === 0 && !error && (
          <p className="p-6 text-center text-sm text-muted-foreground">
            No background tasks. Agents start them with Satie's tools.
          </p>
        )}
        <ul className="flex flex-col gap-2">
          {tasks.map((t) => (
            <TaskCard key={t.id} task={t} forwarding={forwarding} onChanged={refresh} onOpenViewer={(process) => setViewing({ id: t.id, process })} />
          ))}
        </ul>
      </div>
      {viewed && <LogViewer task={viewed} process={viewing?.process} onClose={() => setViewing(undefined)} />}
    </aside>
  );
}
