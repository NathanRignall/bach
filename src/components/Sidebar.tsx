import { useRef, useState } from "react";
import { Archive, ArchiveRestore, ChevronDown, ChevronRight, CircleAlert, CircleCheck, GitFork, ListChecks, MoreHorizontal, Pencil, Plus, Settings, ShieldAlert, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { Session, TaskView } from "@/api";
import { groupByProject, projectKey, projectName, sessionStatus } from "@/session";
import { AgentDot } from "./AgentBadge";
import { ConnectionPicker } from "./ConnectionPicker";
import { ResizeHandle } from "./ResizeHandle";

export const SIDEBAR_WIDTH = { default: 256, min: 200, max: 480 };

interface Props {
  sessions: Session[];
  activeId?: string;
  collapsed: Set<string>;
  onSelect: (id: string) => void;
  onNew: () => void;
  onNewInProject: (cwd: string, agent: Session["agent"]) => void;
  onToggleProject: (key: string) => void;
  onArchive: (id: string, archived: boolean) => void;
  onDelete: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onOpenCleanup: () => void;
  tasks: TaskView[];
  onToggleTasks: () => void;
  onOpenSettings: () => void;
  width: number;
  onWidth: (w: number) => void;
}

export function Sidebar({ sessions, activeId, collapsed, onSelect, onNew, onNewInProject, onToggleProject, onArchive, onDelete, onRename, onOpenCleanup, tasks, onToggleTasks, onOpenSettings, width, onWidth }: Props) {
  // Tasks only agents use (not interactive) are hidden from the panel by default, so not counted.
  const runningTasks = tasks.filter((t) => t.status === "running" && t.interactive).length;
  const [showArchived, setShowArchived] = useState(false);
  const live = sessions.filter((s) => !s.archived);
  const archived = sessions.filter((s) => s.archived);
  const [editing, setEditing] = useState<{ id: string; draft: string }>();
  const cancelled = useRef(false);

  function finishRename() {
    if (editing && !cancelled.current) {
      const title = editing.draft.trim();
      const current = sessions.find((s) => s.id === editing.id)?.title;
      if (title && title !== current) onRename(editing.id, title);
    }
    cancelled.current = false;
    setEditing(undefined);
  }
  return (
    <aside style={{ width }} className="relative flex shrink-0 flex-col gap-3 border-r bg-sidebar p-3 text-sidebar-foreground">
      {/* Room for the sidebar button (and the Mac traffic lights), and something to drag the window by. */}
      <div data-tauri-drag-region className="-mx-3 -mt-3 h-(--title-bar-height) shrink-0" />
      <Button variant="outline" onClick={onNew}>
        <Plus data-icon="inline-start" />
        New session
      </Button>

      <nav className="-mx-1 flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-1">
        {groupByProject(live).map(([key, group]) => {
          const open = !collapsed.has(key) || group.some((s) => s.id === activeId);
          return (
            <Collapsible key={key} open={open} onOpenChange={() => onToggleProject(key)}>
              <div className="group/head flex items-center text-muted-foreground" title={key || "Sessions without a project folder"}>
                <button
                  onClick={() => onToggleProject(key)}
                  className="flex h-7 min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 text-left text-xs font-semibold hover:text-foreground"
                >
                  {open ? <ChevronDown className="size-3.5 shrink-0" /> : <ChevronRight className="size-3.5 shrink-0" />}
                  <span className="min-w-0 flex-1 truncate">{projectName(key)}</span>
                  <span className={cn("font-normal tabular-nums", SHOWN_AT_REST("head"))}>{group.length}</span>
                </button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className={cn("mr-1", SHOWN_ON_HOVER("head"))}
                  title="New session in this project"
                  aria-label={`New session in ${projectName(key)}`}
                  onClick={() => onNewInProject(key, group[0].agent)}
                >
                  <Plus />
                </Button>
              </div>
              <CollapsibleContent>
                <ul className="ml-2 flex flex-col gap-px">
                  {group.map((s) => {
                    return (
                      <li
                        key={s.id}
                        className={cn(
                          "group/item flex items-center rounded-lg has-data-popup-open:bg-sidebar-accent/60",
                          s.id === activeId ? "bg-sidebar-accent" : "hover:bg-sidebar-accent/60",
                        )}
                      >
                        {editing?.id === s.id ? (
                          <div className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1">
                            <Input
                              autoFocus
                              aria-label="Session name"
                              value={editing.draft}
                              maxLength={80}
                              className="h-6 px-1.5 text-sm"
                              onFocus={(e) => e.currentTarget.select()}
                              onChange={(e) => setEditing({ id: s.id, draft: e.target.value })}
                              onBlur={finishRename}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") e.currentTarget.blur();
                                if (e.key === "Escape") {
                                  cancelled.current = true;
                                  e.currentTarget.blur();
                                }
                              }}
                            />
                          </div>
                        ) : (
                          <button
                            onClick={() => onSelect(s.id)}
                            onDoubleClick={() => setEditing({ id: s.id, draft: s.title })}
                            title={`${s.title}\nDouble-click to rename`}
                            className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left text-sm"
                          >
                            <StatusIcon session={s} />
                            <span className="min-w-0 flex-1 truncate">{s.title}</span>
                          </button>
                        )}
                        <RowMenu
                          archived={false}
                          onRename={() => setEditing({ id: s.id, draft: s.title })}
                          onArchive={() => onArchive(s.id, true)}
                          onDelete={() => onDelete(s.id)}
                        />
                      </li>
                    );
                  })}
                </ul>
              </CollapsibleContent>
            </Collapsible>
          );
        })}
        {archived.length > 0 && (
          <Collapsible open={showArchived} onOpenChange={setShowArchived}>
            <button
              onClick={() => setShowArchived((v) => !v)}
              className="flex h-7 w-full items-center gap-1.5 rounded-md px-2 text-left text-xs font-semibold text-muted-foreground hover:text-foreground"
            >
              {showArchived ? <ChevronDown className="size-3.5 shrink-0" /> : <ChevronRight className="size-3.5 shrink-0" />}
              <span className="min-w-0 flex-1">Archived</span>
              <span className="font-normal tabular-nums">{archived.length}</span>
            </button>
            <CollapsibleContent>
              <ul className="ml-2 flex flex-col gap-px">
                {archived.map((s) => {
                  return (
                    <li
                      key={s.id}
                      className={cn("group/item flex items-center rounded-lg has-data-popup-open:bg-sidebar-accent/60", s.id === activeId ? "bg-sidebar-accent" : "hover:bg-sidebar-accent/60")}
                    >
                      <button onClick={() => onSelect(s.id)} className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left text-sm text-muted-foreground">
                        <AgentDot kind={s.agent} className="opacity-40" />
                        <span className="min-w-0 flex-1 truncate">{s.title}</span>
                        <span className={cn("text-[11px]", SHOWN_AT_REST("item"))}>{projectName(projectKey(s.cwd))}</span>
                      </button>
                      <RowMenu archived onArchive={() => onArchive(s.id, false)} onDelete={() => onDelete(s.id)} />
                    </li>
                  );
                })}
              </ul>
            </CollapsibleContent>
          </Collapsible>
        )}
      </nav>

      <div className="flex flex-col gap-3 border-t pt-3">
        <Button variant="ghost" size="sm" className="justify-start text-muted-foreground" onClick={onToggleTasks}>
          <ListChecks data-icon="inline-start" />
          Background tasks
          {runningTasks > 0 && <span className="ml-auto rounded-full bg-primary/15 px-1.5 text-xs font-medium tabular-nums text-primary">{runningTasks} running</span>}
        </Button>
        <Button variant="ghost" size="sm" className="justify-start text-muted-foreground" onClick={onOpenCleanup}>
          <GitFork data-icon="inline-start" />
          Clean up worktrees…
        </Button>
        <Button variant="ghost" size="sm" className="justify-start text-muted-foreground" onClick={onOpenSettings}>
          <Settings data-icon="inline-start" />
          Settings
        </Button>
        <ConnectionPicker tasks={tasks} />
      </div>
      <ResizeHandle width={width} onWidth={onWidth} min={SIDEBAR_WIDTH.min} max={SIDEBAR_WIDTH.max} edge="right" reset={SIDEBAR_WIDTH.default} label="Resize sidebar" />
    </aside>
  );
}

