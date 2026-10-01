import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { ArrowDown, PanelLeft, ShieldAlert } from "lucide-react";
import {
  AgentInfo,
  AgentKind,
  ApiError,
  inTauri,
  macTitleBar,
  Decision,
  LogEntry,
  Session,
  answerApproval,
  deleteSession,
  getSession,
  listAgents,
  listSessions,
  onReconnect,
  onSessionEvent,
  removeQueued,
  sendMessage,
  sendQueued,
  startSession as startSessionCall,
  stopSession,
  updateSession,
} from "@/api";
import { BlockPlace, BlockView, TranscriptContext } from "@/components/Transcript";
import { ImageLinks } from "@/lib/imageLinks";
import { WorktreeCleanup } from "@/components/WorktreeCleanup";
import { BROWSER_WIDTH, BrowserPanel } from "@/components/BrowserPanel";
import { closePreview, usePreview } from "@/lib/preview";
import { TASKS_WIDTH, TaskLogView, TasksPanel, useTasks } from "@/components/TasksPanel";
import { useForwarding } from "@/components/Ports";
import { SettingsDialog } from "@/components/SettingsDialog";
import { TerminalPanel } from "@/components/TerminalPanel";
import { UsageIndicator, useBudget, usePlanUsage } from "@/components/UsageIndicator";
import { Composer } from "@/components/Composer";
import { NewSessionPage } from "@/components/NewSessionPage";
import { SessionHeader, SessionView } from "@/components/SessionHeader";
import { ForkDialog, HandoffDialog } from "@/components/ContinueDialogs";
import { useReview } from "@/lib/review";
import { DiffMode, DiffView, diffBase, useDiff } from "@/components/DiffView";
import { ConnectionBanner, pausedReason, useConnection } from "@/components/ConnectionPicker";
import { FileBrowser } from "@/components/FileBrowser";
import { SIDEBAR_WIDTH, Sidebar } from "@/components/Sidebar";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { isBoolean, isNumber, useStored } from "@/lib/layout";
import { useAttention } from "@/lib/notifications";
import {
  NewSession,
  Transcript,
  blockOfEntry,
  addDelta,
  applyEntries,
  awaitingApproval,
  byActivity,
  canRun,
  emptyLive,
  emptyTranscript,
  expireApprovals,
  groupByProject,
  newSession,
  projectKey,
  settle,
  type Live,
} from "@/session";

const COLLAPSED_KEY = "bach.collapsedProjects";
const loadCollapsed = (): Set<string> => {
  try {
    return new Set(JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]"));
  } catch {
    return new Set();
  }
};

const message = (e: unknown) => ApiError.from(e).message;

/** Adds or replaces a session, keeping the list most recently active first. */
const upsert = (all: Session[], s: Session) => [...all.filter((x) => x.id !== s.id), s].sort(byActivity);

