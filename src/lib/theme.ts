import { useSyncExternalStore } from "react";

export type Mode = "system" | "light" | "dark" | "high-contrast";
export type Accent = "terracotta" | "blue" | "green" | "violet" | "rose";

export const MODES: { value: Mode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "high-contrast", label: "High contrast" },
];

/** `swatch` is what the picker shows; the real colours live in index.css under `data-accent`. */
export const ACCENTS: { value: Accent; label: string; swatch: string }[] = [
  { value: "terracotta", label: "Terracotta", swatch: "oklch(0.66 0.14 40)" },
  { value: "blue", label: "Blue", swatch: "oklch(0.62 0.16 255)" },
  { value: "green", label: "Green", swatch: "oklch(0.64 0.15 150)" },
  { value: "violet", label: "Violet", swatch: "oklch(0.62 0.17 295)" },
  { value: "rose", label: "Rose", swatch: "oklch(0.64 0.18 5)" },
];

export interface Theme {
  mode: Mode;
  accent: Accent;
}

const KEY = "bach.theme";
const media = window.matchMedia("(prefers-color-scheme: dark)");

function load(): Theme {
  try {
    const t = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return {
      mode: MODES.some((m) => m.value === t.mode) ? t.mode : "system",
      accent: ACCENTS.some((a) => a.value === t.accent) ? t.accent : "terracotta",
    };
  } catch {
    return { mode: "system", accent: "terracotta" };
  }
}

let current = load();
const listeners = new Set<() => void>();

function apply() {
  const hc = current.mode === "high-contrast";
  const dark = hc || current.mode === "dark" || (current.mode === "system" && media.matches);
  const root = document.documentElement;
  root.classList.toggle("dark", dark);
  root.classList.toggle("high-contrast", hc);
  root.dataset.accent = current.accent;
}

const notify = () => listeners.forEach((l) => l());

/** Applies the saved theme, and follows the system setting while the mode is "system". */
export function initTheme() {
  apply();
  media.addEventListener("change", () => (apply(), notify()));
}

/**
 * Calls `listener` after the theme has been applied to the page (so its CSS variables are current),
 * whether the mode or accent was chosen here or the system switched while the mode is "system".
 * For things that copy the colours, like the terminal, and so can't follow the CSS by themselves.
 */
export function onThemeChange(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

export function setTheme(patch: Partial<Theme>) {
  current = { ...current, ...patch };
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {}
  apply();
  notify();
}

export function useTheme(): Theme {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => current,
  );
}