/** What a session is doing, in the place of its agent's dot when there's something to say. */
function StatusIcon({ session: s }: { session: Session }) {
  switch (sessionStatus(s)) {
    case "waiting":
      return <ShieldAlert className="size-3.5 shrink-0 animate-pulse text-primary" role="img" aria-label="Waiting for you" />;
    case "failed":
      return <CircleAlert className="size-3.5 shrink-0 text-destructive" role="img" aria-label="Failed" />;
    case "done":
      return <CircleCheck className="size-3.5 shrink-0 text-emerald-600 dark:text-emerald-400" role="img" aria-label="Done, not yet viewed" />;
    case "working":
      return <AgentDot kind={s.agent} running />;
    case "idle":
      return <AgentDot kind={s.agent} />;
  }
}

/**
 * Row controls take the place of the badge or count at the row's right edge on hover (or keyboard focus),
 * rather than holding empty space open on every row.
 */
const SHOWN_AT_REST = (g: "item" | "head") =>
  g === "item" ? "group-hover/item:hidden group-has-focus-visible/item:hidden" : "group-hover/head:hidden group-has-focus-visible/head:hidden";
const SHOWN_ON_HOVER = (g: "item" | "head") =>
  g === "item"
    ? "hidden group-hover/item:flex group-has-focus-visible/item:flex"
    : "hidden group-hover/head:inline-flex group-has-focus-visible/head:inline-flex";

function RowMenu({ archived, onRename, onArchive, onDelete }: { archived: boolean; onRename?: () => void; onArchive: () => void; onDelete: () => void }) {
  const [confirming, setConfirming] = useState(false);
  const renaming = useRef(false);
  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (open) renaming.current = false;
        setConfirming(false);
      }}
    >
      <DropdownMenuTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className={cn("mr-1 data-popup-open:inline-flex", SHOWN_ON_HOVER("item"))}
            title="Session actions"
            aria-label="Session actions"
          />
        }
      >
        <MoreHorizontal />
      </DropdownMenuTrigger>
      {/* Renaming focuses the name field, so the menu mustn't hand focus back to its button as it closes. */}
      <DropdownMenuContent align="end" className="w-auto" finalFocus={() => !renaming.current}>
        {onRename && (
          <DropdownMenuItem
            onClick={() => {
              renaming.current = true;
              onRename();
            }}
          >
            <Pencil />
            Rename
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onClick={onArchive}>
          {archived ? <ArchiveRestore /> : <Archive />}
          {archived ? "Restore" : "Archive"}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" closeOnClick={confirming} onClick={confirming ? onDelete : () => setConfirming(true)}>
          <Trash2 />
          {confirming ? "Click again to delete" : "Delete"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
