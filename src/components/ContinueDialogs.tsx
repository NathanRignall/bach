import { useEffect, useState } from "react";
import { GitFork, Handshake } from "lucide-react";
import { AgentInfo, AgentKind, ApiError, Session, forkSession, handoffSession, handoffSummary } from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { AgentDot } from "./AgentBadge";

const agentName = (agents: AgentInfo[], kind: AgentKind) => agents.find((a) => a.kind === kind)?.name ?? kind;

/** Fork from a message: the message can be changed before it is sent to the new session. */
export function ForkDialog({
  session,
  seq,
  text,
  onClose,
  onStarted,
}: {
  session: Session;
  seq: number;
  text: string;
  onClose: () => void;
  onStarted: (s: Session) => void;
}) {
  const [prompt, setPrompt] = useState(text);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function start() {
    setBusy(true);
    setError(undefined);
    try {
      onStarted(await forkSession(session.id, seq, prompt));
    } catch (e) {
      setError(ApiError.from(e).message);
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-3 sm:max-w-xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <GitFork className="size-4" />
            Fork from this message
          </DialogTitle>
          <DialogDescription>
            A new session with the conversation up to here, in a new worktree with the files as they were before this message was sent. “{session.title}” stays as it is.
          </DialogDescription>
        </DialogHeader>
        <Textarea
          autoFocus
          aria-label="Message to send in the fork"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          className="max-h-[40vh] min-h-24 overflow-y-auto"
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && prompt.trim() && !busy) void start();
          }}
        />
        <p className="text-xs text-muted-foreground">Change the message to try another approach. Uncommitted and untracked files are restored; ignored ones (build output, dependencies) are not.</p>
        {error && <p className="text-xs text-destructive" role="alert">{error}</p>}
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void start()} disabled={busy || !prompt.trim()}>
            {busy ? <Spinner data-icon="inline-start" /> : <GitFork data-icon="inline-start" />}
            Start fork
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Hand a session's task to another agent: a draft summary to edit, sent as the new session's first message. */
export function HandoffDialog({
  session,
  agents,
  onClose,
  onStarted,
}: {
  session: Session;
  agents: AgentInfo[];
  onClose: () => void;
  onStarted: (s: Session) => void;
}) {
  const installed = agents.filter((a) => a.installed);
  const others = installed.filter((a) => a.kind !== session.agent);
  const [agent, setAgent] = useState<AgentKind | undefined>(others[0]?.kind);
  const [summary, setSummary] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let current = true;
    handoffSummary(session.id).then(
      (s) => current && setSummary(s),
      (e) => current && setError(ApiError.from(e).message),
    );
    return () => void (current = false);
  }, [session.id]);

  async function start() {
    if (!agent || !summary) return;
    setBusy(true);
    setError(undefined);
    try {
      // The mode is the old agent's words; it is kept when the new agent has one by that name.
      const modes = agents.find((a) => a.kind === agent)?.permissionModes ?? [];
      const permissionMode = modes.find((m) => m.id === session.permissionMode)?.id;
      onStarted(await handoffSession({ sessionId: session.id, agent, prompt: summary, permissionMode }));
    } catch (e) {
      setError(ApiError.from(e).message);
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-3 sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Handshake className="size-4" />
            Hand off to another agent
          </DialogTitle>
          <DialogDescription>
            A new session that continues this task in the same worktree ({session.gitBranch ?? session.workdir ?? session.cwd}). This message is what it starts from.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap items-center gap-2" role="radiogroup" aria-label="Agent to hand off to">
          {installed.map((a) => (
            <button
              key={a.kind}
              type="button"
              role="radio"
              aria-checked={agent === a.kind}
              onClick={() => setAgent(a.kind)}
              className={cn(
                "flex items-center gap-2 rounded-lg border px-3 py-1.5 text-sm",
                agent === a.kind ? "border-primary bg-primary/10" : "hover:bg-accent",
              )}
            >
              <AgentDot kind={a.kind} />
              {a.name}
              {a.kind === session.agent && <span className="text-xs text-muted-foreground">(current)</span>}
            </button>
          ))}
        </div>
        {summary === undefined && !error && (
          <div className="flex h-40 items-center justify-center">
            <Spinner className="size-5 text-muted-foreground" />
          </div>
        )}
        {summary !== undefined && (
          <Textarea
            aria-label="Summary to send"
            value={summary}
            onChange={(e) => setSummary(e.target.value)}
            className="max-h-[50vh] min-h-56 overflow-y-auto font-mono text-xs md:text-xs"
          />
        )}
        {summary !== undefined && (
          <p className="text-xs text-muted-foreground">
            Written from the transcript and the worktree's changes. Edit it before it goes to {agent ? agentName(agents, agent) : "the new agent"}.
          </p>
        )}
        {error && <p className="text-xs text-destructive" role="alert">{error}</p>}
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void start()} disabled={busy || !agent || !summary?.trim()}>
            {busy ? <Spinner data-icon="inline-start" /> : <Handshake data-icon="inline-start" />}
            Hand off{agent ? ` to ${agentName(agents, agent)}` : ""}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
