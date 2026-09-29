import { AgentKind } from "@/api";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

/** Each agent keeps one colour everywhere it is named, so sessions can be told apart at a glance. */
const STYLES: Record<AgentKind, string> = {
  claude: "bg-orange-500/15 text-orange-700 dark:bg-orange-400/15 dark:text-orange-300",
  codex: "bg-emerald-500/15 text-emerald-700 dark:bg-emerald-400/15 dark:text-emerald-300",
  opencode: "bg-sky-500/15 text-sky-700 dark:bg-sky-400/15 dark:text-sky-300",
};

export function AgentBadge({ kind, children, className }: { kind: AgentKind; children?: string; className?: string }) {
  return (
    <span className={cn("inline-flex shrink-0 items-center rounded-full px-1.5 py-px text-[11px] font-medium", STYLES[kind], className)}>
      {children ?? kind}
    </span>
  );
}

const DOTS: Record<AgentKind, string> = {
  claude: "bg-orange-500 dark:bg-orange-400",
  codex: "bg-emerald-500 dark:bg-emerald-400",
  opencode: "bg-sky-500 dark:bg-sky-400",
};
const SPINNERS: Record<AgentKind, string> = {
  claude: "text-orange-500 dark:text-orange-400",
  codex: "text-emerald-500 dark:text-emerald-400",
  opencode: "text-sky-500 dark:text-sky-400",
};

/** The same colour as the badge, for places too tight to spell the agent out; spins in that colour while running. */
export function AgentDot({ kind, running, className }: { kind: AgentKind; running?: boolean; className?: string }) {
  return (
    <span role="img" aria-label={running ? `${kind}, running` : kind} title={kind} className="flex size-3.5 shrink-0 items-center justify-center">
      {running ? (
        <Spinner className={cn("size-3.5", SPINNERS[kind], className)} />
      ) : (
        <span className={cn("size-2 rounded-full", DOTS[kind], className)} />
      )}
    </span>
  );
}