export function App() {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  /** A fork or hand-off being set up from the open session. */
  const [continuing, setContinuing] = useState<{ kind: "fork"; seq: number; text: string } | { kind: "handoff" }>();
  /** The open session; none means the create page. */
  const [activeId, setActiveId] = useState<string>();
  // A fresh set per session: images only link within the transcript they're in.
  const imageLinks = useMemo(() => new ImageLinks(), [activeId]);
  /** What the create page is setting up. */
  const [newDraft, setNewDraft] = useState<NewSession>(() => newSession("claude"));
  const [draft, setDraft] = useState("");
  const [draftImages, setDraftImages] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [starting, setStarting] = useState(false);
  const [connectionError, setConnectionError] = useState<string>();
  const paused = pausedReason(useConnection());
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const [cleanupOpen, setCleanupOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [viewingTask, setViewingTask] = useState<TaskLogView>();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useStored("bach.sidebarOpen", true, isBoolean);
  const [sidebarWidth, setSidebarWidth] = useStored("bach.sidebarWidth", SIDEBAR_WIDTH.default, isNumber);
  const [tasksWidth, setTasksWidth] = useStored("bach.tasksWidth", TASKS_WIDTH.default, isNumber);
  const [browserWidth, setBrowserWidth] = useStored("bach.browserWidth", BROWSER_WIDTH.default, isNumber);
  const preview = usePreview();
  const tasks = useTasks();
  const forwarding = useForwarding();
  const planUsage = usePlanUsage();
  const budget = useBudget();
  // Transcripts of the sessions opened so far. Kept in a ref so event handlers see the latest
  // `seq` (to spot gaps); `rerender` shows changes.
  const transcripts = useRef(new Map<string, Transcript>());
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  // What each session's agent is still writing, shown until the finished entry arrives.
  const drafts = useRef(new Map<string, Live>());
  // Transcripts being fetched, with the entries that arrived meanwhile.
  const fetching = useRef(new Map<string, LogEntry[]>());
  // Follow new output only while the reader is at the bottom; scrolling up pins the view.
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [showJump, setShowJump] = useState(false);
  const [view, setView] = useState<SessionView>("chat");
  const [diffMode, setDiffMode] = useState<DiffMode>("uncommitted");
  // The commit shown while browsing the branch's commits.
  const [diffCommit, setDiffCommit] = useState<string>();
  // A search hit to scroll to once its session's transcript is shown, and a counter that asks the
  // sidebar's search field for focus.
  const [reveal, setReveal] = useState<{ id: string; seq: number }>();
  const [searchFocus, setSearchFocus] = useState(0);

  /** Fetches a session's transcript, or the part of it after what's already here. */
  async function fetchTranscript(id: string) {
    if (fetching.current.has(id)) return;
    fetching.current.set(id, []);
    const have = transcripts.current.get(id) ?? emptyTranscript;
    try {
      const log = await getSession(id, have.seq || undefined);
      const arrived = (fetching.current.get(id) ?? []).sort((a, b) => a.seq - b.seq);
      const current = transcripts.current.get(id) ?? emptyTranscript;
      transcripts.current.set(id, applyEntries(applyEntries(current, log.entries), arrived));
      setSessions((all) => upsert(all, log.session));
      rerender();
    } catch (e) {
      if (ApiError.from(e).code === "not_found") setSessions((all) => all.filter((s) => s.id !== id));
      else setConnectionError(message(e));
    } finally {
      fetching.current.delete(id);
    }
  }

  function addEntry(id: string, entry: LogEntry) {
    const live = drafts.current.get(id);
    if (live) drafts.current.set(id, settle(live, entry.entry));
    setSessions((all) => all.map((s) => (s.id === id ? { ...s, updatedAt: entry.at, lastSeq: entry.seq } : s)).sort(byActivity));
    const buffer = fetching.current.get(id);
    if (buffer) return void buffer.push(entry);
    const t = transcripts.current.get(id);
    if (!t) return; // not opened yet: fetched in full when it is
    if (entry.seq > t.seq + 1) return void fetchTranscript(id); // missed some
    transcripts.current.set(id, applyEntries(t, [entry]));
    rerender();
  }

  const loadSessions = () =>
    listSessions()
      .then((list) => {
        setSessions(list.sort(byActivity));
        setConnectionError(undefined);
        return list;
      })
      .catch((e) => {
        setConnectionError(message(e));
        return [] as Session[];
      });

  useEffect(() => {
    Promise.all([listAgents(), loadSessions()])
      .then(([a, list]) => {
        setAgents(a);
        // Open on the create page, set up like the latest session.
        setNewDraft(newSession(list[0]?.agent ?? a.find((x) => x.installed)?.kind ?? "claude", list[0]?.cwd));
      })
      .catch((e) => setConnectionError(message(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    const unEvents = onSessionEvent((e) => {
      switch (e.type) {
        case "changed":
          return setSessions((all) => upsert(all, e.session));
        case "entry":
          return addEntry(e.sessionId, e.entry);
        case "delta":
          drafts.current.set(e.sessionId, addDelta(drafts.current.get(e.sessionId) ?? emptyLive, e));
          return rerender();
        case "deleted":
          transcripts.current.delete(e.sessionId);
          setSessions((all) => all.filter((s) => s.id !== e.sessionId));
          setActiveId((id) => (id === e.sessionId ? undefined : id));
      }
    });
    // Whatever happened while disconnected: catch up.
    const unReconnect = onReconnect(() => {
      // The first load may have failed while disconnected.
      void listAgents().then(setAgents, () => {});
      void loadSessions();
      for (const id of transcripts.current.keys()) void fetchTranscript(id);
    });
    return () => (unEvents(), unReconnect());
  }, []);

  const active = sessions.find((s) => s.id === activeId);
  const transcript = activeId ? transcripts.current.get(activeId) : undefined;
  const live = (activeId && drafts.current.get(activeId)) || emptyLive;
  const showChanges = !!active && view === "changes";
  const showFiles = !!active && view === "files";
  const review = useReview(active?.id);
  // A selection being commented on would shift under the diff's live refresh.
  const diff = useDiff(active, diffMode, showChanges && !review.selection, diffCommit);

  // Each session opens on its chat; a worktree's changes are compared with where it branched from.
  useEffect(() => {
    setView("chat");
    setDiffMode(active && diffBase(active) ? "branch" : "uncommitted");
    setDiffCommit(undefined);
    setContinuing(undefined);
  }, [activeId]);

  useEffect(() => {
    if (activeId && !transcripts.current.has(activeId)) void fetchTranscript(activeId);
  }, [activeId]);

  const scrollToBottom = () => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };
  const lastTop = useRef(0);
  const followLatest = () => {
    stick.current = true;
    setShowJump(false);
    scrollToBottom();
    lastTop.current = scrollRef.current?.scrollTop ?? 0;
  };
  // Only a scroll that moves *up* away from the bottom unpins. Our own scroll-to-bottom can
  // land a little short when content grows again before the event fires; that must not unpin.
  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.scrollTop;
    if (el.scrollHeight - top - el.clientHeight < 48) {
      stick.current = true;
      setShowJump(false);
    } else if (top < lastTop.current) {
      stick.current = false;
      setShowJump(true);
    }
    lastTop.current = top;
  };

  // Re-pin to the bottom when switching sessions, then follow growth of the content (new
  // blocks, sub-agent steps, an expanded card) for as long as the reader hasn't scrolled up.
  const chatShown = !!active && view === "chat";
  useEffect(() => {
    const content = contentRef.current;
    if (!content) return;
    followLatest();
    const ro = new ResizeObserver(() => stick.current && scrollToBottom());
    ro.observe(content);
    // Also follow when the window is resized while pinned to the bottom.
    if (scrollRef.current) ro.observe(scrollRef.current);
    return () => ro.disconnect();
  }, [activeId, chatShown]);

  // Scrolls to a search hit (instead of the latest message) once its transcript is loaded, and
  // flashes the message. Declared after the re-pin above so that this scroll comes last.
  useEffect(() => {
    if (!reveal || reveal.id !== activeId || !chatShown || !transcript) return;
    const i = blockOfEntry(transcript.blocks, reveal.seq);
    const el = contentRef.current?.querySelector(`[data-block="${i}"]`)?.firstElementChild;
    setReveal(undefined);
    if (!el) return;
    stick.current = false;
    setShowJump(true);
    el.scrollIntoView({ block: "center" });
    lastTop.current = scrollRef.current?.scrollTop ?? 0;
    el.animate(
      [
        { outline: "2px solid color-mix(in oklab, var(--primary) 70%, transparent)", outlineOffset: "4px" },
        { outline: "2px solid transparent", outlineOffset: "4px" },
      ],
      { duration: 1800, easing: "ease-out" },
    );
  }, [reveal, activeId, chatShown, transcript?.seq]);

  // Cmd/Ctrl+B hides and shows the sidebar, Cmd/Ctrl+K searches sessions; Cmd/Ctrl+Shift+D switches between chat and changes.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const key = e.key.toLowerCase();
      if (!e.shiftKey && key === "b") {
        e.preventDefault();
        setSidebarOpen(!sidebarOpen);
      } else if (!e.shiftKey && key === "k") {
        e.preventDefault();
        setSidebarOpen(true);
        setSearchFocus((n) => n + 1);
      } else if (e.shiftKey && key === "d") {
        e.preventDefault();
        setView((v) => (v === "chat" ? "changes" : "chat"));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sidebarOpen]);

  // Badge, notifications and marking the open session seen.
  useAttention(sessions, activeId, setActiveId);

  function toggleProject(key: string) {
    const next = new Set(collapsed);
    next.has(key) ? next.delete(key) : next.add(key);
    setCollapsed(next);
    try {
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
    } catch {}
  }

  /** Opens the create page for a new session in `cwd`. */
  function startSession(cwd: string, agent: AgentKind) {
    setNewDraft(newSession(agent, cwd));
    setActiveId(undefined);
  }

  async function archive(id: string, archived: boolean) {
    if (archived && activeId === id) startSession(active?.cwd ?? "", active?.agent ?? "claude");
    await update({ sessionId: id, archived });
  }

  async function remove(id: string) {
    if (activeId === id) startSession(active?.cwd ?? "", active?.agent ?? "claude");
    await deleteSession(id).catch((e) => setConnectionError(message(e)));
  }

  /**
   * Sends the draft, or `again` (a retry) as a new message; while the agent is busy the backend
   * queues it instead. On the create page, this starts the session.
   */
  async function send(again?: { text: string; images?: string[] }) {
    const prompt = (again?.text ?? draft).trim();
    const images = again ? (again.images ?? []) : draftImages;
    if ((!prompt && !images.length) || starting) return;
    if (active && !canRun(active)) return;
    const queueing = !!active?.runId;
    // A retry of what's sitting in the composer (a failed start puts it back) consumes it.
    if (again === undefined || draft.trim() === prompt) {
      setDraft("");
      setDraftImages([]);
    }
    if (!queueing) {
      setStarting(true);
      followLatest();
    }
    try {
      if (active) {
        const s = await sendMessage(active.id, prompt, images);
        setSessions((all) => upsert(all, s));
      } else {
        // A retry from the create page shouldn't stack up errors from earlier attempts.
        setNewDraft((d) => ({ ...d, blocks: [] }));
        const s = await startSessionCall({
          agent: newDraft.agent,
          cwd: newDraft.cwd.trim(),
          branch: newDraft.branch ?? undefined,
          worktree: !!newDraft.worktree,
          newBranch: newDraft.worktree ? newDraft.newBranch?.trim() || undefined : undefined,
          modelChoice: newDraft.modelChoice ?? undefined,
          effort: newDraft.effort ?? undefined,
          permissionMode: newDraft.permissionMode ?? undefined,
          prompt,
          images,
        });
        setSessions((all) => upsert(all, s));
        setActiveId(s.id);
        setNewDraft(newSession(s.agent, s.cwd));
      }
    } catch (e) {
      if (again === undefined) {
        setDraft(prompt);
        setDraftImages(images);
      }
      const err = ApiError.from(e);
      if (!active) setNewDraft((d) => ({ ...d, blocks: [{ kind: "error", text: err.message, retryText: prompt }] }));
      // A message the agent couldn't take is in the transcript already, with a Retry.
      else if (err.code !== "failed") setConnectionError(err.message);
    } finally {
      setStarting(false);
    }
  }

  /** Sends `prompt` to the active session as a message, queued while its turn runs, then shows the chat. */
  async function ask(prompt: string) {
    if (!active) return;
    const s = await sendMessage(active.id, prompt);
    setSessions((all) => upsert(all, s));
    setView("chat");
    followLatest();
  }

  async function decide(requestId: string, decision: Decision, answers?: Record<string, string>) {
    if (!active) return;
    try {
      await answerApproval({ sessionId: active.id, requestId, decision, answers });
    } catch (e) {
      const err = ApiError.from(e);
      // Gone (the run ended, or the agent withdrew it) is shown by the transcript; anything
      // else was refused (e.g. an incomplete answer), and the agent is still waiting.
      if (err.code !== "not_found") setConnectionError(err.message);
    }
  }

  const update = (args: Parameters<typeof updateSession>[0]) =>
    updateSession(args)
      .then((s) => setSessions((all) => upsert(all, s)))
      .catch((e) => setConnectionError(message(e)));

  /** Sends a message again as a new turn (the agent still remembers the earlier one). */
  function retry(text?: string, images?: string[]) {
    const last = [...(transcript?.blocks ?? [])].reverse().find((b) => b.kind === "user");
    if (last?.kind !== "user" && text === undefined) return;
    // A failed message only remembers its text; its images are the last message's.
    const sameAsLast = last?.kind === "user" && (text === undefined || text === last.text);
    void send({ text: text ?? (last?.kind === "user" ? last.text : ""), images: images ?? (sameAsLast ? last.images : undefined) });
  }

  if (loading) {
    return (
      <div data-tauri-drag-region className="flex h-dvh items-center justify-center gap-2 text-sm text-muted-foreground">
        <Spinner className="size-5" /> Connecting…
      </div>
    );
  }

  const running = !!active?.runId;
  // Approvals left open by a run that is over (e.g. the backend restarted) can't be answered.
  const blocks = running ? (transcript?.blocks ?? []) : expireApprovals(transcript?.blocks ?? []);
  // Each block's turn (the user's messages so far), so images are only linked within one turn.
  const turns: number[] = [];
  for (const b of blocks) turns.push((turns[turns.length - 1] ?? 0) + (b.kind === "user" ? 1 : 0));
  const lastTurn = turns[turns.length - 1] ?? 0;
  const recentProjects = groupByProject(sessions)
    .map(([key]) => key)
    .filter((k) => k && k !== newDraft.cwd.trim())
    .slice(0, 5);

  return (
    <div className="flex h-dvh bg-background text-foreground">
      {/* Fixed in the top bar so it stays put whether the sidebar is shown or not. */}
      <div className="fixed top-0 z-40 flex h-(--title-bar-height) items-center" style={{ left: macTitleBar ? "calc(var(--traffic-lights-end) + 4px)" : 12 }}>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-muted-foreground"
          title={sidebarOpen ? "Hide sidebar (Ctrl/Cmd+B)" : "Show sidebar (Ctrl/Cmd+B)"}
          aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          aria-expanded={sidebarOpen}
          onClick={() => setSidebarOpen(!sidebarOpen)}
        >
          <PanelLeft />
        </Button>
      </div>

      {sidebarOpen && (
      <Sidebar
        width={sidebarWidth}
        onWidth={setSidebarWidth}
        onOpenSettings={() => setSettingsOpen(true)}
        sessions={sessions}
        activeId={activeId}
        collapsed={collapsed}
        onSelect={(id, seq) => {
          setActiveId(id);
          setReveal(seq === undefined ? undefined : { id, seq });
        }}
        searchFocus={searchFocus}
        onNew={() => startSession(active?.cwd ?? newDraft.cwd, active?.agent ?? newDraft.agent)}
        onNewInProject={startSession}
        onToggleProject={toggleProject}
        onArchive={(id, archived) => void archive(id, archived)}
        onDelete={(id) => void remove(id)}
        onRename={(id, title) => void update({ sessionId: id, title })}
        onOpenCleanup={() => setCleanupOpen(true)}
        tasks={tasks.tasks}
        onToggleTasks={() => setTasksOpen((v) => !v)}
      />
      )}

      {active && continuing?.kind === "fork" && (
        <ForkDialog
          session={active}
          seq={continuing.seq}
          text={continuing.text}
          onClose={() => setContinuing(undefined)}
          onStarted={(s) => (setSessions((all) => upsert(all, s)), setActiveId(s.id), setView("chat"), setContinuing(undefined))}
        />
      )}
      {active && continuing?.kind === "handoff" && (
        <HandoffDialog
          session={active}
          agents={agents}
          onClose={() => setContinuing(undefined)}
          onStarted={(s) => (setSessions((all) => upsert(all, s)), setActiveId(s.id), setView("chat"), setContinuing(undefined))}
        />
      )}

      {settingsOpen && <SettingsDialog onClose={() => setSettingsOpen(false)} />}

      {cleanupOpen && (
        <WorktreeCleanup sessions={sessions} onClose={() => setCleanupOpen(false)} />
      )}

      <main className="flex min-w-96 flex-1 flex-col">
        <ConnectionBanner inset={!sidebarOpen} />
        {!inTauri && connectionError?.includes("different version") && (
          <div className="flex items-center gap-3 border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive" role="alert">
            <p className="min-w-0 flex-1">{connectionError}</p>
            <Button size="xs" onClick={() => location.reload()}>
              Reload page
            </Button>
          </div>
        )}
        {!active && connectionError && <p className="p-6 text-sm text-destructive">{connectionError}</p>}
        {!active && macTitleBar && <div data-tauri-drag-region className="h-(--title-bar-height) shrink-0" />}
        {!active && (
          <NewSessionPage
            session={newDraft}
            agents={agents}
            draft={draft}
            onDraft={setDraft}
            images={draftImages}
            onImages={setDraftImages}
            onSend={() => void send()}
            starting={starting}
            recentProjects={recentProjects}
            onChange={(p) => setNewDraft((d) => ({ ...d, ...p }))}
            indicator={<UsageIndicator usage={planUsage} budget={budget} />}
            error={connectionError}
            paused={paused}
          />
        )}
        {active && (
          <>
            <SessionHeader session={active} inset={!sidebarOpen} view={view} onView={setView} origin={sessions.find((s) => s.id === active.origin?.sessionId)} onOpenOrigin={setActiveId} onHandoff={!running && !starting && !active.workdirRemoved && active.lastSeq > 0 ? () => setContinuing({ kind: "handoff" }) : undefined} changedFiles={diffMode === "commits" ? undefined : new Set(diff.diff?.files.map((f) => f.path)).size} />
            {showChanges && <DiffView session={active} state={diff} mode={diffMode} onMode={setDiffMode} commit={diffCommit} onCommit={setDiffCommit} review={review} onAsk={canRun(active) ? ask : undefined} />}
            {showFiles && <FileBrowser key={active.id} session={active} />}
            {view === "chat" && (
            <>

            <TranscriptContext.Provider
              value={{
                decide,
                retry: running || starting ? undefined : retry,
                agent: active.agent,
                outputs: running ? live.outputs : undefined,
                tasks: tasks.tasks,
                forwarding,
                showTask: (id) => (setTasksOpen(true), setViewingTask({ id })),
                showSession: setActiveId,
                fork: active.cwd.trim() ? (seq, text) => setContinuing({ kind: "fork", seq, text }) : undefined,
                inRepo: !!active.gitBranch,
                images: imageLinks,
              }}
            >
            <div className="relative min-h-0 flex-1">
              <div ref={scrollRef} onScroll={onScroll} className="h-full overflow-y-auto">
                <div ref={contentRef} className="mx-auto flex max-w-3xl flex-col gap-4 px-5 py-6 select-text">
                  {connectionError && (
                    <p className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                      {connectionError}
                    </p>
                  )}
                  {blocks.map((b, i) => (
                    // Takes no space of its own; it only lets a search hit find the block.
                    <div key={i} data-block={i} className="contents">
                      <BlockPlace.Provider value={{ turn: turns[i], index: i }}>
                        <BlockView block={b} live={running} />
                      </BlockPlace.Provider>
                    </div>
                  ))}
                  {running && live.thinking && <BlockView block={{ kind: "thinking", text: live.thinking.text, streaming: true }} live />}
                  {running && live.text && (
                    <BlockPlace.Provider value={{ turn: lastTurn, index: blocks.length }}>
                      <BlockView block={{ kind: "text", text: live.text.text }} live />
                    </BlockPlace.Provider>
                  )}
                  {(running || starting) &&
                    (awaitingApproval(active) ? (
                      <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                        <ShieldAlert className="size-4 animate-pulse text-primary" />{" "}
                        {blocks.some((b) => b.kind === "approval" && !b.decision && b.toolName === "AskUserQuestion")
                          ? "Waiting for your answer"
                          : "Waiting for your approval"}
                      </div>
                    ) : (
                      <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                        <Spinner className="text-primary" /> Working…
                      </div>
                    ))}
                </div>
              </div>
              {showJump && (
                <Button
                  variant="outline"
                  size="sm"
                  className="absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-popover shadow-md hover:bg-accent dark:bg-popover dark:hover:bg-accent"
                  onClick={followLatest}
                >
                  <ArrowDown data-icon="inline-start" />
                  Jump to latest
                </Button>
              )}
            </div>
            </TranscriptContext.Provider>

            <div className="mx-auto w-full max-w-3xl px-5 pb-5">
              <Composer
                draft={draft}
                onDraft={setDraft}
                images={draftImages}
                onImages={setDraftImages}
                onSend={() => void send()}
                queued={active.queued}
                onSendQueued={(id) => {
                  followLatest();
                  sendQueued(active.id, id)
                    .then((s) => setSessions((all) => upsert(all, s)))
                    .catch((e) => setConnectionError(message(e)));
                }}
                onRemoveQueued={(id, edit) => {
                  const q = active.queued.find((m) => m.id === id);
                  // Editing takes it back into the composer, after anything already there.
                  if (edit && q) {
                    setDraft((d) => (d.trim() ? `${d.trimEnd()}\n\n${q.text}` : q.text));
                    setDraftImages((imgs) => [...imgs, ...(q.images ?? [])]);
                  }
                  removeQueued(active.id, id)
                    .then((s) => setSessions((all) => upsert(all, s)))
                    .catch((e) => setConnectionError(message(e)));
                }}
                onStop={() => void stopSession(active.id).catch((e) => setConnectionError(message(e)))}
                running={running}
                starting={starting}
                blockedReason={canRun(active) ? paused : "This session can't be continued"}
                placeholder={active.workdirRemoved ? "This session's worktree was removed" : "Message the agent…"}
                agents={agents}
                agent={active.agent}
                cwd={active.workdir ?? active.cwd}
                agentLocked
                onAgent={() => {}}
                indicator={<UsageIndicator context={active.context} usage={planUsage} budget={budget} />}
                modelChoice={active.modelChoice ?? undefined}
                onModel={(modelChoice) => void update({ sessionId: active.id, modelChoice })}
                effort={active.effort ?? undefined}
                onEffort={(effort) => void update({ sessionId: active.id, effort })}
                permissionMode={active.permissionMode ?? undefined}
                onPermissionMode={(permissionMode) => void update({ sessionId: active.id, permissionMode })}
              />
            </div>
            </>
            )}
          </>
        )}
        <TerminalPanel project={projectKey(active?.cwd ?? newDraft.cwd)} cwd={(active?.workdir ?? active?.cwd ?? newDraft.cwd).trim()} />
      </main>

      {preview && <BrowserPanel preview={preview} width={browserWidth} onWidth={setBrowserWidth} onClose={closePreview} />}

      {tasksOpen && (
        <TasksPanel
          width={tasksWidth}
          onWidth={setTasksWidth}
          tasks={tasks.tasks}
          error={tasks.error}
          refresh={() => void tasks.refresh()}
          onClose={() => setTasksOpen(false)}
          viewing={viewingTask}
          onViewing={setViewingTask}
        />
      )}
    </div>
  );
}
