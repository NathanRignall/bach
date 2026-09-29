// Which backend commands go to: the Tauri app's own (local), or a `bach-server` over a
// WebSocket (normally reached through `ssh -L 3421:localhost:3421 orion`).

export const inTauri = "__TAURI_INTERNALS__" in window;

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

/** Host to reach a task's ports on, when the UI can tell (a page served by the backend host, or a local app). */
export const taskHost = (): string | null => (remoteUrl ? (inTauri ? null : location.hostname) : "localhost");
