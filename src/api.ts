import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type AgentKind = "claude" | "codex" | "opencode";

export interface AgentInfo {
  kind: AgentKind;
  name: string;
  installed: boolean;
}

export type AgentEvent =
  | { type: "session"; id: string }
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; id: string; output: string; is_error: boolean }
  | { type: "done"; cost_usd: number | null; is_error: boolean }
  | { type: "error"; message: string }
  | { type: "raw"; line: string };

export type RunEvent = AgentEvent & { run_id: string };

const inTauri = "__TAURI_INTERNALS__" in window;

// Commands go over a WebSocket to `bach-server` (normally reached through
// `ssh -L 3421:localhost:3421 orion`) unless we're in the Tauri app with the local backend.
const DEFAULT_WS_URL = import.meta.env.VITE_BACH_WS ?? "ws://localhost:3421";
const STORAGE_KEY = "bach.backend";

const stored = (): string | null => {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
};

/** The remote backend URL in use, or null when commands run locally through Tauri. */
export const remoteUrl: string | null = inTauri ? stored() : (stored() ?? DEFAULT_WS_URL);
export const canSwitchBackend = inTauri;

/** Persists the choice (null = local) and reloads so every listener uses the new transport. */
export function setBackend(url: string | null) {
  try {
    url ? localStorage.setItem(STORAGE_KEY, url) : localStorage.removeItem(STORAGE_KEY);
  } catch {}
  location.reload();
}

type Pending = { resolve: (v: any) => void; reject: (e: Error) => void };

class Bridge {
  constructor(private url: string) {}
  private socket?: Promise<WebSocket>;
  private nextId = 0;
  private pending = new Map<number, Pending>();
  private listeners = new Set<(e: RunEvent) => void>();

  private connect(): Promise<WebSocket> {
    this.socket ??= new Promise<WebSocket>((resolve, reject) => {
      const ws = new WebSocket(this.url);
      ws.onopen = () => resolve(ws);
      ws.onerror = () => {
        this.socket = undefined;
        reject(new Error(`Can't reach bach-server at ${this.url}. Is it running, and is the SSH tunnel up?`));
      };
      ws.onclose = () => {
        this.socket = undefined;
        this.pending.forEach((p) => p.reject(new Error("bach-server connection closed")));
        this.pending.clear();
      };
      ws.onmessage = (m) => {
        const msg = JSON.parse(m.data);
        if (msg.event === "agent-event") return this.listeners.forEach((l) => l(msg.payload));
        const p = this.pending.get(msg.id);
        if (!p) return;
        this.pending.delete(msg.id);
        msg.ok ? p.resolve(msg.result) : p.reject(new Error(msg.error));
      };
    });
    return this.socket;
  }

  async call<T>(cmd: string, args: object = {}): Promise<T> {
    const ws = await this.connect();
    const id = ++this.nextId;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, cmd, ...args }));
    });
  }

  subscribe(cb: (e: RunEvent) => void) {
    this.listeners.add(cb);
    void this.connect().catch(() => {}); // events only flow while connected
    return () => this.listeners.delete(cb);
  }
}

const bridge = remoteUrl ? new Bridge(remoteUrl) : null;

const call = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  bridge ? bridge.call<T>(cmd, args) : invoke<T>(cmd, args);

export const listAgents = () => call<AgentInfo[]>("list_agents");

export const startRun = (args: {
  agent: AgentKind;
  prompt: string;
  cwd?: string;
  sessionId?: string;
}) => call<string>("start_run", args);

export const cancelRun = (runId: string) => call<void>("cancel_run", { runId });

/** Sessions are saved by whichever backend is in use (local app data, or the bach-server host). */
export const listSessions = () => call<unknown[]>("list_sessions");
export const saveSession = (session: object) => call<void>("save_session", { session });
export const deleteSession = (id: string) => call<void>("delete_session", { sessionId: id });

export const onAgentEvent = (cb: (e: RunEvent) => void): Promise<() => void> =>
  bridge ? Promise.resolve(bridge.subscribe(cb)) : listen<RunEvent>("agent-event", (e) => cb(e.payload));
