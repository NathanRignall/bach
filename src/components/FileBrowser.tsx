import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { FolderTree, RefreshCw, Search } from "lucide-react";
import { FileContent, FileList, Session, listFiles, onReconnect, readFile } from "@/api";
import { CodeView } from "@/components/CodeView";
import { FileTree } from "@/components/FileTree";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

/** The file each session last had open, so it's still open on coming back to the files. */
const lastOpen = new Map<string, string>();

const formatSize = (bytes: number) =>
  bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;

type Loaded<T> = { value?: T; loading: boolean; error?: string };

/**
 * Loads with `load` whenever `deps` change and on `refresh`, keeping the last value while it
 * reloads (so a refresh doesn't flash) and ignoring answers that arrive out of order.
 */
function useLoad<T>(load: (() => Promise<T>) | undefined, deps: unknown[]): Loaded<T> & { refresh: () => void } {
  const [state, setState] = useState<Loaded<T>>({ loading: false });
  const latest = useRef(0);
  const refresh = () => {
    const req = ++latest.current;
    if (!load) return setState({ loading: false });
    setState((s) => ({ ...s, loading: true }));
    load().then(
      (value) => req === latest.current && setState({ value, loading: false }),
      (e) => req === latest.current && setState({ loading: false, error: String(e?.message ?? e) }),
    );
  };
  useEffect(() => {
    setState({ loading: false });
    refresh();
  }, deps);
  return { ...state, refresh };
}

/** Every file in the session's folder as a tree; the selected one is shown, read-only. */
export function FileBrowser({ session }: { session: Session }) {
  const [selected, setSelected] = useState(() => lastOpen.get(session.id));
  const [filter, setFilter] = useState("");
  const list = useLoad<FileList>(() => listFiles(session.id), [session.id]);
  const file = useLoad<FileContent>(selected ? () => readFile(session.id, selected) : undefined, [session.id, selected]);

  const refresh = () => (list.refresh(), file.refresh());
  // When the agent finishes (or starts), files have likely changed; so may they have while offline.
  const running = !!session.runId;
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current !== running) refresh();
    wasRunning.current = running;
  }, [running]);
  useEffect(() => onReconnect(refresh), [session.id, selected]);

  const select = (path: string) => {
    lastOpen.set(session.id, path);
    setSelected(path);
  };

  const files = useMemo(() => (list.value?.files ?? []).map((path) => ({ path })), [list.value]);
  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return q ? files.filter((f) => f.path.toLowerCase().includes(q)) : files;
  }, [files, filter]);

  if (session.workdirRemoved) return <Empty>This session's worktree was removed.</Empty>;

  return (
    <div className="flex min-h-0 flex-1">
      <nav className="flex w-72 shrink-0 flex-col border-r" aria-label="Files">
        <div className="flex shrink-0 items-center gap-1 border-b px-2 py-1.5">
          <Search className="ml-1 size-3.5 shrink-0 text-muted-foreground" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setFilter("")}
            placeholder="Filter files"
            aria-label="Filter files"
            className="min-w-0 flex-1 bg-transparent px-1 py-0.5 text-xs outline-none placeholder:text-muted-foreground"
          />
          <Button variant="ghost" size="icon-xs" title="Refresh" aria-label="Refresh" onClick={refresh} disabled={list.loading}>
            {list.loading ? <Spinner /> : <RefreshCw />}
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {list.error ? (
            <p className="p-3 text-xs text-destructive">{list.error}</p>
          ) : !list.value ? (
            <p className="flex items-center gap-2 p-3 text-xs text-muted-foreground">
              <Spinner /> Loading files…
            </p>
          ) : !shown.length ? (
            <p className="p-3 text-xs text-muted-foreground">{filter ? "No files match." : "No files."}</p>
          ) : (
            <>
              {/* Filtering shows every match; otherwise folders start closed. */}
              <FileTree key={filter ? "filtered" : "all"} files={shown} selected={selected} onSelect={(f) => select(f.path)} collapsed={!filter} label="Files" />
              {list.value.truncated && <p className="px-3 py-2 text-xs text-muted-foreground">Only the first {files.length.toLocaleString()} files are listed.</p>}
            </>
          )}
        </div>
      </nav>

      <div className="flex min-w-0 flex-1 flex-col">
        {!selected ? (
          <Empty>Select a file to view it.</Empty>
        ) : (
          <>
            <FileHeader path={selected} size={file.value?.path === selected ? file.value.size : undefined} loading={file.loading} />
            <div className="flex min-h-0 flex-1 flex-col overflow-auto bg-card select-text">
              {file.error ? (
                <Empty className="text-destructive">{file.error}</Empty>
              ) : !file.value ? (
                <Empty>
                  <Spinner /> Loading…
                </Empty>
              ) : file.value.binary ? (
                <Empty>Binary file</Empty>
              ) : file.value.text === null ? (
                <Empty>Too large to show ({formatSize(file.value.size)})</Empty>
              ) : (
                <CodeView path={file.value.path} text={file.value.text} />
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function FileHeader({ path, size, loading }: { path: string; size?: number; loading: boolean }) {
  const i = path.lastIndexOf("/");
  return (
    <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2 text-xs">
      <span className="min-w-0 flex-1 truncate font-mono select-text" title={path}>
        <span className="text-muted-foreground">{path.slice(0, i + 1)}</span>
        <span className="font-medium">{path.slice(i + 1)}</span>
      </span>
      {loading && <Spinner className="text-muted-foreground" />}
      {size !== undefined && <span className="shrink-0 text-muted-foreground tabular-nums">{formatSize(size)}</span>}
    </div>
  );
}

function Empty({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground", className)}>
      <FolderTree className="size-6 opacity-50" />
      <div className="flex items-center gap-2">{children}</div>
    </div>
  );
}
