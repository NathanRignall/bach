import { useEffect, useState } from "react";
import { Settings2 } from "lucide-react";
import { ConnectionStatus, TaskView, getConnection, inTauri, onConnection, relaunchApp, remoteUrl, restartServer, setConnection } from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { PortsButton, useForwarding } from "./Ports";

const MODES = [
  { value: "local", label: "On this computer" },
  { value: "ssh", label: "On another machine, over SSH" },
];

/** What to do about the usual reasons an SSH connection fails. */
function hint(error: string, host: string) {
  if (/Host key verification failed/i.test(error))
    return `Connect once with “ssh ${host}” in a terminal to trust its host key, then try again.`;
  if (/Permission denied/i.test(error))
    return "SSH couldn't log in without asking. Load your key into the agent (ssh-add) or check ~/.ssh/config.";
  if (/Could not resolve hostname/i.test(error)) return "Check the host name, or add it to ~/.ssh/config.";
  if (/command not found|No such file or directory/i.test(error))
    return `bach-server isn't installed on ${host} (or isn't on its PATH). Set how to run it in the connection settings.`;
  return undefined;
}

const where = (s: ConnectionStatus) => (s.connection.mode === "local" ? "this computer" : s.connection.host);

/** "ws://localhost:3421" -> "Agents local"; anything else -> "Agents on <host>", for the browser (non-Tauri) label. */
function remoteLabel(url: string) {
  try {
    const hostname = new URL(url).hostname;
    return hostname === "localhost" || hostname === "127.0.0.1" ? "Agents local" : `Agents on ${hostname}`;
  } catch {
    return `Agents on ${url}`;
  }
}

/** Where the desktop app's agents run, how that connection is doing, and a way to change it. */
export function ConnectionPicker({ tasks }: { tasks: TaskView[] }) {
  const [status, setStatus] = useState<ConnectionStatus>();
  const [open, setOpen] = useState(false);
  const forwarding = useForwarding();
  useEffect(() => {
    if (!inTauri) return;
    void getConnection().then(setStatus);
    return onConnection(setStatus);
  }, []);

  if (!inTauri)
    return (
      <div className="flex items-center gap-1 pl-1.5 text-xs">
        <span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden>
          <span className="size-2 rounded-full bg-sky-500" />
        </span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground" title={remoteUrl ?? undefined}>
          {remoteLabel(remoteUrl ?? "")}
        </span>
      </div>
    );
  if (!status) return null;

  const dot = { connected: "bg-emerald-500", connecting: "animate-pulse bg-amber-500", disconnected: "bg-destructive" }[status.state];
  const host = status.connection.mode === "ssh" ? status.connection.host : "";
  return (
    <div className="flex flex-col gap-1.5 text-xs">
      {/* Lined up with the buttons above: their icon sits in a 14px box after 6px of padding. */}
      <div className="flex items-center gap-1 pl-1.5">
        <span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden>
          <span className={cn("size-2 rounded-full", dot)} />
        </span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground" title={status.version ? `bach-server ${status.version}` : undefined}>
          {status.state === "connecting" ? `Connecting to ${where(status)}…` : `Agents on ${where(status)}`}
        </span>
        <PortsButton forwarding={forwarding} tasks={tasks} />
        <Button variant="ghost" size="icon-xs" aria-label="Connection settings" title="Where agents run" onClick={() => setOpen(true)}>
          <Settings2 />
        </Button>
      </div>
      {status.state === "disconnected" && status.error && (
        <div className="flex flex-col gap-1 rounded-md bg-destructive/10 px-2 py-1.5 text-destructive select-text" role="alert">
          <p className="line-clamp-4 font-mono text-[11px] break-words whitespace-pre-wrap">{status.error}</p>
          {hint(status.error, host) && <p>{hint(status.error, host)}</p>}
          {status.retrying && <p className="text-muted-foreground">Trying again…</p>}
          {status.incompatible && <Relaunch ssh={status.connection.mode === "ssh"} />}
        </div>
      )}
      {open && <ConnectionDialog status={status} onClose={() => setOpen(false)} />}
    </div>
  );
}

