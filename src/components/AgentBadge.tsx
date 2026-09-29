import { AgentKind } from "@/api";
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
