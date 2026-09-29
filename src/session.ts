import type { AgentKind, RunEvent } from "./api";

export type Block =
  | { kind: "user"; text: string }
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool"; id: string; name: string; input: unknown; output?: string; isError?: boolean }
  | { kind: "error"; text: string }
  | { kind: "note"; text: string };

export interface Session {
  id: string;
  title: string;
  /** The user named it, so the first prompt shouldn't. */
  titleEdited?: boolean;
  agent: AgentKind;
  /** The project folder; sessions are grouped by it. */
  cwd: string;
  /** Git branch to run on (or, with `worktree`, to branch from). Defaults to the current one. */
  branch?: string;
  /** Run in an isolated git worktree on a new branch. */
  worktree?: boolean;
  /** Where the agent actually runs once started (a worktree path, or `cwd`). */
  workdir?: string;
  /** The branch the session runs on once started. */
  gitBranch?: string;
  agentSessionId?: string;
  runId?: string;
  blocks: Block[];
}

// crypto.randomUUID needs a secure context, which a page opened over plain http from another host isn't.
const newId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;

export const newSession = (agent: AgentKind, cwd = ""): Session => ({
  id: newId(),
  title: "New session",
  agent,
  cwd,
  blocks: [],
});

/** Git settings and the project folder can't change once the session has started. */
export const isStarted = (s: Session) => !!s.agentSessionId || !!s.workdir;

/** e.g. "Fix the login bug!" -> "bach/fix-the-login-bug-a3f1" */
export function branchNameFor(prompt: string): string {
  const slug = prompt
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .split("-")
    .slice(0, 5)
    .join("-");
  return `bach/${slug || "session"}-${Math.random().toString(16).slice(2, 6)}`;
}

// Sessions saved before folders were mandatory may have none but can still be resumed.
export const canRun = (s: Session) => !!s.cwd.trim() || !!s.agentSessionId;

/** Empty sessions were never saved; they shouldn't pile up as the user moves around. */
export const isKept = (s: Session) => s.blocks.length > 0 || !!s.runId;

/** A session's project is its working directory. */
export const projectKey = (cwd: string) => cwd.trim().replace(/\/+$/, "");
export const projectName = (key: string) => (key ? (key.split("/").filter(Boolean).pop() ?? key) : "No project");

/** Groups keep the order of their first (most recent) session. */
export function groupByProject(sessions: Session[]): [string, Session[]][] {
  const groups = new Map<string, Session[]>();
  for (const s of sessions) {
    const key = projectKey(s.cwd);
    groups.set(key, [...(groups.get(key) ?? []), s]);
  }
  return [...groups];
}

export function applyEvent(s: Session, e: RunEvent): Session {
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
    case "cancelled":
      return { ...s, runId: undefined, blocks: [...blocks, { kind: "note", text: "Stopped" }] };
    case "done":
      return { ...s, runId: undefined };
  }
  return { ...s, blocks };
}