/** The app and the server are different builds: restart whichever is stale. */
function Relaunch({ ssh }: { ssh: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  async function restart() {
    setBusy(true);
    setError(undefined);
    try {
      await restartServer();
      // Everything on screen came from the old server.
      location.reload();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }
  return (
    <div className="flex flex-col gap-1.5 text-foreground">
      <p>Restart the older one. Restarting the server ends agent turns in progress; background tasks keep running.</p>
      <div className="flex gap-1.5">
        {ssh && (
          <Button size="xs" disabled={busy} onClick={() => void restart()}>
            {busy ? "Restarting…" : "Restart server"}
          </Button>
        )}
        <Button size="xs" variant="outline" disabled={busy} onClick={() => void relaunchApp()}>
          Relaunch app
        </Button>
      </div>
      {error && <p className="font-mono text-[11px] break-words whitespace-pre-wrap text-destructive">{error}</p>}
    </div>
  );
}

// Starting agents through a wrapper needs a bach-server built with the `agent-wrapper` feature.
const WRAPPER = !!import.meta.env.VITE_BACH_AGENT_WRAPPER;

function ConnectionDialog({ status, onClose }: { status: ConnectionStatus; onClose: () => void }) {
  const c = status.connection;
  const [mode, setMode] = useState<string>(c.mode);
  const [host, setHost] = useState(c.mode === "ssh" ? c.host : "");
  const [command, setCommand] = useState(c.mode === "ssh" ? c.command : "bach-server");
  const [wrapper, setWrapper] = useState(c.mode === "ssh" ? c.wrapper : "");
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);

  async function apply() {
    setSaving(true);
    try {
      await setConnection(mode === "local" ? { mode: "local" } : { mode: "ssh", host: host.trim(), command: command.trim(), wrapper: WRAPPER ? wrapper.trim() : "" });
      // Everything on screen belongs to the old backend.
      location.reload();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Where agents run</DialogTitle>
          <DialogDescription>
            Over SSH, the app runs <code>bach-server attach</code> on that machine with your own ssh setup (keys, agent,
            ~/.ssh/config). The server starts there if it isn't running, and keeps agents and background tasks going when you
            disconnect.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <Select items={MODES} value={mode} onValueChange={(v) => v && setMode(v)}>
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {MODES.map((m) => (
                <SelectItem key={m.value} value={m.value}>
                  {m.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {mode === "ssh" && (
            <>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="ssh-host">Host</Label>
                <Input id="ssh-host" value={host} placeholder="user@host" spellCheck={false} autoFocus onChange={(e) => setHost(e.target.value)} />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="ssh-command">bach-server on that machine</Label>
                <Input id="ssh-command" value={command} spellCheck={false} className="font-mono" onChange={(e) => setCommand(e.target.value)} />
                <p className="text-xs text-muted-foreground">
                  A command on its PATH, or a full path such as <code>~/dev/bach/target/release/bach-server</code>.
                </p>
              </div>
              {WRAPPER && (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="ssh-wrapper">Start agents through</Label>
                  <Input
                    id="ssh-wrapper"
                    value={wrapper}
                    placeholder="nothing"
                    spellCheck={false}
                    className="font-mono"
                    onChange={(e) => setWrapper(e.target.value)}
                  />
                  <p className="text-xs text-muted-foreground">
                    A command put before the agent's: <code>sandbox</code> runs <code>sandbox claude</code>. Leave it empty
                    to run agents directly.
                  </p>
                </div>
              )}
            </>
          )}
          {error && <p className="text-sm text-destructive">{error}</p>}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={() => void apply()} disabled={saving || (mode === "ssh" && !host.trim())}>
            Connect
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
