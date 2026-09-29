import { useEffect, useRef, useState } from "react";
import { AlertTriangle, GitBranch, Plus } from "lucide-react";
import { GitInfo, gitInfo } from "@/api";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { NewSession as Session } from "@/session";
import { Choice, Picker } from "./Composer";

/** The menu entry that starts a new branch (in a worktree) instead of picking one. */
const NEW_BRANCH = "\0new";

interface GitState {
  info?: GitInfo;
  loading: boolean;
  error?: string;
}

/** Loads branch info for the session's project folder, defaulting the selection to the current branch. */
export function useGitInfo(session: Session, onChange: (patch: Partial<Session>) => void): GitState {
  const cwd = session.cwd.trim();
  const [state, setState] = useState<GitState>({ loading: false });
  const latest = useRef(0);

  useEffect(() => {
    setState({ loading: false });
    if (!cwd) return;
    const req = ++latest.current;
    setState({ loading: true });
    gitInfo(cwd)
      .then((info) => req === latest.current && setState({ info, loading: false }))
      .catch((e) => req === latest.current && setState({ loading: false, error: String(e.message ?? e) }));
  }, [cwd]);

  useEffect(() => {
    const { info } = state;
    if (info?.isRepo && info.current && !session.branch) onChange({ branch: info.current });
  }, [state.info]);

  return state;
}

/** Branch picker and worktree switch; renders nothing for folders that aren't git repos. */
export function BranchControls({ session, git, onChange }: { session: Session; git: GitState; onChange: (p: Partial<Session>) => void }) {
  const { info, loading } = git;
  if (loading && !info) return <Spinner className="size-3.5 text-muted-foreground" />;
  if (!info?.isRepo) return null;

  const selected = session.branch ?? info.current ?? undefined;
  // A repo with no commits has a current branch that isn't listed yet; keep it selectable.
  const names = info.current && !info.branches.includes(info.current) ? [info.current, ...info.branches] : info.branches;
  const describe = (b: string) => {
    if (session.worktree) return b === info.current ? "Checked out here; the new branch starts from it" : "The new branch starts from it";
    if (b === info.current) return "Checked out in this folder";
    return info.dirty ? "Switches this folder to it (it has uncommitted changes)" : "Switches this folder to it";
  };
  const choices: Choice[] = names.map((b) => ({ value: b, label: b + (b === info.current ? " (current)" : ""), description: describe(b) }));
  if (!session.worktree) {
    choices.push({
      value: NEW_BRANCH,
      label: "New branch",
      description: `From ${selected ?? "HEAD"}, in its own worktree`,
      icon: <Plus className="size-4 text-muted-foreground" />,
      separated: true,
    });
  }

  return (
    <>
      <div className="flex items-center gap-1.5">
        <GitBranch className="size-4 text-muted-foreground" />
        <Picker
          heading={session.worktree ? "Branch from" : "Branch"}
          label="Branch"
          mono
          align="start"
          className="min-w-32"
          choices={choices}
          value={selected ?? null}
          placeholder={info.current ? undefined : "Detached HEAD"}
          onChange={(b) => (b === NEW_BRANCH ? onChange({ worktree: true }) : onChange({ branch: b }))}
        />
        {session.worktree && (
          <Input
            aria-label="New branch name"
            className="h-7 w-44 font-mono text-xs"
            placeholder="branch name (auto)"
            value={session.newBranch ?? ""}
            onChange={(e) => onChange({ newBranch: e.target.value })}
            spellCheck={false}
          />
        )}
      </div>
      <div className="flex items-center gap-2">
        <Switch id="worktree" size="sm" checked={!!session.worktree} onCheckedChange={(w) => onChange({ worktree: w })} />
        <Label htmlFor="worktree" className="text-xs">
          Worktree
        </Label>
      </div>
    </>
  );
}

/** Says what will happen to the repository when the first message is sent. */
export function BranchHint({ session, git }: { session: Session; git: GitState }) {
  const { info, error } = git;
  if (error) return <p className="text-xs text-destructive">{error}</p>;
  if (!info?.isRepo) return null;
  const selected = session.branch ?? info.current ?? undefined;
  if (session.worktree) {
    const name = session.newBranch?.trim();
    return (
      <p className="text-xs text-muted-foreground">
        Creates {name ? <span className="font-mono">{name}</span> : "a branch named after your message"} from {selected ?? "HEAD"} in an isolated worktree, so your
        checkout stays as is.
      </p>
    );
  }
  if (selected !== undefined && selected !== info.current) {
    return (
      <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
        {info.dirty && <AlertTriangle className="size-3.5 text-destructive" />}
        Switches this folder to {selected}
        {info.dirty ? " — it has uncommitted changes, so the switch may fail." : "."}
      </p>
    );
  }
  return null;
}
