import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  AgentInfo,
  AgentKind,
  RunEvent,
  canSwitchBackend,
  cancelRun,
  deleteSession,
  listAgents,
  listSessions,
  onAgentEvent,
  remoteUrl,
  saveSession,
  setBackend,
  startRun,
} from "./api";

type Block =
  | { kind: "user"; text: string }
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool"; id: string; name: string; input: unknown; output?: string; isError?: boolean }
  | { kind: "error"; text: string };

interface Session {
  id: string;
  title: string;
  agent: AgentKind;
  cwd: string;
  agentSessionId?: string;
  runId?: string;
  blocks: Block[];
}

// crypto.randomUUID needs a secure context, which a page opened over plain http from another host isn't.
const newId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;

const newSession = (agent: AgentKind, cwd = ""): Session => ({
  id: newId(),
  title: "New session",
  agent,
  cwd,
  blocks: [],
});

function applyEvent(s: Session, e: RunEvent): Session {
  const blocks = [...s.blocks];
  switch (e.type) {
    case "session":
      return { ...s, agentSessionId: e.id };
    case "text":
      blocks.push({ kind: "text", text: e.text });
      break;
    case "thinking":
      blocks.push({ kind: "thinking", text: e.text });
      break;
    case "tool_use":
      blocks.push({ kind: "tool", id: e.id, name: e.name, input: e.input });
      break;
    case "tool_result": {
      const i = blocks.findIndex((b) => b.kind === "tool" && b.id === e.id);
      if (i >= 0) blocks[i] = { ...(blocks[i] as Extract<Block, { kind: "tool" }>), output: e.output, isError: e.is_error };
      break;
    }
    case "error":
      blocks.push({ kind: "error", text: e.message });
      break;
    case "raw":
      blocks.push({ kind: "text", text: e.line });
      break;
    case "done":
      return { ...s, runId: undefined };
  }
  return { ...s, blocks };
}

