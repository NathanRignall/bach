import { ReactNode } from "react";
import { AgentInfo, AgentKind } from "@/api";
import { Button } from "@/components/ui/button";
import { NewSession as Session, projectName } from "@/session";
import { BlockView } from "./Transcript";
import { BranchControls, BranchHint, useGitInfo } from "./GitControls";
import { Composer } from "./Composer";
import { ProjectButton } from "./ProjectButton";

interface Props {
  session: Session;
  agents: AgentInfo[];
  draft: string;
  onDraft: (v: string) => void;
  onSend: () => void;
  starting: boolean;
  recentProjects: string[];
  onChange: (patch: Partial<Session>) => void;
  error?: string;
  /** Shown next to the model picker (usage). */
  indicator?: ReactNode;
}

/** Where a session is set up: the chat input with the project folder and branch beside it. */
export function NewSessionPage({ session, agents, draft, onDraft, onSend, starting, recentProjects, onChange, error, indicator }: Props) {
  const git = useGitInfo(session, onChange);
  const hasFolder = !!session.cwd.trim();
  const setProject = (cwd: string) => onChange({ cwd, branch: undefined, worktree: false });

  return (
    <div className="flex flex-1 flex-col items-center justify-center overflow-y-auto px-5 py-10">
      <div className="flex w-full max-w-2xl flex-col gap-4">
        <h1 className="text-center text-2xl font-medium tracking-tight">What should we work on?</h1>

        {/* Errors from a failed start stay here so the settings can be fixed and retried. */}
        <div className="contents select-text">
          {session.blocks.map((b, i) => (
            <BlockView key={i} block={b} live={false} />
          ))}
        </div>
        {error && <p className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</p>}

        <Composer
          autoFocus
          tall
          draft={draft}
          onDraft={onDraft}
          onSend={onSend}
          onStop={() => {}}
          running={false}
          starting={starting}
          blockedReason={hasFolder ? undefined : "Choose a project folder first"}
          placeholder={hasFolder ? "Message the agent…" : "Choose a project folder, then describe the task…"}
          agents={agents}
          agent={session.agent}
          agentLocked={false}
          indicator={indicator}
          onAgent={(agent: AgentKind) => onChange({ agent })}
          modelChoice={session.modelChoice ?? undefined}
          onModel={(modelChoice) => onChange({ modelChoice })}
          left={
            <>
              <ProjectButton cwd={session.cwd} onChange={setProject} />
              <BranchControls session={session} git={git} onChange={onChange} />
            </>
          }
        />

        <div className="min-h-5 px-1">
          <BranchHint session={session} git={git} />
        </div>

        {recentProjects.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5 px-1">
            <span className="text-xs text-muted-foreground">Recent</span>
            {recentProjects.map((p) => (
              <Button key={p} variant={p === session.cwd.trim() ? "secondary" : "ghost"} size="xs" title={p} onClick={() => setProject(p)}>
                {projectName(p)}
              </Button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
