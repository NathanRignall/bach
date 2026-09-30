import { Fragment, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FileDiff as FileDiffIcon, MessageSquarePlus, RefreshCw, Send } from "lucide-react";
import { DiffHunk, FileDiff, FileStatus, GitDiff, Session, gitDiff, onReconnect } from "@/api";
import { FileTree } from "@/components/FileTree";
import { CommentBox, DraftComment, SelectionActions } from "@/components/ReviewComments";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { type Token, renderLine, useHighlight } from "@/lib/highlight";
import { type Anchor, type LineSelection, type Review, anchorEnd, anchorOf, explainMessage, hunkSelection, isSelected, rangeLabel, reviewMessage } from "@/lib/review";
import { cn } from "@/lib/utils";

/** Uncommitted changes, or everything on the session's branch since it left its base branch. */
export type DiffMode = "uncommitted" | "branch";

/** The branch a worktree session was created from, which its own branch is compared with. */
export const diffBase = (s: Session) => (s.worktree && s.branch && s.branch !== s.gitBranch ? s.branch : undefined);

export interface DiffState {
  diff?: GitDiff;
  loading: boolean;
  error?: string;
  refresh: () => void;
}

/** How often the diff is refetched while the agent is working and the changes are on screen. */
const LIVE_REFRESH_MS = 3000;

/**
 * The session's changes, refetched when its run ends, on reconnecting, and every few seconds
 * while the agent works and `live` (the changes are being looked at).
 */
export function useDiff(session: Session | undefined, mode: DiffMode, live: boolean): DiffState {
  const path = session && !session.workdirRemoved ? (session.workdir ?? session.cwd).trim() : "";
  const baseBranch = session && mode === "branch" ? diffBase(session) : undefined;
  const running = !!session?.runId;
  const [state, setState] = useState<Omit<DiffState, "refresh">>({ loading: false });
  const latest = useRef(0);

  const refresh = () => {
    if (!path) return setState({ loading: false });
    const req = ++latest.current;
    setState((s) => ({ ...s, loading: true }));
    gitDiff(path, baseBranch)
      .then((diff) => req === latest.current && setState({ diff, loading: false }))
      .catch((e) => req === latest.current && setState({ loading: false, error: String(e.message ?? e) }));
  };

  // A different checkout or comparison: start over rather than show the old one meanwhile.
  useEffect(() => {
    setState({ loading: false });
    refresh();
  }, [path, baseBranch]);
  // The agent finished (or started): whatever it did is on disk now.
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current !== running) refresh();
    wasRunning.current = running;
  }, [running]);
  // Coming back to the changes: they may have been made since.
  const wasLive = useRef(live);
  useEffect(() => {
    if (live && !wasLive.current) refresh();
    wasLive.current = live;
    if (!live || !running) return;
    const t = setInterval(refresh, LIVE_REFRESH_MS);
    return () => clearInterval(t);
  }, [live, running, path, baseBranch]);
  useEffect(() => onReconnect(refresh), [path, baseBranch]);

  return { ...state, refresh };
}

const STATUS: Record<FileStatus, { letter: string; label: string; className: string }> = {
  added: { letter: "A", label: "Added", className: "text-emerald-600 dark:text-emerald-400" },
  modified: { letter: "M", label: "Modified", className: "text-amber-600 dark:text-amber-400" },
  deleted: { letter: "D", label: "Deleted", className: "text-destructive" },
  renamed: { letter: "R", label: "Renamed", className: "text-sky-600 dark:text-sky-400" },
};

function StatusLetter({ file }: { file: FileDiff }) {
  const s = STATUS[file.status];
  return (
    <span className={cn("w-3 shrink-0 text-center font-mono text-[11px] font-semibold", s.className)} title={file.untracked ? "Untracked" : s.label}>
      {file.untracked ? "U" : s.letter}
    </span>
  );
}

