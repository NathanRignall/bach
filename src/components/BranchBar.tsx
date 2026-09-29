import { useEffect, useRef, useState } from "react";
import { AlertTriangle, GitBranch, GitFork } from "lucide-react";
import { GitInfo, gitInfo } from "@/api";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Session, isStarted } from "@/session";

interface Props {
  session: Session;
  onChange: (patch: Partial<Session>) => void;
}

/** Branch picker and worktree toggle for the session's project folder. */
export function BranchBar({ session, onChange }: Props) {
  const started = isStarted(session);
  const cwd = session.cwd.trim();
  const [info, setInfo] = useState<GitInfo>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const latest = useRef(0);

  useEffect(() => {
    setInfo(undefined);
    setError(undefined);
    if (started || !cwd) return;
    const req = ++latest.current;
    setLoading(true);
    gitInfo(cwd)
      .then((i) => req === latest.current && setInfo(i))
      .catch((e) => req === latest.current && setError(String(e.message ?? e)))
      .finally(() => req === latest.current && setLoading(false));
  }, [cwd, started]);

  // Default the selection to the branch that's checked out.
  useEffect(() => {
    if (!started && info?.isRepo && info.current && !session.branch) onChange({ branch: info.current });
  }, [info]);

  if (started) {
    return session.gitBranch ? (
      <div className="flex items-center gap-2 text-xs text-muted-foreground" title={session.workdir}>
        {session.worktree ? <GitFork className="size-3.5" /> : <GitBranch className="size-3.5" />}
        <span className="font-mono">{session.gitBranch}</span>
        {session.worktree && <span>· worktree</span>}
      </div>
    ) : null;
  }
  if (loading && !info) return <Spinner className="size-3.5 text-muted-foreground" />;
  if (error) return <p className="text-xs text-destructive">{error}</p>;
  if (!info?.isRepo) return null;

  const selected = session.branch ?? info.current ?? undefined;
  const items = info.branches.map((b) => ({ value: b, label: b + (b === info.current ? " (current)" : "") }));
  const switching = !session.worktree && selected !== undefined && selected !== info.current;

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5">
      <div className="flex items-center gap-2">
        <GitBranch className="size-4 text-muted-foreground" />
        <Select items={items} value={selected ?? null} onValueChange={(b) => b && onChange({ branch: b })}>
          <SelectTrigger size="sm" aria-label="Branch" className="min-w-40 font-mono text-xs">
            <SelectValue placeholder={info.current ? undefined : "Detached HEAD"} />
          </SelectTrigger>
          <SelectContent>
            {items.map((i) => (
              <SelectItem key={i.value} value={i.value} className="font-mono text-xs">
                {i.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="flex items-center gap-2">
        <Switch id="worktree" size="sm" checked={!!session.worktree} onCheckedChange={(w) => onChange({ worktree: w })} />
        <Label htmlFor="worktree" className="text-xs">
          Worktree
        </Label>
      </div>

      <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
        {session.worktree ? (
          <>Creates a new branch from {selected ?? "HEAD"} in an isolated worktree, so your checkout stays as is.</>
        ) : switching ? (
          <>
            {info.dirty ? <AlertTriangle className="size-3.5 text-destructive" /> : null}
            Switches this folder to {selected}
            {info.dirty ? " — it has uncommitted changes, so the switch may fail." : "."}
          </>
        ) : null}
      </p>
    </div>
  );
}
