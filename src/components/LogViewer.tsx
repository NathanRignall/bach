import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, ArrowDownToLine, Copy, Download, Search, WrapText, X } from "lucide-react";
import { TaskView, taskHost, taskLogChunk } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { AnsiLine, cleanLine, stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/utils";

/** Lines kept in memory, and lines drawn at once (a long log is windowed to its end). */
const MAX_LINES = 20000;
const MAX_DRAWN = 5000;
const CHUNK_BYTES = 1024 * 1024;

const STATUS: Record<TaskView["status"], string> = {
  running: "Running",
  exited: "Finished",
  failed: "Failed",
  stopped: "Stopped",
  lost: "Lost",
};

/** Clipboard access needs a secure context, which a page served over plain http isn't. */
async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const el = document.createElement("textarea");
    el.value = text;
    el.style.position = "fixed";
    el.style.opacity = "0";
    document.body.appendChild(el);
    el.select();
    document.execCommand("copy");
    el.remove();
  }
}

/** `text` with the parts matching `q` (already lower-cased) marked. */
function Highlight({ text, q }: { text: string; q: string }) {
  const lower = text.toLowerCase();
  const parts: React.ReactNode[] = [];
  let from = 0;
  for (let at = lower.indexOf(q); at !== -1 && q; at = lower.indexOf(q, from)) {
    if (at > from) parts.push(text.slice(from, at));
    parts.push(
      <mark key={at} className="rounded-sm bg-primary/30 text-foreground">
        {text.slice(at, at + q.length)}
      </mark>,
    );
    from = at + q.length;
  }
  parts.push(text.slice(from));
  return <>{parts}</>;
}

