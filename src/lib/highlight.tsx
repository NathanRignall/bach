// Syntax highlighting by language, for the diff and the file viewer. The parsing happens in a
// worker (highlight.worker.ts), so large files don't hold up the UI; lines show plain until
// their colours arrive.
import { type ReactNode, useEffect, useRef, useState } from "react";

export interface HighlightRequest {
  id: number;
  /** Picks the language, by file name or extension. */
  path: string;
  docs: string[];
}

export interface HighlightResponse {
  id: number;
  /** None when the file's language isn't known. */
  language?: string;
  classes?: string[];
  docs?: Uint32Array[];
}

/** A styled range of a line: `[from, to)` in the line's text. */
export interface Token {
  from: number;
  to: number;
  className: string;
}

export interface Highlighted {
  language: string;
  /** Per document, per line, its styled ranges in order. */
  docs: Token[][][];
}

let worker: Worker | undefined;
let nextId = 0;
const pending = new Map<number, (r: HighlightResponse) => void>();

function getWorker(): Worker {
  if (!worker) {
    worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = ({ data }: MessageEvent<HighlightResponse>) => {
      pending.get(data.id)?.(data);
      pending.delete(data.id);
    };
  }
  return worker;
}

/** Highlights `docs` as the language of the file at `path`; undefined for unknown languages. */
export function highlight(path: string, docs: string[]): Promise<Highlighted | undefined> {
  const id = ++nextId;
  return new Promise((resolve) => {
    pending.set(id, (r) => {
      if (!r.language || !r.docs || !r.classes) return resolve(undefined);
      const classes = r.classes;
      resolve({
        language: r.language,
        docs: r.docs.map((spans, i) => {
          const lines: Token[][] = Array.from({ length: countLines(docs[i]) }, () => []);
          for (let j = 0; j < spans.length; j += 4) {
            lines[spans[j]]?.push({ from: spans[j + 1], to: spans[j + 2], className: classes[spans[j + 3]] });
          }
          return lines;
        }),
      });
    });
    getWorker().postMessage({ id, path, docs } satisfies HighlightRequest);
  });
}

function countLines(text: string) {
  let n = 1;
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) n++;
  return n;
}

const sameDocs = (a: string[], b: string[]) => a.length === b.length && a.every((d, i) => d === b[i]);

/**
 * `docs` highlighted as the language of `path`, once ready (undefined until then, and for
 * unknown languages). Documents with the same text as last time aren't highlighted again.
 */
export function useHighlight(path: string | undefined, newDocs: string[] | undefined): Highlighted | undefined {
  // Refetched but unchanged (e.g. the diff, every few seconds): keep the colours we have.
  const stable = useRef(newDocs);
  if (newDocs !== stable.current && !(newDocs && stable.current && sameDocs(newDocs, stable.current))) stable.current = newDocs;
  const docs = stable.current;
  const [result, setResult] = useState<{ docs: string[]; highlighted?: Highlighted }>();
  useEffect(() => {
    if (!path || !docs) return;
    let current = true;
    void highlight(path, docs).then((highlighted) => current && setResult({ docs, highlighted }));
    return () => {
      current = false;
    };
  }, [path, docs]);
  // Only ever the colours of these very documents, not the ones before.
  return result && result.docs === docs ? result.highlighted : undefined;
}

/** A line's text with its tokens' styles. */
export function renderLine(text: string, tokens: Token[] | undefined): ReactNode {
  if (!tokens?.length) return text;
  const out: ReactNode[] = [];
  let at = 0;
  for (const tok of tokens) {
    if (tok.from > at) out.push(text.slice(at, tok.from));
    out.push(
      <span key={tok.from} className={tok.className}>
        {text.slice(tok.from, tok.to)}
      </span>,
    );
    at = tok.to;
  }
  if (at < text.length) out.push(text.slice(at));
  return out;
}
