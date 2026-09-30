import { Fragment, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FileDiff as FileDiffIcon, MessageSquarePlus, Minus, Plus, RefreshCw, Send } from "lucide-react";
import { BranchStatus, FileDiff, FileStatus, GitDiff, Session, gitCommit, gitDiff, gitPush, gitStage, gitStageHunk, gitStatus, onReconnect } from "@/api";
import { FileTree } from "@/components/FileTree";
import { CommitBox, CommitList, GitError, PushControl, useCommits } from "@/components/GitBar";
import { CommentBox, DraftComment, SelectionActions } from "@/components/ReviewComments";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { type Token, renderLine, useHighlight } from "@/lib/highlight";
import { type Anchor, type Review, anchorEnd, anchorOf, explainMessage, hunkSelection, inFile, isSelected, rangeLabel, reviewMessage } from "@/lib/review";
import { cn } from "@/lib/utils";

/**
 * Uncommitted changes (staged and not), everything on the session's branch since it left its base
 * branch, or the branch's commits, one at a time.
 */
export type DiffMode = "uncommitted" | "branch" | "commits";

const errorText = (e: unknown) => String((e as Error)?.message ?? e);

/** The folder the session's git commands run in. */
export const sessionPath = (s: Session) => (s.workdirRemoved ? "" : (s.workdir ?? s.cwd).trim());

/** Names a file's part of the diff: a file with staged and unstaged changes is shown twice. */
const fileKey = (f: FileDiff) => `${f.staged ? "s" : "u"}:${f.path}`;

/** The branch a worktree session was created from, which its own branch is compared with. */
export const diffBase = (s: Session) => (s.worktree && s.branch && s.branch !== s.gitBranch ? s.branch : undefined);

export interface DiffState {
  diff?: GitDiff;
  /** The branch against its remote. */
  status?: BranchStatus;
  loading: boolean;
  error?: string;
  refresh: () => void;
}

/** How often the diff is refetched while the agent is working and the changes are on screen. */
const LIVE_REFRESH_MS = 3000;

/**
 * The session's changes (in commits mode, those of `commit`), refetched when its run ends, on
 * reconnecting, and every few seconds while the agent works and `live` (the changes are being
 * looked at).
 */
