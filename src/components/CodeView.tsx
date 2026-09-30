import { useMemo } from "react";
import { type Token, renderLine, useHighlight } from "@/lib/highlight";
import { cn } from "@/lib/utils";

/** Lines are laid out in blocks this long, and blocks off screen skip layout and painting. */
const BLOCK_LINES = 200;
/** Matches `leading-5`. */
const LINE_HEIGHT_PX = 20;

/** A file's lines, split once for the view and its highlighting. */
function splitLines(text: string) {
  const lines = text.split("\n");
  // A final newline ends the last line rather than starting another.
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/**
 * A file's text, read-only, with line numbers and syntax highlighting for the language of
 * `path`. Each line is an element with `data-line` (1-based).
 */
export function CodeView({ path, text, className }: { path: string; text: string; className?: string }) {
  const lines = useMemo(() => splitLines(text), [text]);
  const docs = useMemo(() => [text], [text]);
  const tokens = useHighlight(path, docs)?.docs[0];
  const blocks = useMemo(() => Array.from({ length: Math.ceil(lines.length / BLOCK_LINES) }, (_, i) => i * BLOCK_LINES), [lines]);
  const gutter = `calc(${String(lines.length).length}ch + 1.5rem)`;

  return (
    <div className={cn("w-max min-w-full py-2 font-mono text-xs leading-5 [tab-size:4]", className)}>
      {blocks.map((start) => (
        <CodeBlock key={start} lines={lines} start={start} tokens={tokens} gutter={gutter} />
      ))}
    </div>
  );
}

function CodeBlock({ lines, start, tokens, gutter }: { lines: string[]; start: number; tokens?: Token[][]; gutter: string }) {
  const end = Math.min(start + BLOCK_LINES, lines.length);
  return (
    <div style={{ contentVisibility: "auto", containIntrinsicSize: `auto ${(end - start) * LINE_HEIGHT_PX}px` }}>
      {lines.slice(start, end).map((line, i) => {
        const n = start + i + 1;
        return (
          <div key={n} data-line={n} className="flex">
            <span
              style={{ width: gutter }}
              className="sticky left-0 mr-3 shrink-0 border-r border-border/60 bg-card px-2 text-right text-muted-foreground/70 select-none"
            >
              {n}
            </span>
            <span className="pr-4 whitespace-pre">{renderLine(line, tokens?.[n - 1])}</span>
          </div>
        );
      })}
    </div>
  );
}
