import { useEffect, useRef, useState } from "react";
import { AgentInfo, AgentKind, RunEvent, cancelRun, listAgents, onAgentEvent, startRun } from "./api";

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

let counter = 0;
const newSession = (agent: AgentKind, cwd = ""): Session => ({
  id: `s${++counter}`,
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
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    listAgents()
      .then((a) => {
        setAgents(a);
        const first = newSession(a.find((x) => x.installed)?.kind ?? "claude");
        setSessions([first]);
        setActiveId(first.id);
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
            <button key={s.id} className={s.id === activeId ? "item active" : "item"} onClick={() => setActiveId(s.id)}>
              <span className={s.runId ? "dot live" : "dot"} />
              <span className="title">{s.title}</span>
              <span className="agent">{s.agent}</span>
            </button>
          ))}
        </div>
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
      return <div className="msg assistant">{block.text}</div>;
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
