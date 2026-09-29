import { ReactNode } from "react";
import { ArrowUp, Square } from "lucide-react";
import { AgentInfo, AgentKind } from "@/api";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

interface Props {
  draft: string;
  onDraft: (v: string) => void;
  onSend: () => void;
  onStop: () => void;
  running: boolean;
  starting: boolean;
  /** Why sending isn't possible right now, shown as the send button's tooltip. */
  blockedReason?: string;
  placeholder: string;
  agents: AgentInfo[];
  agent: AgentKind;
  agentLocked: boolean;
  onAgent: (a: AgentKind) => void;
  /** Model choice ("default", "opus", …); only offered for Claude Code. */
  modelChoice?: string;
  onModel?: (m: string) => void;
  /** Extra controls shown before the agent picker (project, branch, …). */
  left?: ReactNode;
  tall?: boolean;
  autoFocus?: boolean;
}

/** The chat input card, shared by the new-session page and the running chat. */
const MODELS = [
  { value: "default", label: "Default model" },
  { value: "opus", label: "Opus" },
  { value: "sonnet", label: "Sonnet" },
  { value: "haiku", label: "Haiku" },
];

export function Composer(p: Props) {
  const items = p.agents.map((a) => ({ value: a.kind, label: a.installed ? a.name : `${a.name} (not installed)` }));
  const canSend = !!p.draft.trim() && !p.blockedReason && !p.starting;
  return (
    <div className="rounded-2xl border bg-card p-2 shadow-sm focus-within:ring-2 focus-within:ring-ring/30">
      <Textarea
        autoFocus={p.autoFocus}
        value={p.draft}
        placeholder={p.placeholder}
        className={cn("resize-none border-0 bg-transparent shadow-none focus-visible:ring-0 dark:bg-transparent", p.tall ? "min-h-24" : "min-h-14")}
        onChange={(e) => p.onDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            if (canSend) p.onSend();
          }
        }}
      />
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 px-1 pt-1">
        {p.left}
        <div className="ml-auto flex items-center gap-2">
          {p.agent === "claude" && p.onModel && (
            <Select items={MODELS} value={p.modelChoice ?? "default"} onValueChange={(v) => v && p.onModel!(v)}>
              <SelectTrigger size="sm" aria-label="Model">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {MODELS.map((m) => (
                  <SelectItem key={m.value} value={m.value}>
                    {m.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Select items={items} value={p.agent} disabled={p.agentLocked} onValueChange={(v) => v && p.onAgent(v as AgentKind)}>
            <SelectTrigger size="sm" aria-label="Agent">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {p.agents.map((a) => (
                <SelectItem key={a.kind} value={a.kind} disabled={!a.installed}>
                  {a.installed ? a.name : `${a.name} (not installed)`}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          {p.running ? (
            <Button variant="outline" size="sm" onClick={p.onStop}>
              <Square data-icon="inline-start" className="fill-current" />
              Stop
            </Button>
          ) : (
            <Button size="sm" onClick={p.onSend} disabled={!canSend} title={p.blockedReason}>
              {p.starting ? <Spinner data-icon="inline-start" /> : <ArrowUp data-icon="inline-start" />}
              Send
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
