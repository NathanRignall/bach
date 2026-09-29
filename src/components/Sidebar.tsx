import { useRef, useState } from "react";
import { Archive, ArchiveRestore, ChevronDown, ChevronRight, GitFork, ListChecks, Plus, Settings, ShieldAlert, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { Session, TaskView } from "@/api";
import { awaitingApproval, groupByProject, projectKey, projectName } from "@/session";
import { AgentBadge } from "./AgentBadge";
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
  const runningTasks = tasks.filter((t) => t.status === "running").length;
  const [confirmDelete, setConfirmDelete] = useState<string>();
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
                  className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-xs font-semibold hover:text-foreground"
                >
                  {open ? <ChevronDown className="size-3.5 shrink-0" /> : <ChevronRight className="size-3.5 shrink-0" />}
                  <span className="min-w-0 flex-1 truncate">{projectName(key)}</span>
                  <span className="font-normal">{group.length}</span>
                </button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="opacity-0 group-hover/head:opacity-100 focus-visible:opacity-100"
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
                    const confirming = confirmDelete === s.id;
                    return (
                      <li
                        key={s.id}
                        className={cn(
                          "group/item flex items-center rounded-lg",
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
                            {awaitingApproval(s) ? (
                              <ShieldAlert className="size-3.5 shrink-0 animate-pulse text-primary" aria-label="Needs your approval" />
                            ) : s.runId ? (
                              <Spinner className="size-3.5 shrink-0 text-primary" aria-label="Running" />
                            ) : (
                              <span className="size-1.5 shrink-0 rounded-full bg-muted-foreground/30" />
                            )}
                            <span className="min-w-0 flex-1 truncate">{s.title}</span>
                            <AgentBadge kind={s.agent} />
                          </button>
                        )}
                        {!confirming && (
                          <Button
                            variant="ghost"
                            size="xs"
                            className="invisible group-hover/item:visible focus-visible:visible"
                            title="Archive session"
                            aria-label="Archive session"
                            onClick={() => onArchive(s.id, true)}
                          >
                            <Archive />
                          </Button>
                        )}
                        <Button
                          variant={confirming ? "destructive" : "ghost"}
                          size="xs"
                          className={cn("mr-1", !confirming && "invisible group-hover/item:visible focus-visible:visible")}
                          title="Delete session"
                          aria-label="Delete session"
                          onClick={() => {
                            if (!confirming) return setConfirmDelete(s.id);
                            setConfirmDelete(undefined);
                            onDelete(s.id);
                          }}
                          onBlur={() => setConfirmDelete(undefined)}
                        >
                          {confirming ? "Delete?" : <X />}
                        </Button>
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
              className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-xs font-semibold text-muted-foreground hover:text-foreground"
            >
              {showArchived ? <ChevronDown className="size-3.5 shrink-0" /> : <ChevronRight className="size-3.5 shrink-0" />}
              <span className="min-w-0 flex-1">Archived</span>
              <span className="font-normal">{archived.length}</span>
            </button>
            <CollapsibleContent>
              <ul className="ml-2 flex flex-col gap-px">
                {archived.map((s) => {
                  const confirming = confirmDelete === s.id;
                  return (
                    <li
                      key={s.id}
                      className={cn("group/item flex items-center rounded-lg", s.id === activeId ? "bg-sidebar-accent" : "hover:bg-sidebar-accent/60")}
                    >
                      <button onClick={() => onSelect(s.id)} className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left text-sm text-muted-foreground">
                        <span className="min-w-0 flex-1 truncate">{s.title}</span>
                        <span className="text-[11px]">{projectName(projectKey(s.cwd))}</span>
                      </button>
                      {!confirming && (
                        <Button
                          variant="ghost"
                          size="xs"
                          className="invisible group-hover/item:visible focus-visible:visible"
                          title="Restore session"
                          aria-label="Restore session"
                          onClick={() => onArchive(s.id, false)}
                        >
                          <ArchiveRestore />
                        </Button>
                      )}
                      <Button
                        variant={confirming ? "destructive" : "ghost"}
                        size="xs"
                        className={cn("mr-1", !confirming && "invisible group-hover/item:visible focus-visible:visible")}
                        title="Delete session"
                        aria-label="Delete session"
                        onClick={() => {
                          if (!confirming) return setConfirmDelete(s.id);
                          setConfirmDelete(undefined);
                          onDelete(s.id);
                        }}
                        onBlur={() => setConfirmDelete(undefined)}
                      >
                        {confirming ? "Delete?" : <X />}
                      </Button>
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