export function App() {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeId, setActiveId] = useState<string>();
  const [draft, setDraft] = useState("");
  const [connectionError, setConnectionError] = useState<string>();
  // Events can arrive before startRun resolves and the session learns its run id.
  const early = useRef(new Map<string, RunEvent[]>());
  // What the backend already has, so unchanged sessions aren't re-saved (a save reorders history).
  const saved = useRef(new Map<string, Session>());
  const [confirmDelete, setConfirmDelete] = useState<string>();
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
      .catch((e) => setConnectionError(String(e.message ?? e)));
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
  useEffect(() => endRef.current?.scrollIntoView({ block: "end" }), [active?.blocks.length]);

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

  async function remove(id: string) {
    const s = sessions.find((x) => x.id === id);
    if (s?.runId) await cancelRun(s.runId).catch(() => {});
    saved.current.delete(id);
    setConfirmDelete(undefined);
    const rest = sessions.filter((x) => x.id !== id);
    const next = rest.length ? rest : [newSession(s?.agent ?? "claude", s?.cwd)];
    setSessions(next);
    if (activeId === id) setActiveId(next[0].id);
    await deleteSession(id).catch((e) => setConnectionError(String(e.message ?? e)));
  }

  const patch = (id: string, f: (s: Session) => Session) =>
    setSessions((all) => all.map((s) => (s.id === id ? f(s) : s)));

  async function send() {
    if (!active || !draft.trim() || active.runId) return;
    const prompt = draft.trim();
    setDraft("");
    try {
      const runId = await startRun({
        agent: active.agent,
        prompt,
        cwd: active.cwd || undefined,
        sessionId: active.agentSessionId,
      });
      const buffered = early.current.get(runId) ?? [];
      early.current.delete(runId);
      patch(active.id, (s) =>
        buffered.reduce(applyEvent, {
          ...s,
          runId,
          title: s.blocks.length ? s.title : prompt.slice(0, 40),
          blocks: [...s.blocks, { kind: "user", text: prompt }],
        }),
      );
    } catch (err) {
      patch(active.id, (s) => ({ ...s, blocks: [...s.blocks, { kind: "error", text: String(err) }] }));
    }
  }

  return (
    <div className="app">
      <aside className="sidebar">
        <button
          className="new"
          onClick={() => {
            const s = newSession(active?.agent ?? "claude", active?.cwd);
            setSessions((a) => [s, ...a]);
            setActiveId(s.id);
          }}
        >
          + New session
        </button>
        <div className="list">
          {sessions.map((s) => (
            <div key={s.id} className={s.id === activeId ? "item active" : "item"}>
              <button className="open" onClick={() => setActiveId(s.id)}>
                <span className={s.runId ? "dot live" : "dot"} />
                <span className="title">{s.title}</span>
                <span className="agent">{s.agent}</span>
              </button>
              <button
                className={confirmDelete === s.id ? "del confirm" : "del"}
                title="Delete session"
                onClick={() => (confirmDelete === s.id ? void remove(s.id) : setConfirmDelete(s.id))}
                onBlur={() => setConfirmDelete(undefined)}
              >
                {confirmDelete === s.id ? "Delete?" : "×"}
              </button>
            </div>
          ))}
        </div>
        <BackendPicker />
      </aside>

      <main className="main">
        {!active && connectionError && <div className="msg error" style={{ padding: 24 }}>{connectionError}</div>}
        {active && (
          <>
            <header className="bar">
              <input
                className="cwd"
                placeholder="Working directory (defaults to app cwd)"
                value={active.cwd}
                onChange={(e) => patch(active.id, (s) => ({ ...s, cwd: e.target.value }))}
              />
            </header>
            <div className="transcript">
              {connectionError && <div className="msg error">{connectionError}</div>}
              {active.blocks.length === 0 && <div className="empty">What should we work on?</div>}
              {active.blocks.map((b, i) => (
                <BlockView key={i} block={b} />
              ))}
              <div ref={endRef} />
            </div>
            <div className="composer">
              <textarea
                value={draft}
                placeholder="Message the agent…"
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void send();
                  }
                }}
              />
              <div className="row">
                <select
                  value={active.agent}
                  disabled={!!active.agentSessionId}
                  onChange={(e) => patch(active.id, (s) => ({ ...s, agent: e.target.value as AgentKind }))}
                >
                  {agents.map((a) => (
                    <option key={a.kind} value={a.kind} disabled={!a.installed}>
                      {a.name}
                      {a.installed ? "" : " (not installed)"}
                    </option>
                  ))}
                </select>
                {active.runId ? (
                  <button onClick={() => void cancelRun(active.runId!)}>Stop</button>
                ) : (
                  <button className="primary" onClick={() => void send()} disabled={!draft.trim()}>
                    Send
                  </button>
                )}
              </div>
            </div>
          </>
        )}
      </main>
    </div>
  );
}

function BlockView({ block }: { block: Block }) {
  switch (block.kind) {
    case "user":
      return <div className="msg user">{block.text}</div>;
    case "text":
      return (
        <div className="msg assistant md">
          <Markdown remarkPlugins={[remarkGfm]} components={{ a: (props) => <a {...props} target="_blank" rel="noopener noreferrer" /> }}>
            {block.text}
          </Markdown>
        </div>
      );
    case "thinking":
      return <details className="thinking"><summary>Thinking</summary>{block.text}</details>;
    case "error":
      return <div className="msg error">{block.text}</div>;
    case "tool":
      return (
        <details className={block.isError ? "tool err" : "tool"}>
          <summary>
            <b>{block.name}</b> <code>{JSON.stringify(block.input).slice(0, 100)}</code>
            {block.output === undefined && <span className="spin"> …</span>}
          </summary>
          <pre>{JSON.stringify(block.input, null, 2)}</pre>
          {block.output !== undefined && <pre>{block.output}</pre>}
        </details>
      );
  }
}

function BackendPicker() {
  const [url, setUrl] = useState(remoteUrl ?? "ws://localhost:3421");
  const [mode, setMode] = useState(remoteUrl ? "remote" : "local");
  if (!canSwitchBackend) return <div className="backend">Agents on {remoteUrl}</div>;
  const changed = mode === "local" ? remoteUrl !== null : url !== remoteUrl;
  return (
    <div className="backend">
      <label>
        Agents run
        <select value={mode} onChange={(e) => setMode(e.target.value)}>
          <option value="local">on this machine</option>
          <option value="remote">on a remote bach-server</option>
        </select>
      </label>
      {mode === "remote" && <input value={url} onChange={(e) => setUrl(e.target.value)} spellCheck={false} />}
      {changed && <button onClick={() => setBackend(mode === "local" ? null : url.trim())}>Apply</button>}
    </div>
  );
}
