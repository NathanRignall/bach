import { memo, ReactNode } from "react";

// Dev servers colour their output and redraw lines with control sequences. In a log view, keep
// the colours, drop everything else, and show a line as it ended up after any carriage returns.

const CSI = /\x1b\[([0-9;:?]*)([ -/]*)([@-~])/g;
const OSC = /\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;

/** Text with all terminal control sequences removed. */
export function stripAnsi(s: string): string {
  return s.replace(OSC, "").replace(CSI, "");
}

/** One log line as a person would have seen it: `\r` redraws keep only the last version. */
export function cleanLine(raw: string): string {
  const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
  const i = line.lastIndexOf("\r");
  return i >= 0 ? line.slice(i + 1) : line;
}

/** The 8 standard colours, as text classes that read in both themes. Bright variants (90–97) use
 * the same shades. */
const COLOURS: Record<number, string> = {
  0: "text-zinc-500",
  1: "text-red-600 dark:text-red-400",
  2: "text-green-600 dark:text-green-400",
  3: "text-yellow-700 dark:text-yellow-300",
  4: "text-blue-600 dark:text-blue-400",
  5: "text-fuchsia-600 dark:text-fuchsia-400",
  6: "text-cyan-700 dark:text-cyan-300",
  7: "text-foreground",
};

/** The same colours as translucent backgrounds, so text on them stays readable in both themes. */
const BACKGROUNDS: Record<number, string> = {
  0: "bg-zinc-500/30",
  1: "bg-red-500/25",
  2: "bg-green-500/25",
  3: "bg-yellow-500/25",
  4: "bg-blue-500/25",
  5: "bg-fuchsia-500/25",
  6: "bg-cyan-500/25",
  7: "bg-zinc-300/40 dark:bg-zinc-600/40",
};

/** The same colours solid, for inverse text (which takes the page's colour and sits on one of these). */
const SOLID: Record<number, string> = {
  0: "bg-zinc-500",
  1: "bg-red-600 dark:bg-red-400",
  2: "bg-green-600 dark:bg-green-400",
  3: "bg-yellow-700 dark:bg-yellow-300",
  4: "bg-blue-600 dark:bg-blue-400",
  5: "bg-fuchsia-600 dark:bg-fuchsia-400",
  6: "bg-cyan-700 dark:bg-cyan-300",
  7: "bg-foreground",
};

/** Black and white as a program means them, for text on a background it chose. */
const LITERAL: Record<number, string> = { 0: "text-black", 7: "text-white" };

/** A colour: one of the 8 standard ones (a class), or an exact one from 256/true-colour codes. */
type Colour = { index: number } | { rgb: string };

/** Colour `n` of the 256-colour palette: the 16 standard ones, a 6×6×6 cube, then 24 greys. */
function palette(n: number): Colour | undefined {
  if (!(n >= 0 && n <= 255)) return undefined;
  if (n < 16) return { index: n % 8 };
  if (n < 232) {
    const level = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    const c = n - 16;
    return { rgb: `rgb(${level(Math.floor(c / 36))}, ${level(Math.floor(c / 6) % 6)}, ${level(c % 6)})` };
  }
  const grey = 8 + (n - 232) * 10;
  return { rgb: `rgb(${grey}, ${grey}, ${grey})` };
}

export interface Style {
  fg?: Colour;
  bg?: Colour;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  strike?: boolean;
  /** Foreground and background swapped. */
  inverse?: boolean;
}

/** A 256-colour (`5;n`) or true-colour (`2;r;g;b`) argument starting at `codes[i]`: the colour and
 * how many codes it used. */
function extended(codes: number[], i: number): [Colour | undefined, number] {
  if (codes[i] === 5) return [palette(codes[i + 1]), 2];
  if (codes[i] === 2) {
    const [r, g, b] = codes.slice(i + 1, i + 4);
    const ok = [r, g, b].every((v) => v >= 0 && v <= 255);
    return [ok ? { rgb: `rgb(${r}, ${g}, ${b})` } : undefined, 4];
  }
  return [undefined, 0];
}

export function applySgr(style: Style, params: string): Style {
  // Some programs separate a colour's parts with `:` (38:2::r:g:b); read those like `;`.
  const codes = params === "" ? [0] : params.replace(/:+/g, ";").split(";").map((n) => parseInt(n || "0", 10));
  let s = { ...style };
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) s = {};
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 7) s.inverse = true;
    else if (c === 9) s.strike = true;
    else if (c === 21 || c === 22) s.bold = s.dim = false;
    else if (c === 23) s.italic = false;
    else if (c === 24) s.underline = false;
    else if (c === 27) s.inverse = false;
    else if (c === 29) s.strike = false;
    else if (c >= 30 && c <= 37) s.fg = { index: c - 30 };
    else if (c >= 90 && c <= 97) s.fg = { index: c - 90 };
    else if (c === 39) s.fg = undefined;
    else if (c >= 40 && c <= 47) s.bg = { index: c - 40 };
    else if (c >= 100 && c <= 107) s.bg = { index: c - 100 };
    else if (c === 49) s.bg = undefined;
    else if (c === 38 || c === 48) {
      const [colour, used] = extended(codes, i + 1);
      if (colour) s[c === 38 ? "fg" : "bg"] = colour;
      i += used;
    }
  }
  return s;
}

