// Which backend commands go to. In the desktop app, always the app itself (it runs agents here or
// relays to a server over SSH; see `ConnectionPicker`). In a plain browser, a `bach-server` over
// a WebSocket, normally reached through `ssh -L 3421:localhost:3421 orion`.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Connection, ConnectionStatus, Forwarding, PortForward } from "./generated/protocol";

export const inTauri = "__TAURI_INTERNALS__" in window;

/** The Mac app draws under the title bar (the traffic lights float over the sidebar). */
export const macTitleBar = inTauri && /Mac/.test(navigator.userAgent);

const DEFAULT_WS_URL = import.meta.env.VITE_BACH_WS ?? "ws://localhost:3421";

/** The bach-server a browser page talks to; null in the desktop app. */
export const remoteUrl: string | null = inTauri ? null : DEFAULT_WS_URL;

let current: ConnectionStatus | null = null;

/** The desktop app's connection and how it's doing. */
export async function getConnection(): Promise<ConnectionStatus> {
  current = await invoke<ConnectionStatus>("get_connection");
  return current;
}

/** Switches where the desktop app's agents run. Saved in the app. */
export const setConnection = (connection: Connection) => invoke<void>("set_connection", { connection });

/** Restarts bach-server on the SSH host (the installed build replaces a stale one), then reconnects. */
export const restartServer = () => invoke<void>("restart_server");

/** Host aliases from this computer's ~/.ssh/config (the app's, not the backend's). */
export const sshHosts = () => invoke<string[]>("ssh_hosts");

/** Starts the desktop app over. */
export const relaunchApp = () => invoke<void>("relaunch");

/** Every change to the desktop app's connection. Returns an unsubscribe function. */
export function onConnection(cb: (s: ConnectionStatus) => void): () => void {
  const un = listen<ConnectionStatus>("bach-connection", (e) => {
    current = e.payload;
    cb(e.payload);
  });
  return () => void un.then((f) => f());
}

if (inTauri) void getConnection().catch(() => {});

/** Host to reach a task's ports on, when the UI can tell. */
export function taskHost(): string | null {
  if (!inTauri) return location.hostname;
  const c = current?.connection;
  if (!c || c.mode === "local") return "localhost";
  // `user@host` or an ssh alias; an alias only works in a browser if it also resolves there.
  return c.host.replace(/^.*@/, "");
}

// Port forwarding (desktop app over SSH): ports on the agents' machine, reachable on this computer.

export const getForwarding = () => invoke<Forwarding>("get_forwarding");
export const forwardPort = (port: number) => invoke<PortForward>("forward_port", { port });
export const stopForward = (port: number) => invoke<void>("stop_forward", { port });
/** Opens the port in the default browser, forwarding it first when agents run elsewhere. */
export const openPort = (port: number) => invoke<void>("open_port", { port });
/** Opens a web or mail link in the default browser. */
export const openUrl = (url: string) => invoke<void>("open_url", { url });
export const setAutoForward = (auto: boolean) => invoke<void>("set_auto_forward", { auto });

export function onForwarding(cb: (f: Forwarding) => void): () => void {
  const un = listen<Forwarding>("bach-forwards", (e) => cb(e.payload));
  return () => void un.then((f) => f());
}
