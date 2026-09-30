import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { Search, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { searchSessions, SearchKind, SearchResult, Session } from "@/api";
import { projectKey, projectName } from "@/session";
import { AgentDot } from "./AgentBadge";

const isMac = /Mac|iPhone|iPad/.test(navigator.platform);
const SHORTCUT = isMac ? "⌘K" : "Ctrl K";

const KIND_LABEL: Record<SearchKind, string> = { user: "You", agent: "Agent", tool: "Tool" };

/** Results for what's been typed, a moment after typing stops. The previous ones stay until new ones arrive. */
function useSearch(query: string, refreshKey: string) {
  const [results, setResults] = useState<SearchResult[]>([]);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults([]);
      setError(undefined);
      setBusy(false);
      return;
    }
    setBusy(true);
    let stale = false;
    const timer = setTimeout(() => {
      searchSessions(q)
        .then((r) => !stale && (setResults(r), setError(undefined)))
        .catch((e) => !stale && setError(e instanceof Error ? e.message : String(e)))
        .finally(() => !stale && setBusy(false));
    }, 120);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [query, refreshKey]);
  return { results, error, busy };
}

interface Item {
  id: string;
  /** The transcript entry to open at; none opens the session as it is. */
  seq?: number;
}

interface Props {
  sessions: Session[];
  activeId?: string;
  /** Opens a session, at a transcript entry when given. */
  onOpen: (id: string, seq?: number) => void;
  /** Changes each time something asks for the field (the shortcut). */
  focusSignal: number;
  /** What to show while nothing is searched for. */
  children: ReactNode;
}