function Counts({ additions, deletions }: { additions: number; deletions: number }) {
  return (
    <span className="shrink-0 font-mono text-[11px] tabular-nums">
      {additions > 0 && <span className="text-emerald-600 dark:text-emerald-400">+{additions}</span>}
      {additions > 0 && deletions > 0 && " "}
      {deletions > 0 && <span className="text-destructive">−{deletions}</span>}
    </span>
  );
}

const splitPath = (p: string) => {
  const i = p.lastIndexOf("/");
  return { dir: i < 0 ? "" : p.slice(0, i + 1), name: p.slice(i + 1) };
};

/**
 * A file's diff as the two documents it shows parts of: the old side (context and deleted
 * lines) and the new side (context and added lines), hunk after hunk. Highlighting these rather
 * than line by line keeps multi-line strings and comments coloured right, within a hunk at least.
 * `at[h][i]` is where hunk `h`'s line `i` is: which side, and which of its lines.
 */
function diffSides(file: FileDiff) {
  const old: string[] = [];
  const now: string[] = [];
  const at = file.hunks.map((h) =>
    h.lines.map((l): [side: 0 | 1, line: number] => {
      if (l.kind === "delete") return [0, old.push(l.text) - 1];
      if (l.kind === "context") old.push(l.text);
      return [1, now.push(l.text) - 1];
    }),
  );
  return { docs: [old.join("\n"), now.join("\n")], at };
}

/** What the diff needs from the review: the selection, the drafts, and how to act on them. */
interface ReviewContext {
  review: Review;
  /** The comment box is open on the selection. */
  composing: boolean;
  onCompose: (on: boolean) => void;
  /** Sends the message to the agent, rejecting if it couldn't be sent. */
  onAsk: (prompt: string) => Promise<void>;
  busy: boolean;
}

function Hunk({ file, index, at, tokens, ctx }: { file: FileDiff; index: number; at?: [0 | 1, number][]; tokens?: Token[][][]; ctx: ReviewContext }) {
  const hunk = file.hunks[index];
  const { review } = ctx;
  const sel = review.selection;
  const drag = useRef<number | undefined>(undefined);
  // While dragging, the actions stay out of the way: they would push the lines under the pointer.
  const [dragging, setDragging] = useState(false);
  // A drag from one gutter to another selects the lines between; the button is let go anywhere.
  useEffect(() => {
    const up = () => ((drag.current = undefined), setDragging(false));
    window.addEventListener("mouseup", up);
    return () => window.removeEventListener("mouseup", up);
  }, []);
  const pick = (i: number, extend: boolean) => {
    ctx.onCompose(false);
    review.select({ path: file.path, hunk: index, from: i, to: i }, extend);
  };
  const gutter = (i: number, n: number | null) => (
    <td className="w-px min-w-10 border-r border-border/60 p-0 text-right align-top text-muted-foreground/70 select-none">
      <button
        type="button"
        tabIndex={-1}
        aria-label={`Select ${n === null ? "line" : `line ${n}`} to comment on (shift-click for a range)`}
        className="block w-full cursor-pointer px-2 text-right hover:bg-primary/25 hover:text-foreground"
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          drag.current = e.shiftKey ? undefined : i;
          setDragging(!e.shiftKey);
          pick(i, e.shiftKey);
        }}
        onMouseEnter={() => {
          const from = drag.current;
          if (from !== undefined) review.select({ path: file.path, hunk: index, from: Math.min(from, i), to: Math.max(from, i) });
        }}
        onClick={(e) => e.detail === 0 && pick(i, e.shiftKey)}
      >
        {n ?? "\u00a0"}
      </button>
    </td>
  );
  const anchor = sel && sel.path === file.path && sel.hunk === index ? anchorOf(file, sel) : undefined;
  const end = (i: number) => !dragging && sel && sel.path === file.path && sel.hunk === index && sel.to === i;
  const draftsAfter = (i: number) =>
    review.drafts.filter((d) => {
      const at = anchorEnd(file, d.anchor);
      return d.anchor.path === file.path && at?.[0] === index && at[1] === i;
    });
  return (
    <>
      <tr className="group/hunk bg-sky-500/8 text-muted-foreground">
        <td colSpan={3} className="px-3 py-1 font-mono text-[11px] whitespace-pre">
          <div className="sticky left-3 flex w-[calc(100cqw-1.5rem)] items-center gap-3">
            <span className="min-w-0 truncate">{hunk.header}</span>
            <Button
              size="xs"
              variant="ghost"
              className="ml-auto h-5 opacity-0 group-hover/hunk:opacity-100 focus-visible:opacity-100"
              title="Comment on or ask about this whole hunk"
              onClick={() => (ctx.onCompose(false), review.select(hunkSelection(file.path, index, hunk)))}
            >
              <MessageSquarePlus /> Hunk
            </Button>
          </div>
        </td>
      </tr>
      {hunk.lines.map((l, i) => {
        const where = at?.[i];
        return (
          <Fragment key={i}>
            <tr
              className={cn(
                l.kind === "add" && "bg-emerald-500/12 dark:bg-emerald-400/12",
                l.kind === "delete" && "bg-red-500/12 dark:bg-red-400/12",
                isSelected(sel, file.path, index, i) && "bg-primary/20 dark:bg-primary/25",
              )}
            >
              {gutter(i, l.old)}
              {gutter(i, l.new)}
              <td className="pr-4 whitespace-pre">
                <span
                  className={cn(
                    "inline-block w-5 text-center select-none",
                    l.kind === "add" && "text-emerald-600 dark:text-emerald-400",
                    l.kind === "delete" && "text-destructive",
                  )}
                >
                  {l.kind === "add" ? "+" : l.kind === "delete" ? "-" : " "}
                </span>
                {renderLine(l.text, where && tokens?.[where[0]][where[1]])}
                {l.noNewline && <span className="ml-2 text-[10px] text-muted-foreground select-none" title="No newline at end of file">⏎̸</span>}
              </td>
            </tr>
            {(draftsAfter(i).length > 0 || (anchor && end(i))) && (
              <tr>
                <td colSpan={3} className="p-0">
                  {draftsAfter(i).map((d) => (
                    <DraftComment key={d.id} draft={d} onEdit={(t) => review.edit(d.id, t)} onRemove={() => review.remove(d.id)} />
                  ))}
                  {anchor && end(i) && <SelectionPanel anchor={anchor} ctx={ctx} />}
                </td>
              </tr>
            )}
          </Fragment>
        );
      })}
    </>
  );
}

