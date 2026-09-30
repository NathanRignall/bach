import { useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, GitCommitHorizontal, UploadCloud, X } from "lucide-react";
import { BranchStatus, CommitInfo, gitLog } from "@/api";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

const errorText = (e: unknown) => String((e as Error)?.message ?? e);

/** Git's complaint as the backend words it: a summary line, then git's own output. */
export function GitError({ message, onDismiss, className }: { message: string; onDismiss?: () => void; className?: string }) {
  const [summary, ...rest] = message.split("\n\n");
  const detail = rest.join("\n\n");
  return (
    <div role="alert" className={cn("flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive", className)}>
      <div className="min-w-0 flex-1">
        <p className="font-medium">{summary}</p>
        {detail && <pre className="mt-1 max-h-32 overflow-auto font-mono text-[11px] whitespace-pre-wrap opacity-80 select-text">{detail}</pre>}
      </div>
      {onDismiss && (
        <Button size="icon-xs" variant="ghost" onClick={onDismiss} title="Dismiss" aria-label="Dismiss">
          <X />
        </Button>
      )}
    </div>
  );
}

/** Where the branch stands against its remote, and a button to push it. */
export function PushControl({ status, pushing, onPush }: { status?: BranchStatus; pushing: boolean; onPush: () => void }) {
  if (!status?.branch) return null;
  const { upstream, ahead, behind, hasRemote } = status;
  const nothing = !!upstream && ahead === 0;
  const where = upstream ? (
    <span className="font-mono" title={`Pushes to ${upstream}`}>
      {upstream}
    </span>
  ) : hasRemote ? (
    <span title="The branch isn't on the remote yet">not published</span>
  ) : (
    <span>no remote</span>
  );
  return (
    <div className="flex items-center gap-2">
      <span className="flex items-center gap-1.5">
        {where}
        {upstream && (
          <span className="flex items-center gap-1.5 font-mono tabular-nums">
            <span title={`${ahead} ${ahead === 1 ? "commit" : "commits"} to push`} className={cn("flex items-center", ahead > 0 && "text-foreground")}>
              <ArrowUp className="size-3" />
              {ahead}
            </span>
            <span title={`${behind} ${behind === 1 ? "commit" : "commits"} on the remote to bring in (as of the last fetch)`} className={cn("flex items-center", behind > 0 && "text-amber-600 dark:text-amber-400")}>
              <ArrowDown className="size-3" />
              {behind}
            </span>
          </span>
        )}
        {!upstream && ahead > 0 && <span className="font-mono tabular-nums">{ahead} to push</span>}
      </span>
      <Button
        size="xs"
        variant={nothing ? "outline" : "default"}
        disabled={pushing || nothing}
        onClick={onPush}
        title={nothing ? "Nothing to push" : upstream ? `Push to ${upstream}` : "Push the branch and set it to track the remote's copy"}
      >
        {pushing ? <Spinner /> : <UploadCloud />} {upstream ? "Push" : "Publish branch"}
      </Button>
    </div>
  );
}

