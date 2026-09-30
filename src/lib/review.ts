import { useState } from "react";
import type { DiffHunk, DiffLine, FileDiff } from "@/api";

/** Lines `from`..`to` (inclusive, by index) of one hunk of one file in the diff. */
export interface LineSelection {
  path: string;
  hunk: number;
  from: number;
  to: number;
}

/** Where a diff line is in the old and new file; a line has one or both. */
export interface LineRef {
  old: number | null;
  new: number | null;
}

/**
 * A stretch of a diff, as it was when picked: what to comment on, edit or ask about. It carries
 * the lines' text, so it still makes sense after the diff has moved on.
 */
export interface Anchor {
  path: string;
  /** The span in the old and new file (none if the lines aren't in it). */
  old: [number, number] | null;
  new: [number, number] | null;
  /** The last line, which a comment on the anchor is shown after. */
  end: LineRef;
  /** The lines as diff text, each prefixed with `+`, `-` or a space. */
  quote: string[];
}

/** A comment waiting to be sent with the others. */
export interface Draft {
  id: number;
  anchor: Anchor;
  text: string;
}

const PREFIX = { add: "+", delete: "-", context: " " } as const;
const ref = (l: DiffLine): LineRef => ({ old: l.old, new: l.new });

export const isSelected = (sel: LineSelection | undefined, path: string, hunk: number, i: number) =>
  !!sel && sel.path === path && sel.hunk === hunk && i >= sel.from && i <= sel.to;

/** The lines a selection covers in the current diff, clamped to the hunk. */
export function anchorOf(file: FileDiff, sel: LineSelection): Anchor | undefined {
  const lines = file.hunks[sel.hunk]?.lines.slice(sel.from, sel.to + 1);
  if (!lines?.length) return undefined;
  const span = (n: (l: DiffLine) => number | null): [number, number] | null => {
    const at = lines.map(n).filter((x): x is number => x !== null);
    return at.length ? [Math.min(...at), Math.max(...at)] : null;
  };
  return { path: file.path, old: span((l) => l.old), new: span((l) => l.new), end: ref(lines[lines.length - 1]), quote: lines.map((l) => PREFIX[l.kind] + l.text) };
}

/** Every line of a hunk. */
export const hunkSelection = (path: string, hunk: number, h: DiffHunk): LineSelection => ({ path, hunk, from: 0, to: h.lines.length - 1 });

const same = (a: LineRef, l: DiffLine) => (a.new !== null ? a.new === l.new : a.old === l.old);

/** The index of the line after which an anchor's comment is shown, if it's still in the diff. */
export function anchorEnd(file: FileDiff, a: Anchor): [hunk: number, line: number] | undefined {
  for (let h = 0; h < file.hunks.length; h++) {
    const i = file.hunks[h].lines.findIndex((l) => same(a.end, l));
    if (i >= 0) return [h, i];
  }
  return undefined;
}

const span = ([a, b]: [number, number]) => (a === b ? `line ${a}` : `lines ${a}–${b}`);

/** "line 4" or "lines 4–9" in the new file; for removed lines "old lines 4–9"; and where both, "(old lines …)". */
export function rangeLabel(a: Anchor) {
  if (!a.new) return a.old ? `old ${span(a.old)}` : "";
  return a.old && a.quote.some((l) => l.startsWith("-")) ? `${span(a.new)} (old ${span(a.old)})` : span(a.new);
}

/** A code block around `code` that its own backticks can't close. */
function fenced(code: string[]) {
  const longest = Math.max(2, ...code.map((l) => Math.max(0, ...(l.match(/`+/g) ?? []).map((r) => r.length))));
  const fence = "`".repeat(longest + 1);
  return `${fence}diff\n${code.join("\n")}\n${fence}`;
}

const where = (a: Anchor) => `\`${a.path}\`, ${rangeLabel(a)}`;

/** The message that sends review comments to the agent. */
export function reviewMessage(drafts: Draft[]) {
  const head = `I reviewed your changes and have ${drafts.length === 1 ? "a comment" : `${drafts.length} comments`}. The code is quoted as a diff (+ added, - removed). Please address ${drafts.length === 1 ? "it" : "them"}.`;
  const items = drafts.map((d, i) => `${i + 1}. ${where(d.anchor)}\n\n${fenced(d.anchor.quote)}\n\n${d.text.trim()}`);
  return [head, ...items].join("\n\n");
}

/** The message that asks the agent why it made a change. */
export function explainMessage(anchor: Anchor) {
  return `Why did you make this change in ${where(anchor)}? Explain your reasoning, and say if there's a better way. Don't change any code yet.\n\n${fenced(anchor.quote)}`;
}

/**
 * The selection in the diff, and the comments drafted on it, kept for a session while its
 * view is switched away from the changes.
 */
export function useReview(sessionId: string | undefined) {
  const [state, setState] = useState<{ id?: string; selection?: LineSelection; drafts: Draft[]; next: number }>({ drafts: [], next: 1 });
  // Another session starts clean; its own are lost, as the diff's are.
  const cur = state.id === sessionId ? state : { id: sessionId, drafts: [], next: 1 };
  const set = (f: (s: typeof cur) => Partial<typeof cur>) => setState((s) => {
    const c = s.id === sessionId ? s : cur;
    return { ...c, ...f(c) };
  });

  return {
    selection: cur.selection,
    drafts: cur.drafts,
    /** Selects `sel`, or with `extend` stretches the current selection to it (within one hunk). */
    select: (sel: LineSelection | undefined, extend = false) =>
      set((s) => {
        const from = s.selection;
        if (!sel || !extend || !from || from.path !== sel.path || from.hunk !== sel.hunk) return { selection: sel };
        return { selection: { ...sel, from: Math.min(from.from, sel.from), to: Math.max(from.to, sel.to) } };
      }),
    add: (anchor: Anchor, text: string) =>
      set((s) => ({ drafts: [...s.drafts, { id: s.next, anchor, text }], next: s.next + 1, selection: undefined })),
    edit: (id: number, text: string) => set((s) => ({ drafts: s.drafts.map((d) => (d.id === id ? { ...d, text } : d)) })),
    remove: (id: number) => set((s) => ({ drafts: s.drafts.filter((d) => d.id !== id) })),
    clearDrafts: () => set(() => ({ drafts: [] })),
  };
}

export type Review = ReturnType<typeof useReview>;