/** The sidebar's search field. With something typed, its results replace `children`. */
export function SessionSearch({ sessions, activeId, onOpen, focusSignal, children }: Props) {
  const [query, setQuery] = useState("");
  const [current, setCurrent] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const listId = useId();
  // New activity can change what matches, so search again when it happens.
  const activity = sessions.reduce((latest, s) => Math.max(latest, s.updatedAt), 0) + ":" + sessions.length;
  const { results, error, busy } = useSearch(query, activity);

  useEffect(() => {
    if (focusSignal > 0) {
      input.current?.focus();
      input.current?.select();
    }
  }, [focusSignal]);

  const byId = useMemo(() => new Map(sessions.map((s) => [s.id, s])), [sessions]);
  // Sessions deleted since the search ran can't be opened.
  const shown = useMemo(() => results.filter((r) => byId.has(r.sessionId)), [results, byId]);
  // What the arrow keys go through: each matching entry, or a session matched only by its title.
  const items = useMemo<Item[]>(
    () => shown.flatMap((r) => (r.hits.length ? r.hits.map((h) => ({ id: r.sessionId, seq: h.seq })) : [{ id: r.sessionId }])),
    [shown],
  );
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const searching = query.trim() !== "";
  const itemKey = (i: Item) => `${i.id}:${i.seq ?? ""}`;

  useEffect(() => setCurrent(0), [results]);
  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [current, shown]);

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Escape") {
      e.preventDefault();
      if (query) setQuery("");
      else e.currentTarget.blur();
    } else if (!searching || e.nativeEvent.isComposing) {
      return;
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (items.length) setCurrent((c) => (c + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[current];
      if (item) onOpen(item.id, item.seq);
    }
  }

  const active = items[current];
  return (
    <>
      <div className="relative">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          ref={input}
          type="text"
          role="combobox"
          aria-label="Search sessions"
          aria-expanded={searching}
          aria-controls={searching ? listId : undefined}
          aria-activedescendant={searching && active ? `${listId}-${itemKey(active)}` : undefined}
          placeholder="Search sessions"
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          className="pr-14 pl-8 text-sm"
        />
        {searching ? (
          <button
            type="button"
            title="Clear (Esc)"
            aria-label="Clear search"
            onClick={() => (setQuery(""), input.current?.focus())}
            className="absolute top-1/2 right-1.5 flex size-5 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:text-foreground"
          >
            <X className="size-3.5" />
          </button>
        ) : (
          <kbd className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded border bg-background/60 px-1 font-sans text-[10px] text-muted-foreground">{SHORTCUT}</kbd>
        )}
      </div>

      {!searching ? (
        children
      ) : (
        <div className="-mx-1 flex min-h-0 flex-1 flex-col overflow-y-auto px-1">
          {error ? (
            <p className="px-2 py-1 text-sm text-destructive">Search failed: {error}</p>
          ) : shown.length === 0 ? (
            <p className="px-2 py-1 text-sm text-muted-foreground" role="status">
              {busy ? "Searching…" : "No sessions match."}
            </p>
          ) : (
            <ul ref={list} id={listId} role="listbox" aria-label="Search results" className={cn("flex flex-col gap-1", busy && "opacity-70")}>
              {shown.map((r) => {
                const s = byId.get(r.sessionId)!;
                const titleOnly = r.hits.length === 0;
                const open = (seq?: number) => onOpen(s.id, seq);
                return (
                  <li key={s.id} role="presentation" className={cn("rounded-lg", s.id === activeId && "bg-sidebar-accent/60")}>
                    <div
                      id={titleOnly ? `${listId}-${itemKey({ id: s.id })}` : undefined}
                      role={titleOnly ? "option" : undefined}
                      aria-selected={titleOnly ? active?.id === s.id && active.seq === undefined : undefined}
                      onClick={() => open(r.hits[0]?.seq)}
                      className={cn(
                        "flex cursor-default items-center gap-2 rounded-lg px-2 py-1.5 text-sm hover:bg-sidebar-accent/60",
                        titleOnly && active?.id === s.id && active.seq === undefined && "bg-sidebar-accent",
                      )}
                      title={s.title}
                    >
                      <AgentDot kind={s.agent} />
                      <span className="line-clamp-2 min-w-0 flex-1 break-words">
                        <Highlighted text={s.title} words={words} />
                      </span>
                      <span className="max-w-[30%] shrink-0 truncate text-[11px] text-muted-foreground">{projectName(projectKey(s.cwd))}</span>
                    </div>
                    {r.hits.map((h) => {
                      const selected = active?.id === s.id && active.seq === h.seq;
                      return (
                        <button
                          key={h.seq}
                          id={`${listId}-${itemKey({ id: s.id, seq: h.seq })}`}
                          role="option"
                          aria-selected={selected}
                          tabIndex={-1}
                          onClick={() => open(h.seq)}
                          className={cn(
                            "ml-3.5 block w-[calc(100%-0.875rem)] rounded-md px-2 py-1 text-left text-xs leading-snug text-muted-foreground hover:bg-sidebar-accent/60",
                            selected && "bg-sidebar-accent text-sidebar-foreground",
                          )}
                        >
                          <span className="line-clamp-2 break-words">
                            <span className="mr-1 text-[10px] font-medium tracking-wide uppercase opacity-70">{KIND_LABEL[h.kind]}</span>
                            {h.snippet.map((p, i) =>
                              p.matched ? (
                                <mark key={i} className="rounded-sm bg-primary/20 px-px font-medium text-foreground">
                                  {p.text}
                                </mark>
                              ) : (
                                <span key={i}>{p.text}</span>
                              ),
                            )}
                          </span>
                        </button>
                      );
                    })}
                    {r.hitCount > r.hits.length && (
                      <p className="ml-3.5 px-2 pb-1 text-[11px] text-muted-foreground">+{r.hitCount - r.hits.length} more in this session</p>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </>
  );
}

/** `text` with the query's words picked out, for titles (the backend highlights transcript snippets). */
function Highlighted({ text, words }: { text: string; words: string[] }) {
  const lower = text.toLowerCase();
  const marked = new Array<boolean>(text.length).fill(false);
  for (const w of words) {
    for (let i = lower.indexOf(w); i >= 0; i = lower.indexOf(w, i + 1)) marked.fill(true, i, i + w.length);
  }
  const parts: { text: string; matched: boolean }[] = [];
  for (let i = 0; i < text.length; i++) {
    const last = parts[parts.length - 1];
    if (last && last.matched === marked[i]) last.text += text[i];
    else parts.push({ text: text[i], matched: marked[i] });
  }
  return (
    <>
      {parts.map((p, i) =>
        p.matched ? (
          <mark key={i} className="rounded-sm bg-primary/20 text-foreground">
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}
