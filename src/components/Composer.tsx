import { Fragment, ReactNode } from "react";
import { ArrowUp, Square } from "lucide-react";
import { AgentInfo, AgentKind } from "@/api";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
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
  /** Permission mode ("default", "acceptEdits", "auto", …); only offered for Claude Code. */
  permissionMode?: string;
  onPermissionMode?: (m: string) => void;
  /** Extra controls shown before the agent picker (project, branch, …). */
  left?: ReactNode;
  /** Shown next to the model picker (usage). */
  indicator?: ReactNode;
  tall?: boolean;
  autoFocus?: boolean;
}

/** The chat input card, shared by the new-session page and the running chat. */
const MODELS = [
  { value: "default", label: "Default model", description: "Whatever Claude Code picks" },
  { value: "opus", label: "Opus", description: "Most capable, for hard problems" },
  { value: "sonnet", label: "Sonnet", description: "Fast and capable for everyday work" },
  { value: "haiku", label: "Haiku", description: "Fastest, for small tasks" },
];

/** Claude Code's `--permission-mode` choices, worded as its own picker does. */
const PERMISSION_MODES = [
  { value: "auto", label: "Auto", description: "Claude handles permission decisions" },
  { value: "default", label: "Manual", description: "Always ask before making changes" },
  { value: "acceptEdits", label: "Accept edits", description: "Automatically accept all file edits" },
  { value: "plan", label: "Plan", description: "Create a plan before making changes" },
  { value: "bypassPermissions", label: "Bypass permissions", description: "Run everything without asking" },
];

const AGENT_DESCRIPTIONS: Record<AgentKind, string> = {
  claude: "Anthropic's coding agent",
  codex: "OpenAI's coding agent",
  opencode: "The open-source coding agent",
};

export interface Choice {
  value: string;
  label: string;
  description: string;
  disabled?: boolean;
  /** Shown before the label in the menu. */
  icon?: ReactNode;
  /** Set apart from the choices above it. */
  separated?: boolean;
}

interface PickerProps {
  heading: string;
  label: string;
  choices: Choice[];
  value: string | null;
  placeholder?: string;
  disabled?: boolean;
  /** Menu and trigger in monospace (branch names). */
  mono?: boolean;
  /** Where the menu opens relative to the trigger. */
  align?: "start" | "end";
  className?: string;
  onChange: (v: string) => void;
}

/** A small picker whose menu explains each choice, in the style of Claude Code's own. */
export function Picker({ heading, label, choices, value, placeholder, disabled, mono, align = "end", className, onChange }: PickerProps) {
  return (
    <Select items={choices} value={value} disabled={disabled} onValueChange={(v) => v && onChange(v)}>
      <SelectTrigger size="sm" aria-label={label} className={cn(mono && "font-mono text-xs", className)}>
        <SelectValue placeholder={placeholder} />
      </SelectTrigger>
      <SelectContent className="min-w-64" align={align} alignItemWithTrigger={false}>
        <SelectGroup>
          <SelectLabel>{heading}</SelectLabel>
          {choices.map((c) => (
            <Fragment key={c.value}>
              {c.separated && <SelectSeparator />}
              <SelectItem value={c.value} disabled={c.disabled} className="py-1.5">
                {c.icon}
                <span className="flex flex-col gap-0.5">
                  <span className={cn(mono && "font-mono text-xs")}>{c.label}</span>
                  <span className="font-sans text-xs font-normal text-muted-foreground">{c.description}</span>
                </span>
              </SelectItem>
            </Fragment>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
}

export function Composer(p: Props) {
  const agents: Choice[] = p.agents.map((a) => ({
    value: a.kind,
    label: a.installed ? a.name : `${a.name} (not installed)`,
    description: AGENT_DESCRIPTIONS[a.kind],
    disabled: !a.installed,
  }));
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
          {p.indicator}
          {p.agent === "claude" && p.onPermissionMode && (
            <Picker heading="Mode" label="Permission mode" choices={PERMISSION_MODES} value={p.permissionMode ?? "default"} onChange={p.onPermissionMode} />
          )}
          {p.agent === "claude" && p.onModel && <Picker heading="Model" label="Model" choices={MODELS} value={p.modelChoice ?? "default"} onChange={p.onModel} />}
          <Picker heading="Agent" label="Agent" choices={agents} value={p.agent} disabled={p.agentLocked} onChange={(v) => p.onAgent(v as AgentKind)} />

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