/** Classes and inline styles for a run of text in `style`. */
export function presentation(style: Style): { className: string; css?: React.CSSProperties } {
  const css: React.CSSProperties = {};
  const cls: string[] = [];
  if (style.inverse) {
    // The text colour (or the page's text colour) becomes a solid background, and the text takes
    // the background colour (or the page's).
    const [fg, bg] = [style.bg, style.fg];
    if (!bg) cls.push(SOLID[7]);
    else if ("index" in bg) cls.push(SOLID[bg.index]);
    else css.backgroundColor = bg.rgb;
    if (!fg) cls.push("text-background");
    else if ("index" in fg) cls.push(LITERAL[fg.index] ?? COLOURS[fg.index]);
    else css.color = fg.rgb;
  } else {
    const { fg, bg } = style;
    // On a background the program chose, black and white mean black and white; on the page,
    // they follow the theme so they stay readable.
    if (fg && "index" in fg) cls.push((bg && LITERAL[fg.index]) || COLOURS[fg.index]);
    else if (fg) css.color = fg.rgb;
    if (bg && "index" in bg) cls.push(BACKGROUNDS[bg.index]);
    else if (bg) css.backgroundColor = bg.rgb;
  }
  if (style.bold) cls.push("font-semibold");
  if (style.dim) cls.push("opacity-60");
  if (style.italic) cls.push("italic");
  if (style.underline) cls.push("underline");
  if (style.strike) cls.push("line-through");
  return { className: cls.join(" "), css: Object.keys(css).length ? css : undefined };
}

/** Splits a line into styled runs. */
export function runs(line: string): { text: string; style: Style }[] {
  const out: { text: string; style: Style }[] = [];
  let style: Style = {};
  let last = 0;
  const clean = line.replace(OSC, "");
  for (const m of clean.matchAll(CSI)) {
    if (m.index! > last) out.push({ text: clean.slice(last, m.index), style });
    if (m[3] === "m") style = applySgr(style, m[1]);
    last = m.index! + m[0].length;
  }
  if (last < clean.length) out.push({ text: clean.slice(last), style });
  return out;
}

/** A log line with its colours. Memoised: long logs re-render often. */
export const AnsiLine = memo(function AnsiLine({ text }: { text: string }): ReactNode {
  const parts = runs(cleanLine(text));
  if (parts.length === 0) return <>{"​"}</>; // keep empty lines their height
  return (
    <>
      {parts.map((p, i) => {
        const { className, css } = presentation(p.style);
        return className || css ? (
          <span key={i} className={className || undefined} style={css}>
            {p.text}
          </span>
        ) : (
          p.text
        );
      })}
    </>
  );
});
