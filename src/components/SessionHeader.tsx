import { Brain, Cpu, FileDiff, GitBranch, GitFork, MessageSquare } from "lucide-react";
import { Session, macTitleBar } from "@/api";
import { cn } from "@/lib/utils";

/** What the session's main area shows. */
export type SessionView = "chat" | "changes";

/** Switches between the conversation and the session's changes. */
function ViewSwitcher({ view, onView, changedFiles }: { view: SessionView; onView: (v: SessionView) => void; changedFiles?: number }) {
  const tabs = [
    { id: "chat", label: "Chat", icon: MessageSquare },
    { id: "changes", label: "Changes", icon: FileDiff },
  ] as const;
  return (
    <div className="flex shrink-0 rounded-lg bg-muted p-0.5" role="tablist" aria-label="Session view">
      {tabs.map(({ id, label, icon: Icon }) => (
        <button
          key={id}
          type="button"
          role="tab"
          aria-selected={view === id}
          onClick={() => onView(id)}
          title={id === "changes" ? "Changes (Ctrl/Cmd+Shift+D)" : "Chat (Ctrl/Cmd+Shift+D)"}
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs font-medium",
            view === id ? "bg-background text-foreground shadow-xs" : "text-muted-foreground hover:text-foreground",
          )}
        >
          <Icon className="size-3.5" />
          {label}
          {id === "changes" && !!changedFiles && (
            <span className="rounded-full bg-primary/15 px-1.5 text-[10px] tabular-nums text-primary">{changedFiles}</span>
          )}
        </button>
      ))}
    </div>
  );
}

/** Where a started session runs, and the switch between its chat and its changes. */
export function SessionHeader({
  session,
  inset,
  view,
  onView,
  changedFiles,
}: {
  session: Session;
  /** The sidebar is hidden, so its button sits over the header's left end. */
  inset?: boolean;
  view: SessionView;
  onView: (v: SessionView) => void;
  /** How many files the session's checkout has changed, once known. */
  changedFiles?: number;
}) {
  return (
    // Also the window's title bar in the Mac app: drag it by the empty space.
    <header data-tauri-drag-region className="flex h-(--title-bar-height) shrink-0 items-center gap-4 border-b px-5 text-xs text-muted-foreground"
      style={inset ? { paddingLeft: `calc(${macTitleBar ? "var(--traffic-lights-end)" : "12px"} + 2.75rem)` } : undefined}
    >
      <ViewSwitcher view={view} onView={onView} changedFiles={changedFiles} />
      <span className="min-w-0 truncate font-mono select-text" title={session.workdir ?? session.cwd}>
        {session.cwd || "No project folder"}
      </span>
      {session.gitBranch && (
        <span className="flex shrink-0 items-center gap-1.5" title={session.workdir ?? undefined}>
          {session.worktree ? <GitFork className="size-3.5" /> : <GitBranch className="size-3.5" />}
          <span className="font-mono">{session.gitBranch}</span>
          {session.worktree && <span>· worktree{session.workdirRemoved ? " (removed)" : ""}</span>}
        </span>
      )}
      {session.model && (
        <span className="ml-auto flex shrink-0 items-center gap-1.5" title="Model used on the latest run">
          <Cpu className="size-3.5" />
          <span className="font-mono">{session.model}</span>
          {/* Only a level chosen for the session; the model's default isn't always known. */}
          {session.effort && (
            <span className="flex items-center gap-1" title="Thinking effort">
              · <Brain className="size-3.5" />
              {session.effort}
            </span>
          )}
        </span>
      )}
    </header>
  );
}
