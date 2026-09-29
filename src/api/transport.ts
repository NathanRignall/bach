// Carries commands and events between the UI and a backend. Both transports speak the protocol in
// ./generated/protocol.ts: a command is a name plus arguments, answered with a result or an
// ApiError; events are ServerEvents.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ApiError as ApiErrorData, ClientFrame, ErrorCode, JsonValue, ServerEvent, ServerFrame } from "./generated/protocol";

/** A command failed. `code` is for code that reacts to the failure; `message` is for people. */
export class ApiError extends Error {
  constructor(
    public code: ErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }

  static from(e: unknown): ApiError {
    if (e instanceof ApiError) return e;
    const data = e as Partial<ApiErrorData> | null;
    if (data && typeof data === "object" && typeof data.code === "string" && typeof data.message === "string") {
      return new ApiError(data.code, data.message);
    }
    return new ApiError("failed", e instanceof Error ? e.message : String(e));
  }
}

export interface Transport {
  call(cmd: string, args: unknown): Promise<unknown>;
  /** Every event from now on (while connected). Returns an unsubscribe function. */
  subscribe(cb: (e: ServerEvent) => void): () => void;
  /** Called after the connection dropped and came back: events in between were missed. */
  onReconnect(cb: () => void): () => void;
}

/** The Tauri app's own backend: one `rpc` command, events on the `bach` channel. */
export class TauriTransport implements Transport {
  async call(name: string, args: unknown) {
    try {
      return await invoke("rpc", { name, args });
    } catch (e) {
      throw ApiError.from(e);
    }
  }

  subscribe(cb: (e: ServerEvent) => void) {
    const un = listen<ServerEvent>("bach", (e) => cb(e.payload));
    return () => void un.then((f) => f());
  }

  onReconnect() {
    return () => {}; // in-process: never disconnects
  }
}

type Pending = { resolve: (v: unknown) => void; reject: (e: ApiError) => void };

/** A `bach-server` over a WebSocket. Reconnects while anyone is listening for events. */
export class SocketTransport implements Transport {
  private socket?: Promise<WebSocket>;
  private nextId = 0;
  private pending = new Map<number, Pending>();
  private listeners = new Set<(e: ServerEvent) => void>();
  private reconnectListeners = new Set<() => void>();
  private connectedBefore = false;
  private retryMs = 500;

  constructor(private url: string) {}

  private connect(): Promise<WebSocket> {
    this.socket ??= new Promise<WebSocket>((resolve, reject) => {
      const ws = new WebSocket(this.url);
      ws.onopen = () => {
        this.retryMs = 500;
        resolve(ws);
        if (this.connectedBefore) this.reconnectListeners.forEach((l) => l());
        this.connectedBefore = true;
      };
      ws.onerror = () =>
        reject(new ApiError("unavailable", `Can't reach bach-server at ${this.url}. Is it running, and is the SSH tunnel up?`));
      ws.onclose = () => {
        this.socket = undefined;
        this.pending.forEach((p) => p.reject(new ApiError("unavailable", "bach-server connection closed")));
        this.pending.clear();
        if (this.listeners.size) this.reconnectLater();
      };
      ws.onmessage = (m) => this.receive(JSON.parse(m.data) as ServerFrame);
    });
    return this.socket;
  }

  private reconnectLater() {
    setTimeout(() => void this.connect().catch(() => {}), this.retryMs);
    this.retryMs = Math.min(this.retryMs * 2, 5000);
  }

  private receive(frame: ServerFrame) {
    if (frame.kind === "event") return this.listeners.forEach((l) => l(frame.event));
    const p = this.pending.get(frame.id);
    if (!p) return;
    this.pending.delete(frame.id);
    frame.kind === "reply" ? p.resolve(frame.result) : p.reject(ApiError.from(frame.error));
  }

  async call(cmd: string, args: unknown) {
    const ws = await this.connect();
    const id = ++this.nextId;
    const frame: ClientFrame = { id, cmd, args: args as JsonValue };
    return new Promise<unknown>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      ws.send(JSON.stringify(frame));
    });
  }

  subscribe(cb: (e: ServerEvent) => void) {
    this.listeners.add(cb);
    void this.connect().catch(() => {}); // events only flow while connected
    return () => void this.listeners.delete(cb);
  }

  onReconnect(cb: () => void) {
    this.reconnectListeners.add(cb);
    return () => void this.reconnectListeners.delete(cb);
  }
}
