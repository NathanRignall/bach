// Sessions are kept by the backend: `Session` (generated) says what a session is and where it
// stands, and its transcript is a list of entries. This folds those entries into the blocks the
// transcript renders.
import type { AgentEvent, AgentKind, Decision, Entry, LogEntry, Session } from "./api";

export type Block =
  | { kind: "user"; text: string }
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | ToolBlock
  | ApprovalBlock
  | { kind: "error"; text: string; /** The message that failed to start, so Retry resends that one. */ retryText?: string }
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
  /** When the call started, to show how long it has been running. */
  startedAt?: number;
  task?: TaskInfo;
  /** What a sub-agent did while this call ran. */
  children?: Block[];
}

/** A permission request from the agent; `decision` is unset while it waits for the user. */
export interface ApprovalBlock {
  kind: "approval";
  requestId: string;
  toolName: string;
  input: unknown;
  description?: string;
  reason?: string;
  rules: string[];
  directories: string[];
  decision?: Decision | "expired";
  /** What the user answered, for a question from the agent. */
  answers?: Record<string, string>;
}

/** A session's transcript as far as it has been loaded: its blocks, and the last entry folded in. */
export interface Transcript {
  blocks: Block[];
  seq: number;
}

/** What the create page sets up before the first message makes it a session. */
export interface NewSession {
  agent: AgentKind;
  cwd: string;
  branch?: string | null;
  worktree?: boolean;
  /** The worktree's branch name; the backend makes one up from the message when empty. */
  newBranch?: string;
  modelChoice?: string | null;
  permissionMode?: string | null;
  /** Why the last attempt to start it failed. */
  blocks: Block[];
}

export const isSubagent = (b: ToolBlock) => !!b.task || b.name === "Agent" || b.name === "Task";

// crypto.randomUUID needs a secure context, which a page opened over plain http from another host isn't.
export const newId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;

/** New sessions start in Claude Code's auto mode; the picker can change it before the first message. */
export const newSession = (agent: AgentKind, cwd = ""): NewSession => ({ agent, cwd, permissionMode: "auto", blocks: [] });

// Sessions saved before folders were mandatory may have none but can still be resumed.
export const canRun = (s: Session) => !s.workdirRemoved && (!!s.cwd.trim() || !!s.agentSessionId);

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

export const awaitingApproval = (s: Session) => s.openApprovals.length > 0;

/** Most recently active first, like the backend lists them. */
export const byActivity = (a: Session, b: Session) => b.updatedAt - a.updatedAt;

/** Approvals still open when a run ends (or after a restart) can no longer be answered. */
export const expireApprovals = (blocks: Block[]): Block[] =>
  blocks.map((b) => (b.kind === "approval" && !b.decision ? { ...b, decision: "expired" } : b));

function mapTool(blocks: Block[], id: string, f: (t: ToolBlock) => ToolBlock): Block[] {
  return blocks.map((b) => (b.kind === "tool" && b.id === id ? f(b) : b));
}

/** Routes a block to its parent sub-agent's children when it has one, else the top level. */
function place(blocks: Block[], parent: string | undefined, add: (list: Block[]) => Block[]): Block[] {
  if (!parent || !blocks.some((b) => b.kind === "tool" && b.id === parent)) return add(blocks);
  return mapTool(blocks, parent, (t) => ({ ...t, children: add(t.children ?? []) }));
}

function applyAgentEvent(blocks: Block[], e: AgentEvent, at: number): Block[] {
  switch (e.type) {
    case "session":
    // Usage readings are shown from the session and the account, not in the transcript.
    case "context":
    case "context_windows":
    case "limits":
      return blocks;
    case "text":
      return place(blocks, e.parent, (l) => [...l, { kind: "text", text: e.text }]);
    case "thinking":
      return [...blocks, { kind: "thinking", text: e.text }];
    case "tool_use":
      return place(blocks, e.parent, (l) => [...l, { kind: "tool", id: e.id, name: e.name, input: e.input, startedAt: at }]);
    case "tool_result": {
      const done = (t: ToolBlock): ToolBlock => ({ ...t, output: e.output, isError: e.isError });
      return e.parent ? place(blocks, e.parent, (l) => mapTool(l, e.id, done)) : mapTool(blocks, e.id, done);
    }
    case "task": {
      const patch: TaskInfo = {
        status: e.status,
        title: e.title,
        agentType: e.agentType,
        activity: e.activity,
        toolUses: e.toolUses,
        tokens: e.tokens,
        durationMs: e.durationMs,
        summary: e.summary,
        background: e.background,
      };
      // Keep only what changed so later events don't blank earlier fields.
      const changed = Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined));
      return mapTool(blocks, e.id, (t) => ({ ...t, task: { ...t.task, ...changed } }));
    }
    case "approval":
      return [
        ...blocks,
        {
          kind: "approval",
          requestId: e.requestId,
          toolName: e.toolName,
          input: e.input,
          description: e.description ?? undefined,
          reason: e.reason ?? undefined,
          rules: e.rules,
          directories: e.directories ?? [],
        },
      ];
    case "approval_cancelled":
      return blocks.map((b) => (b.kind === "approval" && b.requestId === e.requestId && !b.decision ? { ...b, decision: "expired" } : b));
    case "error":
      return [...blocks, { kind: "error", text: e.message }];
    case "raw":
      return [...blocks, { kind: "text", text: e.line }];
    case "cancelled":
      return [...expireApprovals(blocks), { kind: "note", text: "Stopped" }];
    case "done":
      return expireApprovals(blocks);
  }
}

function applyEntry(blocks: Block[], entry: Entry, at: number): Block[] {
  switch (entry.type) {
    case "user":
      return [...blocks, { kind: "user", text: entry.text }];
    case "agent":
      return applyAgentEvent(blocks, entry.event, at);
    case "decision":
      return blocks.map((b) =>
        b.kind === "approval" && b.requestId === entry.requestId && !b.decision
          ? { ...b, decision: entry.decision, answers: entry.answers }
          : b,
      );
    case "failed":
      return [...blocks, { kind: "error", text: entry.message, retryText: entry.retryText }];
    case "imported":
      return [...blocks, ...expireApprovals(entry.blocks as unknown as Block[])];
  }
}

/** Folds entries into a transcript. Entries it already has are skipped; a gap means it's stale. */
export function applyEntries(t: Transcript, entries: LogEntry[]): Transcript {
  let { blocks, seq } = t;
  for (const e of entries) {
    if (e.seq <= seq) continue;
    blocks = applyEntry(blocks, e.entry, e.at);
    seq = e.seq;
  }
  return { blocks, seq };
}

export const emptyTranscript: Transcript = { blocks: [], seq: 0 };
