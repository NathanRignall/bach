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

// Outside Tauri (a plain browser) commands go over a WebSocket to `bach-server`, normally
// reached through `ssh -L 3421:localhost:3421 orion`.
const WS_URL = import.meta.env.VITE_BACH_WS ?? "ws://localhost:3421";

type Pending = { resolve: (v: any) => void; reject: (e: Error) => void };

class Bridge {
  private socket?: Promise<WebSocket>;
  private nextId = 0;
  private pending = new Map<number, Pending>();
  private listeners = new Set<(e: RunEvent) => void>();

  private connect(): Promise<WebSocket> {
    this.socket ??= new Promise<WebSocket>((resolve, reject) => {
      const ws = new WebSocket(WS_URL);
      ws.onopen = () => resolve(ws);
      ws.onerror = () => {
        this.socket = undefined;
        reject(new Error(`Can't reach bach-server at ${WS_URL}. Is it running, and is the SSH tunnel up?`));
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

const bridge = new Bridge();

export const listAgents = (): Promise<AgentInfo[]> =>
  inTauri ? invoke("list_agents") : bridge.call("list_agents");

export const startRun = (args: {
  agent: AgentKind;
  prompt: string;
  cwd?: string;
  sessionId?: string;
}): Promise<string> => (inTauri ? invoke("start_run", args) : bridge.call("start_run", args));

export const cancelRun = (runId: string): Promise<void> =>
  inTauri ? invoke("cancel_run", { runId }) : bridge.call("cancel_run", { runId });

export const onAgentEvent = (cb: (e: RunEvent) => void): Promise<() => void> =>
  inTauri
    ? listen<RunEvent>("agent-event", (e) => cb(e.payload))
    : Promise.resolve(bridge.subscribe(cb));