/** The message box and button that commit what is staged. */
export function CommitBox({ staged, onCommit }: { staged: number; onCommit: (message: string) => Promise<void> }) {
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const can = staged > 0 && !!message.trim() && !busy;
  const commit = async () => {
    if (!can) return;
    setBusy(true);
    setError(undefined);
    try {
      await onCommit(message);
      setMessage("");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex shrink-0 flex-col gap-2 border-b p-2 text-xs" role="region" aria-label="Commit">
      {error && <GitError message={error} onDismiss={() => setError(undefined)} />}
      <Textarea
        aria-label="Commit message"
        rows={2}
        className="min-h-14 max-h-40 py-1.5 font-mono text-xs md:text-xs"
        placeholder={staged ? "Commit message" : "Stage changes to commit them"}
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={(e) => (e.metaKey || e.ctrlKey) && e.key === "Enter" && (e.preventDefault(), void commit())}
        spellCheck
      />
      <Button size="sm" disabled={!can} onClick={() => void commit()} title="Commit the staged changes (Ctrl/⌘ + Enter)">
        {busy ? <Spinner /> : <GitCommitHorizontal />} Commit {staged > 0 && `${staged} ${staged === 1 ? "file" : "files"}`}
      </Button>
    </div>
  );
}

const PAGE = 30;

export interface CommitsState {
  commits: CommitInfo[];
  loading: boolean;
  error?: string;
  hasMore: boolean;
  /** Commits from before the branch left its base are included. */
  older: boolean;
  more: () => void;
  showOlder: () => void;
}

/**
 * The commits of the checkout: the branch's own since it left `baseBranch` (all of them
 * without one), a page at a time, and on request the ones before that. Starts over when the
 * checkout or its HEAD changes (`head`).
 */
export function useCommits(path: string, baseBranch: string | undefined, head: string | undefined, enabled: boolean): CommitsState {
  const [state, setState] = useState<Omit<CommitsState, "more" | "showOlder">>({ commits: [], loading: false, hasMore: false, older: false });
  const latest = useRef(0);
  const key = `${path}\0${baseBranch}`;
  const lastKey = useRef(key);

  const load = (skip: number, older: boolean) => {
    const req = ++latest.current;
    setState((s) => ({ ...s, loading: true, error: undefined }));
    gitLog({ path, baseBranch, older: older || !baseBranch, skip, limit: PAGE })
      .then((log) => req === latest.current && setState((s) => ({ commits: skip ? [...s.commits, ...log.commits] : log.commits, loading: false, hasMore: log.hasMore, older })))
      .catch((e) => req === latest.current && setState((s) => ({ ...s, loading: false, error: errorText(e) })));
  };

  useEffect(() => {
    if (!enabled || !path) return;
    // Another checkout starts over from the branch's own commits; a new commit keeps the paging mode.
    const changed = lastKey.current !== key;
    lastKey.current = key;
    if (changed) setState({ commits: [], loading: false, hasMore: false, older: false });
    load(0, changed ? false : state.older);
  }, [enabled, key, head]);

  return {
    ...state,
    more: () => load(state.commits.length, state.older),
    showOlder: () => load(state.commits.length, true),
  };
}

const when = (time: number) => {
  const secs = Math.max(0, Date.now() / 1000 - time);
  const units: [number, string][] = [[60, "s"], [60, "m"], [24, "h"], [30, "d"], [12, "mo"]];
  let n = secs;
  for (const [per, unit] of units) {
    if (n < per) return `${Math.floor(n)}${unit} ago`;
    n /= per;
  }
  return `${Math.floor(n)}y ago`;
};

/** The commits, newest first; picking one shows its diff. */
export function CommitList({ state, base, selected, onSelect }: { state: CommitsState; base?: string; selected?: string; onSelect: (c: CommitInfo) => void }) {
  const { commits, loading, error, hasMore, older } = state;
  const firstOlder = commits.findIndex((c) => !c.onBranch);
  return (
    <div className="text-xs" role="listbox" aria-label="Commits">
      {error && <GitError message={error} className="m-2" />}
      {!error && !loading && !commits.length && <p className="px-3 py-3 text-muted-foreground">No commits yet.</p>}
      {commits.map((c, i) => (
        <div key={c.sha}>
          {i === firstOlder && base && <p className="border-y bg-muted/50 px-3 py-1 text-[11px] text-muted-foreground">Before the branch left {base}</p>}
          <button
            type="button"
            role="option"
            aria-selected={c.sha === selected}
            onClick={() => onSelect(c)}
            title={`${c.sha}\n${c.subject}`}
            className={cn(
              "flex w-full flex-col gap-0.5 border-b border-border/50 px-3 py-1.5 text-left outline-none hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset",
              c.sha === selected && "bg-accent text-accent-foreground hover:bg-accent",
              !c.onBranch && "opacity-70",
            )}
          >
            <span className="truncate font-medium">{c.subject || "(no message)"}</span>
            <span className="flex items-center gap-2 text-[11px] text-muted-foreground">
              <span className="font-mono">{c.short}</span>
              <span className="min-w-0 flex-1 truncate">{c.author}</span>
              <span className="shrink-0">{when(c.time)}</span>
            </span>
          </button>
        </div>
      ))}
      <div className="flex flex-col gap-1 p-2">
        {loading && (
          <p className="flex items-center gap-2 px-1 text-muted-foreground">
            <Spinner /> Loading commits…
          </p>
        )}
        {!loading && hasMore && (
          <Button size="xs" variant="outline" onClick={state.more}>
            Show more
          </Button>
        )}
        {!loading && !hasMore && !older && base && (
          <Button size="xs" variant="outline" onClick={state.showOlder}>
            Show commits from before it left {base}
          </Button>
        )}
      </div>
    </div>
  );
}
