import { Cpu, GitBranch, GitFork } from "lucide-react";
import { Session } from "@/session";

/** Read-only summary of where a started session runs. */
export function SessionHeader({ session }: { session: Session }) {
  return (
    <header className="flex items-center gap-4 border-b px-5 py-2.5 text-xs text-muted-foreground">
      <span className="min-w-0 truncate font-mono" title={session.workdir ?? session.cwd}>
        {session.cwd || "No project folder"}
      </span>
      {session.gitBranch && (
        <span className="flex shrink-0 items-center gap-1.5" title={session.workdir}>
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
