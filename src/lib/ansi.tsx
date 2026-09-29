import { memo, ReactNode } from "react";

// Dev servers colour their output and redraw lines with control sequences. In a log view, keep
// the colours, drop everything else, and show a line as it ended up after any carriage returns.

const CSI = /\x1b\[([0-9;?]*)([ -/]*)([@-~])/g;
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

const COLOURS: Record<number, string> = {
  30: "text-zinc-500",
  31: "text-red-600 dark:text-red-400",
  32: "text-green-600 dark:text-green-400",
  33: "text-yellow-700 dark:text-yellow-300",
  34: "text-blue-600 dark:text-blue-400",
  35: "text-fuchsia-600 dark:text-fuchsia-400",
  36: "text-cyan-700 dark:text-cyan-300",
  37: "text-foreground",
};

interface Style {
  colour?: string;
  bold?: boolean;
  dim?: boolean;
}

function applySgr(style: Style, params: string): Style {
  const codes = params === "" ? [0] : params.split(";").map((n) => parseInt(n || "0", 10));
  let s = { ...style };
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) s = {};
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 22) s.bold = s.dim = false;
    else if (c === 39) s.colour = undefined;
    else if ((c >= 30 && c <= 37) || (c >= 90 && c <= 97)) s.colour = COLOURS[c >= 90 ? c - 60 : c];
    else if (c === 38 || c === 48) i += codes[i + 1] === 5 ? 2 : codes[i + 1] === 2 ? 4 : 0; // 256/true colour: skip
  }
  return s;
}

/** Splits a line into styled runs. */
function runs(line: string): { text: string; style: Style }[] {
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
        const cls = [p.style.colour, p.style.bold && "font-semibold", p.style.dim && "opacity-60"].filter(Boolean).join(" ");
        return cls ? (
          <span key={i} className={cls}>
            {p.text}
          </span>
        ) : (
          p.text
        );
      })}
    </>
  );
});
