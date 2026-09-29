import { useEffect, useReducer, useRef, useState } from "react";
import { ArrowDown, ShieldAlert } from "lucide-react";
import {
  AgentInfo,
  AgentKind,
  ApiError,
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
  sendMessage,
  startSession as startSessionCall,
  stopSession,
  updateSession,
} from "@/api";
import { BlockView, TranscriptContext } from "@/components/Transcript";
import { WorktreeCleanup } from "@/components/WorktreeCleanup";
import { TasksPanel, useTasks } from "@/components/TasksPanel";
import { UsageIndicator, usePlanUsage } from "@/components/UsageIndicator";
import { Composer } from "@/components/Composer";
import { NewSessionPage } from "@/components/NewSessionPage";
import { SessionHeader } from "@/components/SessionHeader";
import { Sidebar } from "@/components/Sidebar";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import {
  NewSession,
  Transcript,
  applyEntries,
  awaitingApproval,
  byActivity,
  canRun,
  emptyTranscript,
  expireApprovals,
  groupByProject,
  newSession,
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
  /** The open session; none means the create page. */
  const [activeId, setActiveId] = useState<string>();
  /** What the create page is setting up. */
  const [newDraft, setNewDraft] = useState<NewSession>(() => newSession("claude"));
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(true);
  const [starting, setStarting] = useState(false);
  const [connectionError, setConnectionError] = useState<string>();
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const [cleanupOpen, setCleanupOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const tasks = useTasks();
  const planUsage = usePlanUsage();
  // Transcripts of the sessions opened so far. Kept in a ref so event handlers see the latest
  // `seq` (to spot gaps); `rerender` shows changes.
  const transcripts = useRef(new Map<string, Transcript>());
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  // Transcripts being fetched, with the entries that arrived meanwhile.
  const fetching = useRef(new Map<string, LogEntry[]>());
  // Follow new output only while the reader is at the bottom; scrolling up pins the view.
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [showJump, setShowJump] = useState(false);

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
  const chatShown = !!active;
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

  // Something is waiting on the user: say so in the tab, in case it's in the background.
  const anyWaiting = sessions.some(awaitingApproval);
  useEffect(() => {
    document.title = anyWaiting ? "● Waiting for you — Bach" : "Bach";
  }, [anyWaiting]);

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

  async function remove(id: string) {
    if (activeId === id) startSession(active?.cwd ?? "", active?.agent ?? "claude");
    await deleteSession(id).catch((e) => setConnectionError(message(e)));
  }

  /** Sends the draft, or `again` (a retry) as a new message. On the create page, this starts the session. */
  async function send(again?: string) {
    const prompt = (again ?? draft).trim();
    if (!prompt || starting) return;
    if (active && (active.runId || !canRun(active))) return;
    // A retry of what's sitting in the composer (a failed start puts it back) consumes it.
    if (again === undefined || draft.trim() === prompt) setDraft("");
    setStarting(true);
    followLatest();
    try {
      if (active) {
        const s = await sendMessage(active.id, prompt);
        setSessions((all) => upsert(all, s));
      } else {
        // A retry from the create page shouldn't stack up errors from earlier attempts.
        setNewDraft((d) => ({ ...d, blocks: [] }));
        const s = await startSessionCall({
          agent: newDraft.agent,
          cwd: newDraft.cwd.trim(),
          branch: newDraft.branch ?? undefined,
          worktree: !!newDraft.worktree,
          modelChoice: newDraft.modelChoice ?? undefined,
          prompt,
        });
        setSessions((all) => upsert(all, s));
        setActiveId(s.id);
        setNewDraft(newSession(s.agent, s.cwd));
      }
    } catch (e) {
      if (again === undefined) setDraft(prompt);
      const err = ApiError.from(e);
      if (!active) setNewDraft((d) => ({ ...d, blocks: [{ kind: "error", text: err.message, retryText: prompt }] }));
      // A message the agent couldn't take is in the transcript already, with a Retry.
      else if (err.code !== "failed") setConnectionError(err.message);
    } finally {
      setStarting(false);
    }
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
  function retry(text?: string) {
    const last = [...(transcript?.blocks ?? [])].reverse().find((b) => b.kind === "user");
    const prompt = text ?? (last?.kind === "user" ? last.text : undefined);
    if (prompt) void send(prompt);
  }

  if (loading) {
    return (
      <div className="flex h-dvh items-center justify-center gap-2 text-sm text-muted-foreground">
        <Spinner className="size-5" /> Connecting…
      </div>
    );
  }

  const running = !!active?.runId;
  // Approvals left open by a run that is over (e.g. the backend restarted) can't be answered.
  const blocks = running ? (transcript?.blocks ?? []) : expireApprovals(transcript?.blocks ?? []);
  const recentProjects = groupByProject(sessions)
    .map(([key]) => key)
    .filter((k) => k && k !== newDraft.cwd.trim())
    .slice(0, 5);

  return (
    <div className="flex h-dvh bg-background text-foreground">
      <Sidebar
        sessions={sessions}
        activeId={activeId}
        collapsed={collapsed}
        onSelect={setActiveId}
        onNew={() => startSession(active?.cwd ?? newDraft.cwd, active?.agent ?? newDraft.agent)}
        onNewInProject={startSession}
        onToggleProject={toggleProject}
        onDelete={(id) => void remove(id)}
        onRename={(id, title) => void update({ sessionId: id, title })}
        onOpenCleanup={() => setCleanupOpen(true)}
        runningTasks={tasks.tasks.filter((t) => t.status === "running").length}
        onToggleTasks={() => setTasksOpen((v) => !v)}
      />

      {cleanupOpen && (
        <WorktreeCleanup sessions={sessions} onClose={() => setCleanupOpen(false)} />
      )}

      <main className="flex min-w-0 flex-1 flex-col">
        {!active && connectionError && <p className="p-6 text-sm text-destructive">{connectionError}</p>}
        {!active && (
          <NewSessionPage
            session={newDraft}
            agents={agents}
            draft={draft}
            onDraft={setDraft}
            onSend={() => void send()}
            starting={starting}
            recentProjects={recentProjects}
            onChange={(p) => setNewDraft((d) => ({ ...d, ...p }))}
            indicator={<UsageIndicator usage={planUsage} />}
            error={connectionError}
          />
        )}
        {active && (
          <>
            <SessionHeader session={active} />

            <TranscriptContext.Provider value={{ decide, retry: running || starting ? undefined : retry }}>
            <div className="relative min-h-0 flex-1">
              <div ref={scrollRef} onScroll={onScroll} className="h-full overflow-y-auto">
                <div ref={contentRef} className="mx-auto flex max-w-3xl flex-col gap-4 px-5 py-6">
                  {connectionError && (
                    <p className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                      {connectionError}
                    </p>
                  )}
                  {blocks.map((b, i) => (
                    <BlockView key={i} block={b} live={running} />
                  ))}
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
                  className="absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-background shadow-md"
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
                onSend={() => void send()}
                onStop={() => void stopSession(active.id).catch((e) => setConnectionError(message(e)))}
                running={running}
                starting={starting}
                blockedReason={canRun(active) ? undefined : "This session can't be continued"}
                placeholder={active.workdirRemoved ? "This session's worktree was removed" : "Message the agent…"}
                agents={agents}
                agent={active.agent}
                agentLocked
                onAgent={() => {}}
                indicator={<UsageIndicator context={active.context} usage={planUsage} />}
                modelChoice={active.modelChoice ?? undefined}
                onModel={(modelChoice) => void update({ sessionId: active.id, modelChoice })}
              />
            </div>
          </>
        )}
      </main>

      {tasksOpen && (
        <TasksPanel
          tasks={tasks.tasks}
          error={tasks.error}
          refresh={() => void tasks.refresh()}
          defaultCwd={(active?.workdir ?? active?.cwd ?? newDraft.cwd).trim()}
          onClose={() => setTasksOpen(false)}
        />
      )}
    </div>
  );
}
