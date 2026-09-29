import { useEffect, useRef, useState } from "react";
import { ChevronDown, Plus, SquareTerminal, X } from "lucide-react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import "@fontsource-variable/jetbrains-mono";
import {
  ApiError,
  TerminalInfo,
  closeTerminal,
  listTerminals,
  onReconnect,
  onTerminalEvent,
  openTerminal,
  resizeTerminal,
  terminalInput,
  terminalSnapshot,
} from "@/api";
import { Button } from "@/components/ui/button";
import { projectName } from "@/session";
import { cn } from "@/lib/utils";

/**
 * The terminal's own font, bundled: xterm measures its cell on a canvas, which only sees fonts the
 * page has loaded or the system has, and a missing one spreads the text apart.
 */
const FONT = "JetBrains Mono Variable";
const fontReady = document.fonts.load(`12px "${FONT}"`).then(
  () => {},
  () => {},
);

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

/** A theme colour as rgb(): xterm doesn't understand the oklch() the theme is written in. */
function themeColor(name: string, alpha = 1) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
  ctx.fillStyle = "#000";
  ctx.fillStyle = value;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

const theme = () => ({
  background: themeColor("--background"),
  foreground: themeColor("--foreground"),
  cursor: themeColor("--foreground"),
  cursorAccent: themeColor("--background"),
  selectionBackground: themeColor("--primary", 0.3),
});

const exitedText = (code: number | null) =>
  `\r\n\x1b[2m[shell ended${code !== null ? ` with code ${code}` : ""}; close this tab or open a new one]\x1b[0m\r\n`;

