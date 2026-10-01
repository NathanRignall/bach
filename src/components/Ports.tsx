import { useEffect, useState } from "react";
import { ArrowRight, Cable, ExternalLink, X } from "lucide-react";
import { Forwarding, TaskView, forwardPort, getForwarding, inTauri, onForwarding, openPort, setAutoForward, stopForward, taskHost } from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { previewPort } from "@/lib/preview";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

/** The desktop app's port forwarding, kept current. Null outside the app. */
export function useForwarding() {
  const [state, setState] = useState<Forwarding | null>(null);
  useEffect(() => {
    if (!inTauri) return;
    void getForwarding().then(setState, () => {});
    return onForwarding(setState);
  }, []);
  return state;
}

const chip = "rounded-md bg-primary/10 px-1.5 py-0.5 font-mono text-primary";

/**
 * A port on the agents' machine. A click shows it in Bach's browser panel (forwarding it to this
 * computer first when agents run elsewhere); ⌘/Ctrl-click opens it in the default browser, or in a
 * browser tab when Bach itself runs in one.
 */
export function PortLink({ port, forwarding, className }: { port: number; forwarding: Forwarding | null; className?: string }) {
  const [error, setError] = useState<string>();
  if (!inTauri) {
    const host = taskHost();
    return host ? (
      <a
        href={`http://${host}:${port}`}
        target="_blank"
        rel="noopener noreferrer"
        title={`Show port ${port} here (⌘/Ctrl-click: new tab)`}
        onClick={(e) => {
          if (e.metaKey || e.ctrlKey) return;
          e.preventDefault();
          void previewPort(port);
        }}
        className={cn(chip, "hover:underline", className)}
      >
        :{port}
      </a>
    ) : (
      <span className={cn(chip, className)}>:{port}</span>
    );
  }
  const fwd = forwarding?.forwards.find((f) => f.remote === port);
  const title = error
    ? error
    : fwd
      ? `Show localhost:${fwd.local} (forwarded from port ${port}); ⌘/Ctrl-click: default browser`
      : forwarding?.available
        ? `Forward port ${port} and show it; ⌘/Ctrl-click: default browser`
        : `Show localhost:${port}; ⌘/Ctrl-click: default browser`;
  return (
    <button
      type="button"
      title={title}
      onClick={(e) => {
        setError(undefined);
        (e.metaKey || e.ctrlKey ? openPort(port) : previewPort(port)).catch((err) => setError(String(err)));
      }}
      className={cn(chip, "inline-flex items-center gap-1 hover:underline", error && "bg-destructive/10 text-destructive", className)}
    >
      :{port}
      {fwd && fwd.local !== port && (
        <>
          <ArrowRight className="size-3" aria-label="forwarded to" />
          {fwd.local}
        </>
      )}
      {fwd && <span className="size-1.5 rounded-full bg-emerald-500" aria-label="forwarded" />}
    </button>
  );
}

/** What listens on a port, as far as the background tasks say: the task, and its process for a compose project. */
function listener(port: number, tasks: TaskView[]) {
  for (const t of tasks) {
    if (t.status !== "running") continue;
    const proc = t.compose?.find((p) => p.ports.includes(port));
    if (proc) return `${t.name} · ${proc.name}`;
    if (t.ports.includes(port) || t.upPorts.includes(port)) return t.name;
  }
  return undefined;
}

/** The forwards to this computer, as a small icon that opens the list (only over SSH). */
export function PortsButton({ forwarding, tasks }: { forwarding: Forwarding | null; tasks: TaskView[] }) {
  if (!forwarding?.available) return null;
  const n = forwarding.forwards.length;
  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className={cn("relative", forwarding.error && "text-destructive")}
            aria-label={`Forwarded ports${n ? ` (${n})` : ""}`}
            title={n ? `${n} port${n === 1 ? "" : "s"} forwarded to this computer` : "Forwarded ports"}
          />
        }
      >
        <Cable />
        {n > 0 && (
          <span className="absolute -top-0.5 -right-0.5 min-w-3.5 rounded-full bg-primary px-1 text-[9px] leading-3.5 font-medium text-primary-foreground tabular-nums">
            {n}
          </span>
        )}
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-96">
        <ForwardedPorts forwarding={forwarding} tasks={tasks} />
      </PopoverContent>
    </Popover>
  );
}

/** The forwards to this computer, when agents run on another machine. */
export function ForwardedPorts({ forwarding, tasks }: { forwarding: Forwarding; tasks: TaskView[] }) {
  const [port, setPort] = useState("");
  const [error, setError] = useState<string>();

  const add = async () => {
    const n = Number(port);
    if (!Number.isInteger(n) || n < 1 || n > 65535) return setError("Enter a port number.");
    setError(undefined);
    try {
      await forwardPort(n);
      setPort("");
    } catch (e) {
      setError(String(e));
    }
  };
  const problem = error ?? forwarding.error;

  return (
    <section className="flex flex-col gap-2" aria-label="Forwarded ports">
      <div className="flex items-center gap-2">
        <h3 className="flex-1 text-xs font-medium text-muted-foreground">Forwarded ports</h3>
        <Label htmlFor="auto-forward" className="text-xs font-normal text-muted-foreground">
          Auto forward
        </Label>
        <Switch id="auto-forward" size="sm" checked={forwarding.auto} onCheckedChange={(v) => void setAutoForward(v)} />
      </div>
      {forwarding.forwards.length > 0 ? (
        <ul className="flex flex-col gap-1">
          {forwarding.forwards.map((f) => {
            const who = listener(f.remote, tasks);
            return (
              <li key={f.remote} className="flex items-center gap-2 text-xs">
                <span className="font-mono">:{f.remote}</span>
                <ArrowRight className="size-3 text-muted-foreground" />
                <button type="button" className="font-mono text-primary hover:underline" onClick={() => void openPort(f.remote)}>
                  localhost:{f.local}
                </button>
                <span className="min-w-0 flex-1 truncate text-muted-foreground" title={who ?? (f.auto ? "Forwarded for a task that has since stopped" : undefined)}>
                  {who ?? (f.auto ? "task" : "")}
                </span>
                <Button variant="ghost" size="icon-xs" title="Open in browser" aria-label={`Open port ${f.remote}`} onClick={() => void openPort(f.remote)}>
                  <ExternalLink />
                </Button>
                <Button variant="ghost" size="icon-xs" title="Stop forwarding" aria-label={`Stop forwarding port ${f.remote}`} onClick={() => void stopForward(f.remote)}>
                  <X />
                </Button>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-xs text-muted-foreground">No ports forwarded.</p>
      )}
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <Input value={port} onChange={(e) => setPort(e.target.value)} inputMode="numeric" placeholder="Forward a port…" className="h-7 font-mono text-xs" />
        <Button type="submit" size="sm" variant="outline" disabled={!port.trim()}>
          Forward
        </Button>
      </form>
      {problem && <p className="text-xs text-destructive select-text">{problem}</p>}
    </section>
  );
}
