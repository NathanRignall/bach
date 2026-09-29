import { useState } from "react";
import { Play, RotateCw, ScrollText, Square } from "lucide-react";
import { ComposeProcess, Forwarding, ProcessAction, taskProcess } from "@/api";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { PortLink } from "./Ports";

/** On its way up or down, or waiting for its dependencies. */
const MOVING = new Set(["Pending", "Launching", "Restarting", "Terminating", "Scheduled"]);

export function processFailed(p: ComposeProcess) {
  return (!p.running && p.exitCode !== 0 && !MOVING.has(p.status)) || p.status === "Error";
}

/** How a process is doing, in a word or two. */
export function processState(p: ComposeProcess) {
  if (p.running && p.ready === false) return "Not ready";
  if (!p.running && !MOVING.has(p.status) && p.status !== "Disabled" && p.exitCode !== 0) return `${p.status} (${p.exitCode})`;
  return p.status;
}

export function ProcessDot({ process: p, className }: { process: ComposeProcess; className?: string }) {
  const tone = processFailed(p)
    ? "bg-destructive"
    : MOVING.has(p.status) || (p.running && p.ready === false)
      ? "bg-amber-500"
      : p.running
        ? "bg-emerald-500"
        : "bg-muted-foreground/40";
  return <span aria-hidden className={cn("inline-block size-2 shrink-0 rounded-full", tone, className)} />;
}

/** "All" plus one tab per process, for picking whose output to show. */
export function ProcessTabs({
  processes,
  value,
  onChange,
  className,
}: {
  processes: ComposeProcess[];
  value?: string;
  onChange: (process?: string) => void;
  className?: string;
}) {
  const tab = (label: React.ReactNode, key: string | undefined, p?: ComposeProcess) => (
    <button
      key={key ?? ""}
      type="button"
      role="tab"
      aria-selected={value === key}
      onClick={() => onChange(key)}
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-0.5 font-mono text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground",
        value === key && "bg-accent text-foreground",
      )}
    >
      {p && <ProcessDot process={p} />}
      {label}
    </button>
  );
  return (
    <div role="tablist" aria-label="Whose output" className={cn("flex gap-1 overflow-x-auto", className)}>
      {tab("All", undefined)}
      {processes.map((p) => tab(p.name, p.name, p))}
    </div>
  );
}

/** The processes of a process-compose task: state, ports, and start/stop/restart while it runs. */
export function ComposeProcesses({
  taskId,
  processes,
  running,
  forwarding,
  onShowLogs,
}: {
  taskId: string;
  processes: ComposeProcess[];
  /** Whether the task (process-compose itself) is running. */
  running: boolean;
  forwarding: Forwarding | null;
  onShowLogs: (process: string) => void;
}) {
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const act = async (p: ComposeProcess, action: ProcessAction) => {
    setBusy(p.name);
    try {
      await taskProcess(taskId, p.name, action);
      setError(undefined);
    } catch (e) {
      setError(String((e as Error).message ?? e));
    }
    setBusy(undefined);
  };

  if (processes.length === 0) {
    return <p className="text-[11px] text-muted-foreground">{running ? "Waiting for process-compose…" : "No processes."}</p>;
  }
  return (
    <div className="flex flex-col">
      <ul className="flex flex-col divide-y rounded-lg border" aria-label="Processes">
        {processes.map((p) => (
          <li key={p.name} className="flex min-h-8 items-center gap-2 px-2 py-1 text-xs">
            <ProcessDot process={p} />
            <span className="min-w-0 truncate font-mono" title={p.name}>
              {p.name}
            </span>
            <span className={cn("shrink-0 text-[11px] text-muted-foreground", processFailed(p) && "text-destructive")}>
              {processState(p)}
              {p.restarts > 0 && <span title={`Restarted ${p.restarts} time${p.restarts === 1 ? "" : "s"}`}> ↻{p.restarts}</span>}
            </span>
            <span className="flex min-w-0 flex-1 flex-wrap justify-end gap-1">
              {p.ports.slice(0, 3).map((port) => (
                <PortLink key={port} port={port} forwarding={forwarding} className="text-[11px]" />
              ))}
            </span>
            <span className="flex shrink-0 items-center">
              <Button variant="ghost" size="icon-xs" aria-label={`Show ${p.name} output`} title="Output" onClick={() => onShowLogs(p.name)}>
                <ScrollText />
              </Button>
              {running &&
                (busy === p.name ? (
                  <Spinner className="mx-1 size-3.5 text-muted-foreground" aria-label={`Working on ${p.name}`} />
                ) : p.running ? (
                  <>
                    <Button variant="ghost" size="icon-xs" aria-label={`Restart ${p.name}`} title="Restart" onClick={() => void act(p, "restart")}>
                      <RotateCw />
                    </Button>
                    <Button variant="ghost" size="icon-xs" aria-label={`Stop ${p.name}`} title="Stop" onClick={() => void act(p, "stop")}>
                      <Square className="fill-current" />
                    </Button>
                  </>
                ) : (
                  <Button variant="ghost" size="icon-xs" aria-label={`Start ${p.name}`} title="Start" onClick={() => void act(p, "start")}>
                    <Play />
                  </Button>
                ))}
            </span>
          </li>
        ))}
      </ul>
      {error && <p className="mt-1 text-xs text-destructive">{error}</p>}
    </div>
  );
}