export function useDiff(session: Session | undefined, mode: DiffMode, live: boolean, commit?: string): DiffState {
  const path = session ? sessionPath(session) : "";
  const baseBranch = session && mode === "branch" ? diffBase(session) : undefined;
  const showing = mode === "commits" ? commit : undefined;
  const running = !!session?.runId;
  const [state, setState] = useState<Omit<DiffState, "refresh">>({ loading: false });
  const latest = useRef(0);

  const refresh = () => {
    if (!path) return setState({ loading: false });
    const req = ++latest.current;
    setState((s) => ({ ...s, loading: true }));
    // In commits mode nothing is shown until a commit is picked.
    const diff = mode === "commits" && !commit ? Promise.resolve(undefined) : gitDiff(path, baseBranch, showing);
    Promise.all([diff, gitStatus(path).catch(() => undefined)])
      .then(([diff, status]) => req === latest.current && setState({ diff, status, loading: false }))
      .catch((e) => req === latest.current && setState({ loading: false, error: errorText(e) }));
  };

  // A different checkout or comparison: start over rather than show the old one meanwhile.
  useEffect(() => {
    setState({ loading: false });
    refresh();
  }, [path, baseBranch, mode, commit]);
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
  }, [live, running, path, baseBranch, mode, commit]);
  useEffect(() => onReconnect(refresh), [path, baseBranch, mode, commit]);

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
  /** Present where files and hunks can be staged and unstaged. */
  stage?: {
    busy: boolean;
    onFile: (file: FileDiff) => void;
    onHunk: (file: FileDiff, index: number) => void;
  };
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
    review.select({ path: file.path, staged: file.staged, hunk: index, from: i, to: i }, extend);
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
          if (from !== undefined) review.select({ path: file.path, staged: file.staged, hunk: index, from: Math.min(from, i), to: Math.max(from, i) });
        }}
        onClick={(e) => e.detail === 0 && pick(i, e.shiftKey)}
      >
        {n ?? "\u00a0"}
      </button>
    </td>
  );
  const anchor = sel && inFile(sel, file) && sel.hunk === index ? anchorOf(file, sel) : undefined;
  const end = (i: number) => !dragging && sel && inFile(sel, file) && sel.hunk === index && sel.to === i;
  // A renamed file's hunks can't be told apart from a whole new file's, and a new file has no
  // earlier version to stage part of.
  const stageable = ctx.stage && !file.untracked && !file.binary && !file.omitted && file.status !== "renamed";
  const draftsAfter = (i: number) =>
    review.drafts.filter((d) => {
      const at = anchorEnd(file, d.anchor);
      return inFile(d.anchor, file) && at?.[0] === index && at[1] === i;
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
              className={"ml-auto h-5 opacity-0 group-hover/hunk:opacity-100 focus-visible:opacity-100"}
              title="Comment on or ask about this whole hunk"
              onClick={() => (ctx.onCompose(false), review.select(hunkSelection(file, index)))}
            >
              <MessageSquarePlus /> Hunk
            </Button>
            {stageable && (
              <Button
                size="xs"
                variant="outline"
                className="h-5"
                disabled={ctx.stage?.busy}
                title={file.staged ? "Take this hunk out of the commit" : "Add this hunk to the commit"}
                onClick={() => ctx.stage?.onHunk(file, index)}
              >
                {file.staged ? <Minus /> : <Plus />} {file.staged ? "Unstage hunk" : "Stage hunk"}
              </Button>
            )}
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
                isSelected(sel, file, index, i) && "bg-primary/20 dark:bg-primary/25",
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
  const stage = ctx.stage;
  return (
    <section id={`diff-${fileKey(file)}`} className={cn("overflow-clip rounded-lg border bg-card", stage && file.staged && "border-l-2 border-l-emerald-500")}>
      <div className="sticky top-0 z-10 flex items-center border-b bg-card hover:bg-muted">
        <button type="button" onClick={onToggle} aria-expanded={open} className="flex min-w-0 flex-1 items-center gap-2 px-3 py-2 text-left text-xs">
          {open ? <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" /> : <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />}
          <StatusLetter file={file} />
          <span className="min-w-0 flex-1 truncate font-mono" title={file.path}>
            {file.oldPath && <span className="text-muted-foreground">{file.oldPath} → </span>}
            <span className="text-muted-foreground">{dir}</span>
            <span className="font-medium">{name}</span>
          </span>
          <Counts additions={file.additions} deletions={file.deletions} />
        </button>
        {stage && (
          <Button
            size="xs"
            variant="outline"
            className="mr-2 shrink-0"
            disabled={stage.busy}
            title={file.staged ? "Take this file out of the commit" : "Add this file to the commit"}
            onClick={() => stage.onFile(file)}
          >
            {file.staged ? <Minus /> : <Plus />} {file.staged ? "Unstage" : "Stage"}
          </Button>
        )}
      </div>
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

/** A part of the changes (the staged ones, or the rest) with what can be done to all of it. */
function SectionHeading({ title, count, action }: { title: string; count: number; action?: ReactNode }) {
  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 text-[11px] font-medium whitespace-nowrap text-muted-foreground uppercase">
      <span className="min-w-0 truncate">{title}</span>
      <span className="shrink-0 rounded-full bg-muted px-1.5 tabular-nums">{count}</span>
      {action}
    </div>
  );
}

/** The session's changes: a file list, and each file's diff. */
export function DiffView({
  session,
  state,
  mode,
  onMode,
  commit,
  onCommit,
  review,
  onAsk,
}: {
  session: Session;
  state: DiffState;
  mode: DiffMode;
  onMode: (m: DiffMode) => void;
  /** The commit whose changes are shown in commits mode. */
  commit?: string;
  onCommit: (sha: string | undefined) => void;
  review: Review;
  /** Sends a message to the session's agent (queued while it works), rejecting if it couldn't be. */
  onAsk?: (prompt: string) => Promise<void>;
}) {
  const { diff, status, loading, error, refresh } = state;
  const path = sessionPath(session);
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
      setSendError(errorText(e));
      throw e;
    } finally {
      setBusy(false);
    }
  };

  // Staging, committing and pushing change the repository, so the diff is fetched again after.
  const [acting, setActing] = useState(false);
  const [actError, setActError] = useState<string>();
  const act = async (change: () => Promise<unknown>) => {
    setActing(true);
    setActError(undefined);
    try {
      await change();
      // Hunks move between the staged and unstaged parts, so the lines picked no longer exist.
      review.select(undefined);
    } catch (e) {
      setActError(errorText(e));
    } finally {
      setActing(false);
      refresh();
    }
  };
  const [pushing, setPushing] = useState(false);
  const [pushError, setPushError] = useState<string>();
  const push = async () => {
    setPushing(true);
    setPushError(undefined);
    try {
      await gitPush(path);
    } catch (e) {
      setPushError(errorText(e));
    } finally {
      setPushing(false);
      refresh();
    }
  };
  const paths = (files: FileDiff[]) => files.flatMap((f) => (f.oldPath ? [f.path, f.oldPath] : [f.path]));

  const base = diffBase(session);
  const files = diff?.files ?? [];
  const staging = !!diff?.staging;
  const stage: ReviewContext["stage"] = staging
    ? {
        busy: acting,
        onFile: (f) => void act(() => gitStage(path, paths([f]), !f.staged)),
        onHunk: (f, i) => void act(() => gitStageHunk({ path, file: f.path, hunk: i, header: f.hunks[i].header, stage: !f.staged })),
      }
    : undefined;
  const ctx: ReviewContext = { review, composing, onCompose: setComposing, onAsk: (p) => ask(p).then(() => review.select(undefined), () => {}), busy, stage };
  // Esc clears the selection (a comment box handles its own Esc).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !composing && review.select(undefined);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const commits = useCommits(path, base, status?.head ?? undefined, mode === "commits");
  // Looking at commits starts on the newest.
  useEffect(() => {
    if (mode === "commits" && !commit && commits.commits.length) onCommit(commits.commits[0].sha);
  }, [mode, commit, commits.commits]);

  // Files toggled away from how they start (see `startsOpen`), by `fileKey`.
  const [toggled, setToggled] = useState(new Set<string>());
  useEffect(() => (setToggled(new Set()), setSelected(undefined), setComposing(false), setSendError(undefined), setActError(undefined), setPushError(undefined)), [session.id, mode, commit]);

  const isOpen = (f: FileDiff) => startsOpen(f) !== toggled.has(fileKey(f));
  const toggle = (key: string) =>
    setToggled((t) => {
      const next = new Set(t);
      next.has(key) ? next.delete(key) : next.add(key);
      return next;
    });
  // The file last picked in the tree.
  const [selected, setSelected] = useState<FileDiff>();
  const reveal = (f: FileDiff) => {
    setSelected(f);
    if (!isOpen(f)) toggle(fileKey(f));
    requestAnimationFrame(() => document.getElementById(`diff-${fileKey(f)}`)?.scrollIntoView({ block: "start" }));
  };
  // Drafts whose lines the diff has moved past (or whose file it no longer has).
  const lost = review.drafts.filter((d) => {
    const f = files.find((f) => inFile(d.anchor, f));
    return !f || !anchorEnd(f, d.anchor);
  });
  const changed = new Set(files.map((f) => f.path)).size;
  const additions = files.reduce((n, f) => n + f.additions, 0);
  const deletions = files.reduce((n, f) => n + f.deletions, 0);

  const sections = staging
    ? [
        { key: "staged", title: "Staged changes", files: files.filter((f) => f.staged), all: { label: "Unstage all", icon: <Minus />, stage: false } },
        { key: "unstaged", title: "Changes", files: files.filter((f) => !f.staged), all: { label: "Stage all", icon: <Plus />, stage: true } },
      ].filter((x) => x.files.length)
    : [{ key: "all", title: "", files, all: undefined }];
  const stagedCount = new Set(files.filter((f) => f.staged).map((f) => f.path)).size;
  const modes: [DiffMode, string][] = [...(base ? [["branch", `All changes vs ${base}`] as [DiffMode, string]] : []), ["uncommitted", "Uncommitted"], ["commits", "Commits"]];
  const showNav = mode === "commits" || files.length > 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
        <div className="flex rounded-lg bg-muted p-0.5" role="radiogroup" aria-label="Show">
          {modes.map(([m, label]) => (
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
        {diff && (
          <span>
            {changed} {changed === 1 ? "file" : "files"} changed
            {files.length > 0 && (
              <>
                {" · "}
                <Counts additions={additions} deletions={deletions} />
              </>
            )}
            {diff.base && (
              <span className="font-mono" title={mode === "commits" ? "This commit" : "Compared with this commit"}>
                {" · "}
                {diff.base}
              </span>
            )}
          </span>
        )}
        <div className="ml-auto flex items-center gap-3">
          <PushControl status={status} pushing={pushing} onPush={() => void push()} />
          <Button variant="ghost" size="icon-xs" title="Refresh" aria-label="Refresh" onClick={refresh} disabled={loading}>
            {loading ? <Spinner /> : <RefreshCw />}
          </Button>
        </div>
      </div>
      {(pushError || actError) && (
        <div className="flex shrink-0 flex-col gap-2 border-b px-4 py-2">
          {pushError && <GitError message={pushError} onDismiss={() => setPushError(undefined)} />}
          {actError && <GitError message={actError} onDismiss={() => setActError(undefined)} />}
        </div>
      )}

      {session.workdirRemoved ? (
        <Empty>This session's worktree was removed.</Empty>
      ) : (
        <div className="flex min-h-0 flex-1">
          {showNav && (
            <nav className="hidden w-64 shrink-0 flex-col overflow-hidden border-r md:flex" aria-label="Changed files">
              {staging && (
                <CommitBox
                  staged={stagedCount}
                  onCommit={async (message) => {
                    await gitCommit(path, message);
                    review.select(undefined);
                    refresh();
                  }}
                />
              )}
              {mode === "commits" && (
                <div className={cn("overflow-y-auto", files.length ? "max-h-[45%] shrink-0 border-b" : "flex-1")}>
                  <CommitList state={commits} base={base} selected={commit} onSelect={(c) => onCommit(c.sha)} />
                </div>
              )}
              <div className="min-h-0 flex-1 overflow-y-auto">
                {sections.map((sec) => (
                  <div key={sec.key}>
                    {staging && (
                      <SectionHeading
                        title={sec.title}
                        count={sec.files.length}
                        action={
                          sec.all && (
                            <Button size="xs" variant="ghost" className="ml-auto shrink-0 px-1.5" disabled={acting} onClick={() => void act(() => gitStage(path, paths(sec.files), sec.all.stage))}>
                              {sec.all.icon} {sec.all.label}
                            </Button>
                          )
                        }
                      />
                    )}
                    <FileTree
                      files={sec.files}
                      selected={selected && (!staging || selected.staged === (sec.key === "staged")) ? selected.path : undefined}
                      onSelect={reveal}
                      actions={
                        staging
                          ? (f) => (
                              <Button
                                size="icon-xs"
                                variant="ghost"
                                disabled={acting}
                                title={f.staged ? "Unstage this file" : "Stage this file"}
                                aria-label={f.staged ? `Unstage ${f.path}` : `Stage ${f.path}`}
                                onClick={() => void act(() => gitStage(path, paths([f]), !f.staged))}
                              >
                                {f.staged ? <Minus /> : <Plus />}
                              </Button>
                            )
                          : undefined
                      }
                      label={sec.title || "Changed files"}
                      decorate={(f) => ({
                        before: <StatusLetter file={f} />,
                        after: <Counts additions={f.additions} deletions={f.deletions} />,
                        className: cn(f.status === "deleted" && "line-through"),
                        title: f.oldPath ? `${f.oldPath} → ${f.path}` : f.path,
                      })}
                    />
                  </div>
                ))}
              </div>
            </nav>
          )}
          {error ? (
            <Empty className="text-destructive">{error}</Empty>
          ) : mode === "commits" && !commit ? (
            <Empty>{commits.loading ? "Loading commits…" : "Pick a commit to see what it changed."}</Empty>
          ) : !diff ? (
            <Empty>
              <Spinner /> Loading changes…
            </Empty>
          ) : !files.length ? (
            <Empty>{mode === "commits" ? "This commit changed no files." : mode === "branch" && base ? `No changes since ${base}.` : "No uncommitted changes."}</Empty>
          ) : (
            <div className="min-w-0 flex-1 overflow-y-auto">
              <div className="flex flex-col gap-3 p-4 select-text">
                {diff.truncated && <p className="text-xs text-muted-foreground">Some large files are listed without their lines.</p>}
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
                {sections.map((sec) => (
                  <Fragment key={sec.key}>
                    {staging && (
                      <div className="-mb-1 -ml-3">
                        <SectionHeading
                          title={sec.title}
                          count={sec.files.length}
                          action={
                            sec.all && (
                              <Button size="xs" variant="outline" className="ml-2" disabled={acting} onClick={() => void act(() => gitStage(path, paths(sec.files), sec.all.stage))}>
                                {sec.all.icon} {sec.all.label}
                              </Button>
                            )
                          }
                        />
                      </div>
                    )}
                    {sec.files.map((f) => (
                      <FileCard key={fileKey(f)} file={f} open={isOpen(f)} onToggle={() => toggle(fileKey(f))} ctx={ctx} />
                    ))}
                  </Fragment>
                ))}
              </div>
            </div>
          )}
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