/** One terminal, drawn from the backend's scrollback and then kept current by its output events. */
function TerminalView({ id, visible }: { id: string; visible: boolean }) {
  const el = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal>(null);
  const fit = useRef<FitAddon>(null);
  // xterm measures its font when it's attached, and a hidden element measures as nothing (the
  // text then comes out spread apart), so it's only attached once it's first shown.
  const [shown, setShown] = useState(visible);
  useEffect(() => {
    if (visible) setShown(true);
  }, [visible]);
  const [fontLoaded, setFontLoaded] = useState(false);
  useEffect(() => void fontReady.then(() => setFontLoaded(true)), []);

  useEffect(() => {
    if (!shown || !fontLoaded) return;
    const t = new Terminal({
      fontFamily: `"${FONT}", Menlo, monospace`,
      fontSize: 12,
      lineHeight: 1.2,
      cursorBlink: true,
      scrollback: 10000,
      macOptionIsMeta: true,
      theme: theme(),
    });
    const f = new FitAddon();
    t.loadAddon(f);
    t.open(el.current!);
    term.current = t;
    fit.current = f;

    // Output events are numbered: apply them in order after the snapshot, and redraw from a new
    // snapshot if any were missed.
    let seq = -1;
    let pending: { seq: number; data: string }[] = [];
    let disposed = false;
    const load = async () => {
      seq = -1;
      try {
        const snap = await terminalSnapshot(id);
        if (disposed) return;
        t.reset();
        t.write(bytes(snap.data));
        seq = snap.seq;
        for (const p of pending.sort((a, b) => a.seq - b.seq)) {
          if (p.seq === seq + 1) {
            t.write(bytes(p.data));
            seq = p.seq;
          }
        }
        pending = [];
        if (snap.terminal.exited) t.write(exitedText(snap.terminal.exited.code));
      } catch (e) {
        if (ApiError.from(e).code !== "not_found") t.write(`\r\n\x1b[31m${ApiError.from(e).message}\x1b[0m\r\n`);
      }
    };
    const unEvents = onTerminalEvent((e) => {
      if (e.type === "output" && e.terminalId === id) {
        if (seq < 0) return void pending.push(e);
        if (e.seq <= seq) return;
        if (e.seq !== seq + 1) return void load();
        t.write(bytes(e.data));
        seq = e.seq;
      } else if (e.type === "exited" && e.terminalId === id && seq >= 0) {
        t.write(exitedText(e.status.code));
      }
    });
    const unReconnect = onReconnect(() => void load());
    void load();

    const input = t.onData((d) => void terminalInput(id, d).catch(() => {}));
    let resizing: ReturnType<typeof setTimeout>;
    const resized = t.onResize(({ cols, rows }) => {
      clearTimeout(resizing);
      resizing = setTimeout(() => void resizeTerminal(id, cols, rows).catch(() => {}), 80);
    });
    // Fit to the panel whenever it changes size (only while shown: hidden, it has no size).
    const ro = new ResizeObserver(() => el.current?.offsetParent && f.fit());
    ro.observe(el.current!);
    // Follow light/dark (main.tsx switches the class first).
    const media = matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => setTimeout(() => (t.options.theme = theme()), 0);
    media.addEventListener("change", onScheme);

    return () => {
      disposed = true;
      unEvents();
      unReconnect();
      input.dispose();
      resized.dispose();
      clearTimeout(resizing);
      ro.disconnect();
      media.removeEventListener("change", onScheme);
      t.dispose();
    };
  }, [id, shown, fontLoaded]);

  useEffect(() => {
    if (!visible) return;
    const frame = requestAnimationFrame(() => {
      fit.current?.fit();
      term.current?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [visible]);

  return <div ref={el} className={cn("h-full w-full", !visible && "hidden")} />;
}

const HEIGHT_KEY = "bach.terminalHeight";
const savedHeight = () => {
  try {
    return Number(localStorage.getItem(HEIGHT_KEY)) || 280;
  } catch {
    return 280;
  }
};

/**
 * Terminals on the machine the agents run on, in a panel under the chat. They belong to the
 * backend: hiding the panel, reloading or reconnecting leaves them running.
 */
export function TerminalPanel({ open, onClose, cwd }: { open: boolean; onClose: () => void; cwd: string }) {
  const [terms, setTerms] = useState<TerminalInfo[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [activeId, setActiveId] = useState<string>();
  const [error, setError] = useState<string>();
  const [height, setHeight] = useState(savedHeight);
  const opening = useRef(false);

  useEffect(() => {
    const load = () =>
      listTerminals()
        .then((t) => (setTerms(t), setLoaded(true)))
        .catch((e) => setError(ApiError.from(e).message));
    void load();
    const unEvents = onTerminalEvent((e) => {
      if (e.type === "opened") setTerms((all) => (all.some((t) => t.id === e.terminal.id) ? all : [...all, e.terminal]));
      if (e.type === "closed") setTerms((all) => all.filter((t) => t.id !== e.terminalId));
      if (e.type === "exited") setTerms((all) => all.map((t) => (t.id === e.terminalId ? { ...t, exited: e.status } : t)));
    });
    const unReconnect = onReconnect(() => void load());
    return () => (unEvents(), unReconnect());
  }, []);

  async function newTerminal() {
    if (opening.current) return;
    opening.current = true;
    setError(undefined);
    try {
      const t = await openTerminal({ cwd: cwd || undefined, cols: 100, rows: 24 });
      setTerms((all) => (all.some((x) => x.id === t.id) ? all : [...all, t]));
      setActiveId(t.id);
    } catch (e) {
      setError(ApiError.from(e).message);
    } finally {
      opening.current = false;
    }
  }

  // Opening the panel with no terminal starts one.
  useEffect(() => {
    if (open && loaded && terms.length === 0) void newTerminal();
  }, [open, loaded]);

  const active = terms.find((t) => t.id === activeId) ?? terms[terms.length - 1];

  function startResize(e: React.PointerEvent) {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = height;
    const move = (ev: PointerEvent) => {
      const h = Math.min(Math.max(startHeight + startY - ev.clientY, 120), window.innerHeight * 0.75);
      setHeight(h);
      try {
        localStorage.setItem(HEIGHT_KEY, String(Math.round(h)));
      } catch {}
    };
    const up = () => (window.removeEventListener("pointermove", move), window.removeEventListener("pointerup", up));
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  return (
    <section
      aria-label="Terminal"
      style={{ height }}
      className={cn("relative flex shrink-0 flex-col border-t bg-background", !open && "hidden")}
    >
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize terminal"
        onPointerDown={startResize}
        className="absolute inset-x-0 -top-1 z-10 h-2 cursor-row-resize"
      />
      <div className="flex h-9 shrink-0 items-center gap-1 border-b px-2 text-xs">
        <SquareTerminal className="mx-1 size-3.5 text-muted-foreground" />
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {terms.map((t) => (
            <div
              key={t.id}
              className={cn(
                "group flex shrink-0 items-center rounded-md",
                t.id === active?.id ? "bg-muted text-foreground" : "text-muted-foreground hover:bg-muted/60",
              )}
            >
              <button type="button" title={t.cwd} className="py-1 pl-2 pr-1" onClick={() => setActiveId(t.id)}>
                {projectName(t.cwd) || t.shell}
                {t.exited && <span className="ml-1 opacity-60">(ended)</span>}
              </button>
              <button
                type="button"
                aria-label={`Close terminal ${projectName(t.cwd)}`}
                title="Close (ends the shell)"
                className="mr-1 rounded p-0.5 opacity-60 hover:bg-background hover:opacity-100"
                onClick={() => void closeTerminal(t.id).catch((e) => setError(ApiError.from(e).message))}
              >
                <X className="size-3" />
              </button>
            </div>
          ))}
          <Button variant="ghost" size="icon-xs" aria-label="New terminal" title={`New terminal${cwd ? ` in ${cwd}` : ""}`} onClick={() => void newTerminal()}>
            <Plus />
          </Button>
        </div>
        {error && <span className="truncate text-destructive select-text">{error}</span>}
        <Button variant="ghost" size="icon-xs" aria-label="Hide terminal" title="Hide (Ctrl+`)" onClick={onClose}>
          <ChevronDown />
        </Button>
      </div>
      <div className="min-h-0 flex-1 py-1 pl-2">
        {terms.map((t) => (
          <TerminalView key={t.id} id={t.id} visible={open && t.id === active?.id} />
        ))}
      </div>
    </section>
  );
}
