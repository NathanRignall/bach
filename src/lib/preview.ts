import { useSyncExternalStore } from "react";
import { portUrl } from "@/api";

/** The page shown in the browser panel, if it's open. A `port` is on the agents' machine. */
export interface Preview {
  url: string;
  port?: number;
}

let current: Preview | null = null;
const listeners = new Set<() => void>();
const set = (p: Preview | null) => ((current = p), listeners.forEach((l) => l()));

export const usePreview = () =>
  useSyncExternalStore(
    (l) => (listeners.add(l), () => void listeners.delete(l)),
    () => current,
  );

/** Shows a port on the agents' machine in the browser panel (forwarding it first if need be). */
export async function previewPort(port: number) {
  set({ url: await portUrl(port), port });
}

export const closePreview = () => set(null);
