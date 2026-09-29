import { useEffect, useState } from "react";
import { ArrowRight, ExternalLink, X } from "lucide-react";
import { Forwarding, forwardPort, getForwarding, inTauri, onForwarding, openPort, setAutoForward, stopForward, taskHost } from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
 * A port on the agents' machine. In the app a click opens it in the browser (forwarding it to
 * this computer first when agents run elsewhere); in a browser it links to the backend's host.
 */
export function PortLink({ port, forwarding, className }: { port: number; forwarding: Forwarding | null; className?: string }) {
  const [error, setError] = useState<string>();
  if (!inTauri) {
    const host = taskHost();
    return host ? (
      <a href={`http://${host}:${port}`} target="_blank" rel="noopener noreferrer" className={cn(chip, "hover:underline", className)}>
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
      ? `Open localhost:${fwd.local} (forwarded from port ${port})`
      : forwarding?.available
        ? `Forward port ${port} and open it`
        : `Open localhost:${port}`;
  return (
    <button
      type="button"
      title={title}
      onClick={() => {
        setError(undefined);
        openPort(port).catch((e) => setError(String(e)));
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

/** The forwards to this computer, when agents run on another machine. */
export function ForwardedPorts({ forwarding }: { forwarding: Forwarding | null }) {
  const [port, setPort] = useState("");
  const [error, setError] = useState<string>();
  if (!forwarding?.available) return null;

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
    <section className="flex flex-col gap-2 border-b px-4 py-3" aria-label="Forwarded ports">
      <div className="flex items-center gap-2">
        <h3 className="flex-1 text-xs font-medium text-muted-foreground">Forwarded ports</h3>
        <Label htmlFor="auto-forward" className="text-xs font-normal text-muted-foreground">
          Task ports automatically
        </Label>
        <Switch id="auto-forward" size="sm" checked={forwarding.auto} onCheckedChange={(v) => void setAutoForward(v)} />
      </div>
      {forwarding.forwards.length > 0 && (
        <ul className="flex flex-col gap-1">
          {forwarding.forwards.map((f) => (
            <li key={f.remote} className="flex items-center gap-2 text-xs">
              <span className="font-mono">:{f.remote}</span>
              <ArrowRight className="size-3 text-muted-foreground" />
              <button type="button" className="font-mono text-primary hover:underline" onClick={() => void openPort(f.remote)}>
                localhost:{f.local}
              </button>
              {f.auto && <span className="text-muted-foreground">task</span>}
              <span className="flex-1" />
              <Button variant="ghost" size="icon-xs" title="Open in browser" aria-label={`Open port ${f.remote}`} onClick={() => void openPort(f.remote)}>
                <ExternalLink />
              </Button>
              <Button variant="ghost" size="icon-xs" title="Stop forwarding" aria-label={`Stop forwarding port ${f.remote}`} onClick={() => void stopForward(f.remote)}>
                <X />
              </Button>
            </li>
          ))}
        </ul>
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
