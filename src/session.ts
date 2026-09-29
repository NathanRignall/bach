import type { AgentKind, RunEvent } from "./api";

export type Block =
  | { kind: "user"; text: string }
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | ToolBlock
  | { kind: "error"; text: string }
  | { kind: "note"; text: string };

/** Progress of a sub-agent or background task started by a tool call. */
export interface TaskInfo {
  status?: string;
  title?: string;
  agentType?: string;
  activity?: string;
  toolUses?: number;
  tokens?: number;
  durationMs?: number;
  summary?: string;
  background?: boolean;
}

export interface ToolBlock {
  kind: "tool";
  id: string;
  name: string;
  input: unknown;
  output?: string;
  isError?: boolean;
  /** When the call started (client clock), to show how long it has been running. */
  startedAt?: number;
  task?: TaskInfo;
  /** What a sub-agent did while this call ran. */
  children?: Block[];
}

export const isSubagent = (b: ToolBlock) => !!b.task || b.name === "Agent" || b.name === "Task";

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
  /** The worktree was cleaned up, so the session can be read but not continued. */
  workdirRemoved?: boolean;
  /** The branch the session runs on once started. */
  gitBranch?: string;
  agentSessionId?: string;
  /** Model choice for Claude Code: "default" or an alias like "opus". */
  modelChoice?: string;
  /** The model the agent reported using on its latest run. */
  model?: string;
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
export const canRun = (s: Session) => !s.workdirRemoved && (!!s.cwd.trim() || !!s.agentSessionId);

/**
 * A draft has nothing sent yet. It lives on the create page and isn't listed in the sidebar,
 * so opening the app or pressing "New session" never adds an empty entry.
 */
export const isDraft = (s: Session) => !isStarted(s) && !s.blocks.some((b) => b.kind === "user");

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

function mapTool(blocks: Block[], id: string, f: (t: ToolBlock) => ToolBlock): Block[] {
  return blocks.map((b) => (b.kind === "tool" && b.id === id ? f(b) : b));
}

/** Routes a block to its parent sub-agent's children when it has one, else the top level. */
function place(blocks: Block[], parent: string | undefined, add: (list: Block[]) => Block[]): Block[] {
  if (!parent || !blocks.some((b) => b.kind === "tool" && b.id === parent)) return add(blocks);
  return mapTool(blocks, parent, (t) => ({ ...t, children: add(t.children ?? []) }));
}

export function applyEvent(s: Session, e: RunEvent): Session {
  const blocks = s.blocks;
  switch (e.type) {
    case "session":
      return { ...s, agentSessionId: e.id, model: e.model ?? s.model };
    case "text":
      return { ...s, blocks: place(blocks, e.parent, (l) => [...l, { kind: "text", text: e.text }]) };
    case "thinking":
      return { ...s, blocks: [...blocks, { kind: "thinking", text: e.text }] };
    case "tool_use":
      return {
        ...s,
        blocks: place(blocks, e.parent, (l) => [
          ...l,
          { kind: "tool", id: e.id, name: e.name, input: e.input, startedAt: Date.now() },
        ]),
      };
    case "tool_result": {
      const done = (t: ToolBlock): ToolBlock => ({ ...t, output: e.output, isError: e.is_error });
      return {
        ...s,
        blocks: e.parent
          ? place(blocks, e.parent, (l) => mapTool(l, e.id, done))
          : mapTool(blocks, e.id, done),
      };
    }
    case "task": {
      const patch: TaskInfo = {
        status: e.status,
        title: e.title,
        agentType: e.agent_type,
        activity: e.activity,
        toolUses: e.tool_uses,
        tokens: e.tokens,
        durationMs: e.duration_ms,
        summary: e.summary,
        background: e.background,
      };
      // Keep only what changed so later events don't blank earlier fields.
      const changed = Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined));
      return { ...s, blocks: mapTool(blocks, e.id, (t) => ({ ...t, task: { ...t.task, ...changed } })) };
    }
    case "error":
      return { ...s, blocks: [...blocks, { kind: "error", text: e.message }] };
    case "raw":
      return { ...s, blocks: [...blocks, { kind: "text", text: e.line }] };
    case "cancelled":
      return { ...s, runId: undefined, blocks: [...blocks, { kind: "note", text: "Stopped" }] };
    case "done":
      return { ...s, runId: undefined };
  }
}