/** A task's whole log, full screen, kept up to date while the task runs. */
export function LogViewer({ task, onClose }: { task: TaskView; onClose: () => void }) {
  const running = task.status === "running";
  // `dropped` counts lines discarded from the front, so line numbers stay stable.
  const [buffer, setBuffer] = useState({ lines: [] as string[], dropped: 0 });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [filter, setFilter] = useState("");
  const [wrap, setWrap] = useState(false);
  const [follow, setFollow] = useState(true);
  const offset = useRef<number | undefined>(undefined);
  const inFlight = useRef(false);
  const scroller = useRef<HTMLDivElement>(null);
  const filterInput = useRef<HTMLInputElement>(null);
  const lastTop = useRef(0);

  // Pull new output: the tail first, then from where we stopped. While the task runs, keep going.
  useEffect(() => {
    let live = true;
    async function pull() {
      if (inFlight.current) return;
      inFlight.current = true;
      try {
        const first = offset.current === undefined;
        const chunk = await taskLogChunk(task.id, offset.current, CHUNK_BYTES);
        if (!live) return;
        offset.current = chunk.next;
        const added = chunk.text ? chunk.text.split("\n") : [];
        if (added[added.length - 1] === "") added.pop();
        if (first || chunk.restarted) {
          setBuffer({ lines: added.slice(-MAX_LINES), dropped: 0 });
        } else if (added.length) {
          setBuffer((b) => {
            const all = b.lines.concat(added);
            const over = Math.max(0, all.length - MAX_LINES);
            return { lines: over ? all.slice(over) : all, dropped: b.dropped + over };
          });
        }
        setError(undefined);
      } catch (e) {
        if (live) setError(String((e as Error).message ?? e));
      } finally {
        inFlight.current = false;
        if (live) setLoading(false);
      }
    }
    void pull();
    const t = running ? setInterval(pull, 1000) : undefined; // one last pull when it ends
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [task.id, running]);

  const q = filter.trim().toLowerCase();
  const matching = useMemo(() => {
    const all = buffer.lines.map((line, i) => ({ line, n: buffer.dropped + i + 1 }));
    return q ? all.filter((l) => stripAnsi(cleanLine(l.line)).toLowerCase().includes(q)) : all;
  }, [buffer, q]);
  const drawn = matching.length > MAX_DRAWN ? matching.slice(-MAX_DRAWN) : matching;

  // Follow the end while pinned there; scrolling up lets go, scrolling back down (or the button) re-pins.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && follow) el.scrollTop = el.scrollHeight;
  }, [drawn, follow, wrap]);
  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    const top = el.scrollTop;
    if (el.scrollHeight - top - el.clientHeight < 40) setFollow(true);
    else if (top < lastTop.current) setFollow(false);
    lastTop.current = top;
  };

  // Esc closes (first clearing a filter being typed); "/" jumps to the filter.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const typing = document.activeElement === filterInput.current;
      if (e.key === "Escape") {
        if (typing && filterInput.current?.value) setFilter("");
        else onClose();
      } else if (e.key === "/" && !typing) {
        e.preventDefault();
        filterInput.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const plainText = () => buffer.lines.map((l) => stripAnsi(cleanLine(l))).join("\n");
  const download = () => {
    const url = URL.createObjectURL(new Blob([plainText() + "\n"], { type: "text/plain" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${task.name.replace(/[^\w.-]+/g, "_").slice(0, 60) || "task"}-${task.id}.log`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const host = taskHost();
  const gutter = String(buffer.dropped + buffer.lines.length).length;

  return (
    <div role="dialog" aria-modal="true" aria-label={`Log: ${task.name}`} className="fixed inset-0 z-50 flex flex-col bg-background">
      <header className="flex flex-col gap-1.5 border-b px-4 py-2.5">
        <div className="flex items-center gap-3">
          <h2 className="min-w-0 truncate text-sm font-semibold">{task.name}</h2>
          <Badge variant={task.status === "failed" ? "destructive" : "outline"}>
            {STATUS[task.status]}
            {task.status === "failed" && task.exitCode !== null ? ` (${task.exitCode})` : ""}
          </Badge>
          {task.ports.map((p) =>
            host ? (
              <a key={p} href={`http://${host}:${p}`} target="_blank" rel="noopener noreferrer" className="rounded-md bg-primary/10 px-1.5 py-0.5 font-mono text-xs text-primary hover:underline">
                :{p}
              </a>
            ) : (
              <span key={p} className="rounded-md bg-primary/10 px-1.5 py-0.5 font-mono text-xs text-primary">
                :{p}
              </span>
            ),
          )}
          <span className="ml-auto text-xs text-muted-foreground tabular-nums">
            {matching.length === buffer.lines.length ? `${buffer.dropped + buffer.lines.length} lines` : `${matching.length} of ${buffer.lines.length} lines match`}
          </span>
          <Button variant="ghost" size="icon-sm" aria-label="Close log viewer" onClick={onClose}>
            <X />
          </Button>
        </div>
        <p className="truncate font-mono text-[11px] text-muted-foreground" title={task.command}>
          $ {task.command}
        </p>
        {task.problems.map((p) => (
          <p key={p} className="flex items-start gap-1.5 text-xs text-destructive">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
            {p}
          </p>
        ))}
      </header>

      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2">
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            ref={filterInput}
            aria-label="Filter log lines"
            className="h-8 w-64 pl-7 font-mono text-xs"
            placeholder="Filter lines   ( / )"
            value={filter}
            spellCheck={false}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <Button variant={follow ? "secondary" : "outline"} size="sm" aria-pressed={follow} onClick={() => setFollow((f) => !f)}>
          <ArrowDownToLine data-icon="inline-start" />
          Follow
        </Button>
        <Button variant={wrap ? "secondary" : "outline"} size="sm" aria-pressed={wrap} onClick={() => setWrap((w) => !w)}>
          <WrapText data-icon="inline-start" />
          Wrap
        </Button>
        <Button variant="outline" size="sm" onClick={() => void copyText(plainText())}>
          <Copy data-icon="inline-start" />
          Copy
        </Button>
        <Button variant="outline" size="sm" onClick={download}>
          <Download data-icon="inline-start" />
          Download
        </Button>
        {running && (
          <span className="ml-auto flex items-center gap-1.5 text-xs text-muted-foreground">
            <Spinner className="size-3.5 text-primary" /> live
          </span>
        )}
      </div>

      {error && <p className="border-b bg-destructive/10 px-4 py-1.5 text-xs text-destructive">{error}</p>}

      <div ref={scroller} onScroll={onScroll} className="min-h-0 flex-1 overflow-auto bg-muted/30 py-2 font-mono text-xs leading-relaxed select-text">
        {loading && (
          <div className="flex justify-center p-10">
            <Spinner className="size-5 text-muted-foreground" />
          </div>
        )}
        {!loading && matching.length === 0 && (
          <p className="p-10 text-center text-sm text-muted-foreground">{q ? "No lines match." : "(no output yet)"}</p>
        )}
        {matching.length > drawn.length && (
          <p className="px-4 pb-2 text-muted-foreground">… {matching.length - drawn.length} earlier lines not shown (download for the whole log)</p>
        )}
        <div className={cn(wrap ? "" : "w-max min-w-full")}>
          {drawn.map(({ line, n }) => (
            <div key={n} className="flex gap-3 px-4 hover:bg-accent/50">
              <span className="shrink-0 text-right text-muted-foreground/60 select-none tabular-nums" style={{ width: `${gutter}ch` }}>
                {n}
              </span>
              <span className={cn("min-w-0", wrap ? "break-all whitespace-pre-wrap" : "whitespace-pre")}>
                {q ? <Highlight text={stripAnsi(cleanLine(line))} q={q} /> : <AnsiLine text={line} />}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