/** The actions on the selected lines, or the comment box once Comment is pressed. */
function SelectionPanel({ anchor, ctx }: { anchor: Anchor; ctx: ReviewContext }) {
  const { review } = ctx;
  const label = rangeLabel(anchor);
  if (ctx.composing) return <CommentBox label={label} onSave={(t) => (ctx.onCompose(false), review.add(anchor, t))} onCancel={() => ctx.onCompose(false)} />;
  return (
    <SelectionActions
      label={`${anchor.quote.length} ${anchor.quote.length === 1 ? "line" : "lines"} selected · ${label}`}
      busy={ctx.busy}
      onComment={() => ctx.onCompose(true)}
      onExplain={() => void ctx.onAsk(explainMessage(anchor))}
      onCancel={() => review.select(undefined)}
    />
  );
}

function DiffLines({ file, ctx }: { file: FileDiff; ctx: ReviewContext }) {
  const sides = useMemo(() => diffSides(file), [file]);
  const tokens = useHighlight(file.path, sides.docs)?.docs;
  return (
    <div className="@container overflow-x-auto">
      <table className="w-full border-collapse font-mono text-xs leading-5 [tab-size:4]">
        <tbody>
          {file.hunks.map((_, i) => (
            <Hunk key={i} file={file} index={i} at={sides.at[i]} tokens={tokens} ctx={ctx} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function FileCard({ file, open, onToggle, ctx }: { file: FileDiff; open: boolean; onToggle: () => void; ctx: ReviewContext }) {
  const { dir, name } = splitPath(file.path);
  const empty = file.binary ? "Binary file" : file.omitted ? "Too large to show" : !file.hunks.length ? (file.status === "renamed" ? "Renamed without changes" : "No content changes") : undefined;
  return (
    <section id={`diff-${file.path}`} className="overflow-clip rounded-lg border bg-card">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="sticky top-0 z-10 flex w-full items-center gap-2 border-b bg-card px-3 py-2 text-left text-xs hover:bg-muted"
      >
        {open ? <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" /> : <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />}
        <StatusLetter file={file} />
        <span className="min-w-0 flex-1 truncate font-mono" title={file.path}>
          {file.oldPath && <span className="text-muted-foreground">{file.oldPath} → </span>}
          <span className="text-muted-foreground">{dir}</span>
          <span className="font-medium">{name}</span>
        </span>
        <Counts additions={file.additions} deletions={file.deletions} />
      </button>
      {open &&
        (empty ? (
          <p className="px-3 py-3 text-xs text-muted-foreground">{empty}</p>
        ) : (
          <DiffLines file={file} ctx={ctx} />
        ))}
    </section>
  );
}

/** Files with more changed lines than this start collapsed. */
const COLLAPSE_LINES = 600;
const startsOpen = (f: FileDiff) => f.status !== "deleted" && f.additions + f.deletions <= COLLAPSE_LINES;

/** The session's changes: a file list, and each file's diff. */
export function DiffView({
  session,
  state,
  mode,
  onMode,
  review,
  onAsk,
}: {
  session: Session;
  state: DiffState;
  mode: DiffMode;
  onMode: (m: DiffMode) => void;
  review: Review;
  /** Sends a message to the session's agent (queued while it works), rejecting if it couldn't be. */
  onAsk?: (prompt: string) => Promise<void>;
}) {
  const { diff, loading, error, refresh } = state;
  const [composing, setComposing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [sendError, setSendError] = useState<string>();
  const ask = async (prompt: string) => {
    if (!onAsk) return;
    setBusy(true);
    setSendError(undefined);
    try {
      await onAsk(prompt);
    } catch (e) {
      setSendError(String((e as Error).message ?? e));
      throw e;
    } finally {
      setBusy(false);
    }
  };
  const ctx: ReviewContext = { review, composing, onCompose: setComposing, onAsk: (p) => ask(p).then(() => review.select(undefined), () => {}), busy };
  // Esc clears the selection (a comment box handles its own Esc).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !composing && review.select(undefined);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const base = diffBase(session);
  // Files toggled away from how they start (see `startsOpen`), by path.
  const [toggled, setToggled] = useState(new Set<string>());
  useEffect(() => (setToggled(new Set()), setSelected(undefined), setComposing(false), setSendError(undefined)), [session.id, mode]);

  const files = diff?.files ?? [];
  const isOpen = (f: FileDiff) => startsOpen(f) !== toggled.has(f.path);
  const toggle = (path: string) =>
    setToggled((t) => {
      const next = new Set(t);
      next.has(path) ? next.delete(path) : next.add(path);
      return next;
    });
  // The file last picked in the tree.
  const [selected, setSelected] = useState<string>();
  const reveal = (f: FileDiff) => {
    setSelected(f.path);
    if (!isOpen(f)) toggle(f.path);
    requestAnimationFrame(() => document.getElementById(`diff-${f.path}`)?.scrollIntoView({ block: "start" }));
  };
  // Drafts whose lines the diff has moved past (or whose file it no longer has).
  const lost = review.drafts.filter((d) => {
    const f = files.find((f) => f.path === d.anchor.path);
    return !f || !anchorEnd(f, d.anchor);
  });
  const additions = files.reduce((n, f) => n + f.additions, 0);
  const deletions = files.reduce((n, f) => n + f.deletions, 0);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
        {base && (
          <div className="flex rounded-lg bg-muted p-0.5" role="radiogroup" aria-label="Compare with">
            {(
              [
                ["branch", `All changes vs ${base}`],
                ["uncommitted", "Uncommitted"],
              ] as const
            ).map(([m, label]) => (
              <button
                key={m}
                type="button"
                role="radio"
                aria-checked={mode === m}
                onClick={() => onMode(m)}
                className={cn("rounded-md px-2 py-0.5", mode === m ? "bg-background text-foreground shadow-xs" : "hover:text-foreground")}
              >
                {label}
              </button>
            ))}
          </div>
        )}
        {diff && (
          <span>
            {files.length} {files.length === 1 ? "file" : "files"} changed
            {files.length > 0 && (
              <>
                {" · "}
                <Counts additions={additions} deletions={deletions} />
              </>
            )}
            {diff.base && (
              <span className="font-mono" title="Compared with this commit">
                {" · "}
                {diff.base}
              </span>
            )}
          </span>
        )}
        <Button variant="ghost" size="icon-xs" className="ml-auto" title="Refresh" aria-label="Refresh" onClick={refresh} disabled={loading}>
          {loading ? <Spinner /> : <RefreshCw />}
        </Button>
      </div>

      {session.workdirRemoved ? (
        <Empty>This session's worktree was removed.</Empty>
      ) : error ? (
        <Empty className="text-destructive">{error}</Empty>
      ) : !diff ? (
        <Empty>
          <Spinner /> Loading changes…
        </Empty>
      ) : !files.length ? (
        <Empty>{mode === "branch" && base ? `No changes since ${base}.` : "No uncommitted changes."}</Empty>
      ) : (
        <div className="flex min-h-0 flex-1">
          <nav className="hidden w-64 shrink-0 overflow-y-auto border-r md:block" aria-label="Changed files">
            <FileTree
              files={files}
              selected={selected}
              onSelect={reveal}
              label="Changed files"
              decorate={(f) => ({
                before: <StatusLetter file={f} />,
                after: <Counts additions={f.additions} deletions={f.deletions} />,
                className: cn(f.status === "deleted" && "line-through"),
                title: f.oldPath ? `${f.oldPath} → ${f.path}` : f.path,
              })}
            />
          </nav>
          <div className="min-w-0 flex-1 overflow-y-auto">
            <div className="flex flex-col gap-3 p-4 select-text">
              {diff.truncated && (
                <p className="text-xs text-muted-foreground">Some large files are listed without their lines.</p>
              )}
              {lost.length > 0 && (
                <section className="rounded-lg border bg-card p-2 text-xs">
                  <p className="mb-1 px-1 text-muted-foreground">Comments on lines that are no longer in the diff</p>
                  <div className="@container">
                    {lost.map((d) => (
                      <DraftComment key={d.id} draft={d} showPath onEdit={(t) => review.edit(d.id, t)} onRemove={() => review.remove(d.id)} />
                    ))}
                  </div>
                </section>
              )}
              {files.map((f) => (
                <FileCard key={f.path} file={f} open={isOpen(f)} onToggle={() => toggle(f.path)} ctx={ctx} />
              ))}
            </div>
          </div>
        </div>
      )}

      {review.drafts.length > 0 && (
        <div className="flex shrink-0 items-center gap-3 border-t bg-card px-4 py-2 text-xs" role="region" aria-label="Review comments">
          <span className="font-medium">
            {review.drafts.length} draft {review.drafts.length === 1 ? "comment" : "comments"}
          </span>
          {sendError && <span className="min-w-0 truncate text-destructive" title={sendError}>{sendError}</span>}
          <Button size="xs" variant="ghost" className="ml-auto" onClick={review.clearDrafts} disabled={busy}>
            Discard
          </Button>
          <Button
            size="xs"
            disabled={busy || !onAsk}
            title={session.runId ? "The agent is working: this is queued until its turn ends" : "Send all comments to the agent as one message"}
            onClick={() => ask(reviewMessage(review.drafts)).then(review.clearDrafts, () => {})}
          >
            {busy ? <Spinner /> : <Send />} {session.runId ? "Queue for agent" : "Send to agent"}
          </Button>
        </div>
      )}
    </div>
  );
}

function Empty({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground", className)}>
      <FileDiffIcon className="size-6 opacity-50" />
      <div className="flex items-center gap-2">{children}</div>
    </div>
  );
}
