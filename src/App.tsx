import { useEffect, useRef, useState } from "react";
import { Square, ArrowUp } from "lucide-react";
import {
  AgentInfo,
  AgentKind,
  RunEvent,
  cancelRun,
  deleteSession,
  listAgents,
  listSessions,
  onAgentEvent,
  prepareWorkspace,
  saveSession,
  startRun,
} from "@/api";
import { BlockView } from "@/components/Transcript";
import { BranchBar } from "@/components/BranchBar";
import { WorktreeCleanup } from "@/components/WorktreeCleanup";
import { CwdInput } from "@/components/CwdInput";
import { Sidebar } from "@/components/Sidebar";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { Session, applyEvent, branchNameFor, canRun, isKept, isStarted, newSession, projectKey } from "@/session";

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
  // Events can arrive before startRun resolves and the session learns its run id.
  const early = useRef(new Map<string, RunEvent[]>());
  // What the backend already has, so unchanged sessions aren't re-saved (a save reorders history).
  const saved = useRef(new Map<string, Session>());
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    Promise.all([listAgents(), listSessions()])
      .then(([a, stored]) => {
        setAgents(a);
        const old = (stored as Session[]).map((s) => ({ ...s, runId: undefined }));
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
  useEffect(() => endRef.current?.scrollIntoView({ block: "end" }), [active?.blocks.length, active?.runId]);

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

  function startSession(cwd: string, agent: AgentKind) {
    const s = newSession(agent, cwd);
    setSessions((a) => [s, ...a.filter(isKept)]);
    setActiveId(s.id);
    // Make sure the new session is visible even if its project was collapsed.
    if (collapsed.has(projectKey(cwd))) toggleProject(projectKey(cwd));
  }

  async function remove(id: string) {
    const s = sessions.find((x) => x.id === id);
    if (s?.runId) await cancelRun(s.runId).catch(() => {});
    saved.current.delete(id);
    const rest = sessions.filter((x) => x.id !== id);
    const next = rest.length ? rest : [newSession(s?.agent ?? "claude", s?.cwd)];
    setSessions(next);
    if (activeId === id) setActiveId(next[0].id);
    await deleteSession(id).catch((e) => setConnectionError(String(e.message ?? e)));
  }

  async function send() {
    if (!active || !draft.trim() || active.runId || starting || !canRun(active)) return;
    const prompt = draft.trim();
    setDraft("");
    setStarting(true);
    const fail = (err: unknown) => {
      setDraft(prompt);
      const message = String((err as Error).message ?? err);
      patch(active.id, (s) => ({ ...s, blocks: [...s.blocks, { kind: "error", text: message }] }));
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

  if (loading) {
    return (
      <div className="flex h-dvh items-center justify-center gap-2 text-sm text-muted-foreground">
        <Spinner className="size-5" /> Connecting…
      </div>
    );
  }

  const running = !!active?.runId;
  const agentItems = agents.map((a) => ({ value: a.kind, label: a.installed ? a.name : `${a.name} (not installed)` }));

  return (
    <div className="flex h-dvh bg-background text-foreground">
      <Sidebar
        sessions={sessions}
        activeId={activeId}
        collapsed={collapsed}
        onSelect={setActiveId}
        onNew={() => startSession(active?.cwd ?? "", active?.agent ?? "claude")}
        onNewInProject={startSession}
        onToggleProject={toggleProject}
        onDelete={(id) => void remove(id)}
        onRename={(id, title) => patch(id, (s) => ({ ...s, title, titleEdited: true }))}
        onOpenCleanup={() => setCleanupOpen(true)}
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
        {active && (
          <>
            <header className="flex flex-col gap-2 border-b px-5 py-2.5">
              <div className="flex items-center gap-2">
                <CwdInput
                  key={active.id}
                  value={active.cwd}
                  locked={isStarted(active)}
                  onCommit={(cwd) => patch(active.id, (s) => ({ ...s, cwd, branch: undefined, worktree: false }))}
                />
              </div>
              <BranchBar key={active.id} session={active} onChange={(p) => patch(active.id, (s) => ({ ...s, ...p }))} />
            </header>

            <div className="flex-1 overflow-y-auto">
              <div className="mx-auto flex max-w-3xl flex-col gap-4 px-5 py-6">
                {connectionError && (
                  <p className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                    {connectionError}
                  </p>
                )}
                {active.blocks.length === 0 && (
                  <p className="mt-[18vh] text-center text-xl text-muted-foreground">
                    {canRun(active) ? "What should we work on?" : "Choose a project folder to get started"}
                  </p>
                )}
                {active.blocks.map((b, i) => (
                  <BlockView key={i} block={b} />
                ))}
                {(running || starting) && (
                  <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                    <Spinner className="text-primary" /> Working…
                  </div>
                )}
                <div ref={endRef} />
              </div>
            </div>

            <div className="mx-auto w-full max-w-3xl px-5 pb-5">
              <div className="rounded-2xl border bg-card p-2 shadow-sm focus-within:ring-2 focus-within:ring-ring/30">
                <Textarea
                  value={draft}
                  disabled={!canRun(active)}
                  placeholder={
                    canRun(active)
                      ? "Message the agent…"
                      : active.workdirRemoved
                        ? "This session's worktree was removed"
                        : "Choose a project folder first"
                  }
                  className="min-h-14 resize-none border-0 bg-transparent shadow-none focus-visible:ring-0 dark:bg-transparent"
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      void send();
                    }
                  }}
                />
                <div className="flex items-center justify-between gap-2 px-1 pt-1">
                  <Select
                    items={agentItems}
                    value={active.agent}
                    disabled={!!active.agentSessionId}
                    onValueChange={(v) => v && patch(active.id, (s) => ({ ...s, agent: v as AgentKind }))}
                  >
                    <SelectTrigger size="sm" aria-label="Agent">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {agents.map((a) => (
                        <SelectItem key={a.kind} value={a.kind} disabled={!a.installed}>
                          {a.installed ? a.name : `${a.name} (not installed)`}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>

                  {running ? (
                    <Button variant="outline" size="sm" onClick={() => void cancelRun(active.runId!)}>
                      <Square data-icon="inline-start" className="fill-current" />
                      Stop
                    </Button>
                  ) : (
                    <Button size="sm" onClick={() => void send()} disabled={!draft.trim() || !canRun(active) || starting}>
                      {starting ? <Spinner data-icon="inline-start" /> : <ArrowUp data-icon="inline-start" />}
                      Send
                    </Button>
                  )}
                </div>
              </div>
            </div>
          </>
        )}
      </main>
    </div>
  );
}
