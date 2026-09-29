import { Cpu, GitBranch, GitFork } from "lucide-react";
import { Session } from "@/api";

/** Read-only summary of where a started session runs. */
export function SessionHeader({ session }: { session: Session }) {
  return (
    // Also the window's title bar in the Mac app: drag it by the empty space.
    <header data-tauri-drag-region className="flex h-(--title-bar-height) shrink-0 items-center gap-4 border-b px-5 text-xs text-muted-foreground">
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
        </span>
      )}
    </header>
  );
}
