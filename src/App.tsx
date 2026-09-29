import { useEffect, useRef, useState } from "react";
import { ArrowDown, ShieldAlert } from "lucide-react";
import {
  AgentInfo,
  AgentKind,
  ApprovalDecision,
  RunEvent,
  cancelRun,
  deleteSession,
  listAgents,
  listSessions,
  onAgentEvent,
  prepareWorkspace,
  respondApproval,
  saveSession,
  startRun,
} from "@/api";
import { BlockView, TranscriptContext } from "@/components/Transcript";
import { WorktreeCleanup } from "@/components/WorktreeCleanup";
import { TasksPanel, useTasks } from "@/components/TasksPanel";
import { Composer } from "@/components/Composer";
import { NewSessionPage } from "@/components/NewSessionPage";
import { SessionHeader } from "@/components/SessionHeader";
import { Sidebar } from "@/components/Sidebar";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import {
  Session,
  applyEvent,
  awaitingApproval,
  branchNameFor,
  canRun,
  decideApproval,
  expireApprovals,
  groupByProject,
  isDraft,
  isStarted,
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

export function App() {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeId, setActiveId] = useState<string>();
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(true);
  const [starting, setStarting] = useState(false);
  const [connectionError, setConnectionError] = useState<string>();
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const [cleanupOpen, setCleanupOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const tasks = useTasks();
  // Events can arrive before startRun resolves and the session learns its run id.
  const early = useRef(new Map<string, RunEvent[]>());
  // What the backend already has, so unchanged sessions aren't re-saved (a save reorders history).
  const saved = useRef(new Map<string, Session>());
  // Follow new output only while the reader is at the bottom; scrolling up pins the view.
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [showJump, setShowJump] = useState(false);

  useEffect(() => {
    Promise.all([listAgents(), listSessions()])
      .then(([a, stored]) => {
        setAgents(a);
        const old = (stored as Session[]).map((s) => ({ ...s, runId: undefined, blocks: expireApprovals(s.blocks) }));
        old.forEach((s) => saved.current.set(s.id, s));
        // Open on a fresh session; it is only saved once it has content.
        const fresh = newSession(old[0]?.agent ?? a.find((x) => x.installed)?.kind ?? "claude", old[0]?.cwd);
        setSessions([fresh, ...old]);
        setActiveId(fresh.id);
      })
      .catch((e) => setConnectionError(String(e.message ?? e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    const un = onAgentEvent((e) =>
      setSessions((all) => {
        if (!all.some((s) => s.runId === e.run_id)) {
          early.current.set(e.run_id, [...(early.current.get(e.run_id) ?? []), e]);
          return all;
        }
        return all.map((s) => (s.runId === e.run_id ? applyEvent(s, e) : s));
      }),
    );
    return () => void un.then((f) => f());
  }, []);

  const active = sessions.find((s) => s.id === activeId);

  useEffect(() => {
    const t = setTimeout(() => {
      for (const s of sessions) {
        if (!s.blocks.length || saved.current.get(s.id) === s) continue;
        saved.current.set(s.id, s);
        saveSession({ ...s, runId: undefined }).catch((e) => setConnectionError(String(e.message ?? e)));
      }
    }, 400);
    return () => clearTimeout(t);
  }, [sessions]);

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
  const chatShown = !!active && (isStarted(active) || !!active.workdirRemoved);
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

  const patch = (id: string, f: (s: Session) => Session) =>
    setSessions((all) => all.map((s) => (s.id === id ? f(s) : s)));

  function toggleProject(key: string) {
    const next = new Set(collapsed);
    next.has(key) ? next.delete(key) : next.add(key);
    setCollapsed(next);
    try {
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
    } catch {}
  }

  /** Opens the create page, reusing the one draft rather than piling up new ones. */
  function startSession(cwd: string, agent: AgentKind) {
    const draft = sessions.find(isDraft);
    if (draft) {
      patch(draft.id, (s) => ({ ...s, cwd, agent, branch: undefined, worktree: false, blocks: [] }));
      setActiveId(draft.id);
      return;
    }
    const s = newSession(agent, cwd);
    setSessions((a) => [s, ...a]);
    setActiveId(s.id);
  }

  async function remove(id: string) {
    const s = sessions.find((x) => x.id === id);
    if (s?.runId) await cancelRun(s.runId).catch(() => {});
    saved.current.delete(id);
    const rest = sessions.filter((x) => x.id !== id);
    const draft = rest.find(isDraft) ?? newSession(s?.agent ?? "claude", s?.cwd);
    setSessions(rest.includes(draft) ? rest : [draft, ...rest]);
    if (activeId === id) setActiveId(draft.id);
    await deleteSession(id).catch((e) => setConnectionError(String(e.message ?? e)));
  }

  /** Sends the draft, or `again` (a retry) as a new message in this session. */
  async function send(again?: string) {
    const prompt = (again ?? draft).trim();
    if (!active || !prompt || active.runId || starting || !canRun(active)) return;
    // A retry of what's sitting in the composer (a failed start puts it back) consumes it.
    if (again === undefined || draft.trim() === prompt) setDraft("");
    setStarting(true);
    followLatest();
    // A retry from the create page shouldn't stack up errors from earlier attempts.
    if (!isStarted(active)) patch(active.id, (s) => ({ ...s, blocks: s.blocks.filter((b) => b.kind !== "error") }));
    const fail = (err: unknown) => {
      if (again === undefined) setDraft(prompt);
      const message = String((err as Error).message ?? err);
      patch(active.id, (s) => ({ ...s, blocks: [...s.blocks, { kind: "error", text: message, retryText: prompt }] }));
    };
    try {
      // First message: settle where the agent runs (branch switch or a fresh worktree).
      let workdir = active.workdir;
      if (!isStarted(active) && active.cwd.trim()) {
        const ws = await prepareWorkspace({
          cwd: active.cwd.trim(),
          branch: active.branch,
          worktree: !!active.worktree,
          newBranch: active.worktree ? branchNameFor(prompt) : undefined,
        }).catch((e) => void fail(e));
        if (!ws) return;
        workdir = ws.workdir;
        patch(active.id, (s) => ({ ...s, workdir: ws.workdir, gitBranch: ws.branch ?? undefined, worktree: ws.worktree }));
      }

      const runId = await startRun({
        agent: active.agent,
        prompt,
        cwd: (workdir ?? active.cwd).trim() || undefined,
        sessionId: active.agentSessionId,
        model: active.agent === "claude" && active.modelChoice !== "default" ? active.modelChoice : undefined,
        allowedTools: active.allowRules,
        sessionKey: active.id,
      });
      const buffered = early.current.get(runId) ?? [];
      early.current.delete(runId);
      patch(active.id, (s) =>
        buffered.reduce(applyEvent, {
          ...s,
          runId,
          title: s.blocks.length || s.titleEdited ? s.title : prompt.slice(0, 40),
          blocks: [...s.blocks, { kind: "user", text: prompt }],
        }),
      );
    } catch (err) {
      fail(err);
    } finally {
      setStarting(false);
    }
  }

  async function decide(requestId: string, decision: ApprovalDecision, answers?: Record<string, string>) {
    const s = active;
    if (!s?.runId) return;
    try {
      await respondApproval({ runId: s.runId, requestId, decision, answers });
      patch(s.id, (x) => decideApproval(x, requestId, decision, answers));
    } catch (e) {
      const message = String((e as Error).message ?? e);
      if (!/no longer pending|already finished/.test(message)) {
        // Rejected (e.g. an incomplete answer): the agent is still waiting, so leave it open.
        setConnectionError(message);
        return;
      }
      // The run moved on without it (finished, stopped, or the agent withdrew the request).
      patch(s.id, (x) => ({
        ...x,
        blocks: x.blocks.map((b) => (b.kind === "approval" && b.requestId === requestId && !b.decision ? { ...b, decision: "expired" } : b)),
      }));
    }
  }

  /** Sends a message again as a new turn (the agent still remembers the earlier one). */
  function retry(text?: string) {
    const last = [...(active?.blocks ?? [])].reverse().find((b) => b.kind === "user");
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
  // A session is set up on the create page until it has started (also after a failed start).
  const isNew = !!active && !isStarted(active) && !active.workdirRemoved;
  const recentProjects = groupByProject(sessions.filter((s) => !isDraft(s)))
    .map(([key]) => key)
    .filter((k) => k && k !== active?.cwd.trim())
    .slice(0, 5);

  return (
    <div className="flex h-dvh bg-background text-foreground">
      <Sidebar
        sessions={sessions.filter((s) => !isDraft(s))}
        activeId={activeId}
        collapsed={collapsed}
        onSelect={setActiveId}
        onNew={() => startSession(active?.cwd ?? "", active?.agent ?? "claude")}
        onNewInProject={startSession}
        onToggleProject={toggleProject}
        onDelete={(id) => void remove(id)}
        onRename={(id, title) => patch(id, (s) => ({ ...s, title, titleEdited: true }))}
        onOpenCleanup={() => setCleanupOpen(true)}
        runningTasks={tasks.tasks.filter((t) => t.status === "running").length}
        onToggleTasks={() => setTasksOpen((v) => !v)}
      />

      {cleanupOpen && (
        <WorktreeCleanup
          sessions={sessions}
          onClose={() => setCleanupOpen(false)}
          // Their folder is gone: keep the transcript readable, but they can't be continued.
          onRemoved={(path) => setSessions((all) => all.map((s) => (s.workdir === path ? { ...s, workdirRemoved: true } : s)))}
        />
      )}

      <main className="flex min-w-0 flex-1 flex-col">
        {!active && connectionError && <p className="p-6 text-sm text-destructive">{connectionError}</p>}
        {active && isNew && (
          <NewSessionPage
            key={active.id}
            session={active}
            agents={agents}
            draft={draft}
            onDraft={setDraft}
            onSend={() => void send()}
            starting={starting}
            recentProjects={recentProjects}
            onChange={(p) => patch(active.id, (s) => ({ ...s, ...p }))}
            error={connectionError}
          />
        )}
        {active && !isNew && (
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
                  {active.blocks.map((b, i) => (
                    <BlockView key={i} block={b} live={running} />
                  ))}
                  {(running || starting) &&
                    (awaitingApproval(active) ? (
                      <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                        <ShieldAlert className="size-4 animate-pulse text-primary" />{" "}
                        {active.blocks.some((b) => b.kind === "approval" && !b.decision && b.toolName === "AskUserQuestion")
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
                onStop={() => void cancelRun(active.runId!)}
                running={running}
                starting={starting}
                blockedReason={canRun(active) ? undefined : "This session can't be continued"}
                placeholder={active.workdirRemoved ? "This session's worktree was removed" : "Message the agent…"}
                agents={agents}
                agent={active.agent}
                agentLocked
                onAgent={() => {}}
                modelChoice={active.modelChoice}
                onModel={(modelChoice) => patch(active.id, (s) => ({ ...s, modelChoice }))}
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
          defaultCwd={(active?.workdir ?? active?.cwd ?? "").trim()}
          onClose={() => setTasksOpen(false)}
        />
      )}
    </div>
  );
}
