import { DragEvent, Fragment, ReactNode, useEffect, useState } from "react";
import { ArrowUp, Brain, ImagePlus, ListPlus, Pencil, Square, X } from "lucide-react";
import { Slider } from "@base-ui/react/slider";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { AgentInfo, AgentKind, ModelInfo, QueuedMessage, listModels } from "@/api";
import { AgentBadge } from "@/components/AgentBadge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { imageFiles, toDataUrl } from "@/lib/images";
import { cn } from "@/lib/utils";

interface Props {
  draft: string;
  onDraft: (v: string) => void;
  /** Images to send with the draft, as `data:` URLs (dropped or pasted in). */
  images: string[];
  onImages: (images: string[]) => void;
  onSend: () => void;
  onStop: () => void;
  /** Messages waiting for the agent to finish; sending while it runs adds to them. */
  queued?: QueuedMessage[];
  onSendQueued?: (id: string) => void;
  /** Takes a message out of the queue; `edit` puts it back in the composer. */
  onRemoveQueued?: (id: string, edit: boolean) => void;
  running: boolean;
  starting: boolean;
  /** Why sending isn't possible right now, shown as the send button's tooltip. */
  blockedReason?: string;
  placeholder: string;
  agents: AgentInfo[];
  agent: AgentKind;
  agentLocked: boolean;
  onAgent: (a: AgentKind) => void;
  /** Model choice ("default", "opus", …); offered for Claude Code and Codex. */
  modelChoice?: string;
  onModel?: (m: string) => void;
  /** Permission mode ("default", "acceptEdits", "auto", …); offered for Claude Code and Codex. */
  permissionMode?: string;
  onPermissionMode?: (m: string) => void;
  /** Thinking effort, one of the model's levels (none for its default); `""` asks for the default. */
  effort?: string;
  onEffort?: (e: string) => void;
  /** The project folder the agent runs in (opencode's models depend on it). */
  cwd?: string;
  /** Extra controls shown before the agent picker (project, branch, …). */
  left?: ReactNode;
  /** Shown next to the model picker (usage). */
  indicator?: ReactNode;
  tall?: boolean;
  autoFocus?: boolean;
}

/** The chat input card, shared by the new-session page and the running chat. */
/** Each agent's models, as the backend lists them (Codex and opencode are asked for their current ones). */
const modelLists = new Map<string, Promise<ModelInfo[]>>();

/** `cwd`: the project folder, which opencode's list depends on (it has none without one). */
function useModels(agent: AgentKind, cwd?: string): ModelInfo[] {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const folder = agent === "opencode" ? cwd?.trim() || undefined : undefined;
  useEffect(() => {
    let current = true;
    setModels([]);
    if (agent === "opencode" && !folder) return;
    const key = `${agent}\u0000${folder ?? ""}`;
    let list = modelLists.get(key);
    if (!list) {
      list = listModels(agent, folder);
      modelLists.set(key, list);
      // Ask again next time rather than keep a failure.
      list.catch(() => modelLists.delete(key));
    }
    list.then((m) => current && setModels(m), () => {});
    return () => void (current = false);
  }, [agent, folder]);
  return models;
}

/** The model picker's choices: the agent's default, its models, and the current choice if it isn't one of them. */
function modelChoices(agent: string, models: ModelInfo[], value: string) {
  const fallback = models.find((m) => m.isDefault);
  const choices = [
    { value: "default", label: "Default model", description: fallback ? `${fallback.name}, ${agent}'s default` : `Whatever ${agent} picks` },
    // `provider/model` ids (opencode) are set apart by provider.
    ...models.map((m, i) => ({
      value: m.id,
      label: m.name,
      description: m.description,
      separated: i > 0 && m.id.includes("/") && m.id.split("/")[0] !== models[i - 1].id.split("/")[0],
    })),
  ];
  if (!choices.some((c) => c.value === value)) choices.push({ value, label: value, description: "Chosen earlier" });
  return choices;
}

/** The model a choice runs: the chosen one, or the agent's default (Claude Code's are alike, so any will do). */
function modelFor(models: ModelInfo[], choice: string, agent: AgentKind): ModelInfo | undefined {
  if (choice !== "default") return models.find((m) => m.id === choice);
  return models.find((m) => m.isDefault) ?? (agent === "claude" ? models[0] : undefined);
}

/**
 * How hard the model thinks: a slider over its effort levels, least to most. Hidden for models
 * without levels; "Default" leaves it to the model.
 */
function EffortPicker({ model, value, onChange }: { model?: ModelInfo; value?: string; onChange: (e: string) => void }) {
  const levels = model?.efforts ?? [];
  if (!levels.length) return null;
  const chosen = value && levels.includes(value) ? value : undefined;
  const fallback = model?.defaultEffort && levels.includes(model.defaultEffort) ? model.defaultEffort : undefined;
  const shown = chosen ?? fallback;
  const at = shown ? levels.indexOf(shown) : Math.floor((levels.length - 1) / 2);
  return (
    <Popover>
      <PopoverTrigger render={<Button variant="outline" size="sm" aria-label="Thinking effort" className="gap-1.5 font-normal" />}>
        <Brain className="size-3.5" />
        {chosen ?? "Default"}
      </PopoverTrigger>
      <PopoverContent align="end" className="w-64">
        <div className="flex items-baseline justify-between">
          <span className="text-xs font-medium text-muted-foreground">Thinking</span>
          <span className="text-xs">{chosen ? chosen : `Default${fallback ? ` (${fallback})` : ""}`}</span>
        </div>
        <Slider.Root
          aria-label="Thinking effort"
          min={0}
          max={levels.length - 1}
          step={1}
          value={at}
          onValueChange={(i) => onChange(levels[i as number])}
          className="px-1.5 pt-1"
        >
          <Slider.Control className="flex h-5 w-full items-center">
            <Slider.Track className="h-1 w-full rounded-full bg-muted">
              <Slider.Indicator className={cn("rounded-full", chosen ? "bg-primary" : "bg-muted-foreground/40")} />
              <Slider.Thumb className="size-4 rounded-full border-2 border-primary bg-background shadow-sm outline-none focus-visible:ring-2 focus-visible:ring-ring" />
            </Slider.Track>
          </Slider.Control>
        </Slider.Root>
        <div className="flex justify-between px-0.5 text-[11px] text-muted-foreground">
          {levels.map((l) => (
            <button key={l} type="button" className={cn("hover:text-foreground", l === shown && "font-medium text-foreground")} onClick={() => onChange(l)}>
              {l}
            </button>
          ))}
        </div>
        <Button variant="ghost" size="sm" className="self-start" disabled={!chosen} onClick={() => onChange("")}>
          Use the model's default
        </Button>
      </PopoverContent>
    </Popover>
  );
}

/** Each agent's permission modes, worded as its own picker does; the first is what "none chosen" means. */
const PERMISSION_MODES: Partial<Record<AgentKind, Choice[]>> = {
  claude: [
    { value: "auto", label: "Auto", description: "Claude handles permission decisions" },
    { value: "default", label: "Manual", description: "Always ask before making changes" },
    { value: "acceptEdits", label: "Accept edits", description: "Automatically accept all file edits" },
    { value: "plan", label: "Plan", description: "Create a plan before making changes" },
    { value: "bypassPermissions", label: "Bypass permissions", description: "Run everything without asking" },
  ],
  codex: [
    { value: "auto", label: "Auto", description: "Edit the project; ask before anything else" },
    { value: "readOnly", label: "Read only", description: "Ask before any change" },
    { value: "plan", label: "Plan", description: "Create a plan before making changes" },
    { value: "fullAccess", label: "Full access", description: "Run everything without asking" },
  ],
  // opencode has no sandbox, so even its default asks before shell commands.
  opencode: [
    { value: "auto", label: "Auto", description: "Edit the project; ask before shell commands" },
    { value: "readOnly", label: "Manual", description: "Ask before edits and shell commands" },
    { value: "plan", label: "Plan", description: "Create a plan before making changes" },
    { value: "fullAccess", label: "Full access", description: "Run everything without asking" },
  ],
};

/** The mode a session is in: its choice if the agent has it, or what the agent does by default. */
function modeValue(agent: AgentKind, mode?: string): string {
  const modes = PERMISSION_MODES[agent] ?? [];
  if (agent === "claude") return mode ?? "default";
  return modes.some((m) => m.value === mode) ? mode! : (modes[0]?.value ?? "default");
}

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
  /** Shown in the trigger before the value. */
  icon?: ReactNode;
  className?: string;
  onChange: (v: string) => void;
}

/** A small picker whose menu explains each choice, in the style of Claude Code's own. */
export function Picker({ heading, label, choices, value, placeholder, disabled, mono, align = "end", icon, className, onChange }: PickerProps) {
  return (
    <Select items={choices} value={value} disabled={disabled} onValueChange={(v) => v && onChange(v)}>
      <SelectTrigger size="sm" aria-label={label} className={cn(mono && "font-mono text-xs", className)}>
        {icon}
        <SelectValue placeholder={placeholder}>
          {/* The value box is a flex row (see SelectTrigger), which can't show an ellipsis; a block child can. */}
          {(v: string | null) => <span className="block max-w-40 truncate">{choices.find((c) => c.value === v)?.label ?? placeholder}</span>}
        </SelectValue>
      </SelectTrigger>
      <SelectContent className="w-max min-w-64 max-w-96" align={align} alignItemWithTrigger={false}>
        <SelectGroup>
          <SelectLabel>{heading}</SelectLabel>
          {choices.map((c) => (
            <Fragment key={c.value}>
              {c.separated && <SelectSeparator />}
              <SelectItem value={c.value} disabled={c.disabled} className="py-1.5">
                {c.icon}
                <span className="flex flex-col gap-0.5">
                  <span className={cn("whitespace-normal break-all", mono && "font-mono text-xs")}>{c.label}</span>
                  <span className="font-sans text-xs font-normal whitespace-normal text-muted-foreground">{c.description}</span>
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
  const canSend = (!!p.draft.trim() || p.images.length > 0) && !p.blockedReason && !p.starting;
  const agentName = p.agents.find((a) => a.kind === p.agent)?.name ?? p.agent;
  const modes = PERMISSION_MODES[p.agent];
  const models = useModels(p.agent, p.cwd);
  const [dragging, setDragging] = useState(false);
  const [imageError, setImageError] = useState<string>();

  async function attach(files: File[]) {
    if (!files.length) return;
    setImageError(undefined);
    const read = await Promise.allSettled(files.map(toDataUrl));
    const ok = read.flatMap((r) => (r.status === "fulfilled" ? [r.value] : []));
    if (ok.length < files.length) setImageError("Some images couldn't be read.");
    p.onImages([...p.images, ...ok]);
  }

  const hasFiles = (e: DragEvent) => e.dataTransfer.types.includes("Files");
  const onDragOver = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    setDragging(true);
  };
  const onDrop = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    setDragging(false);
    void attach(imageFiles(e.dataTransfer.files));
  };

  return (
    <div className="flex flex-col gap-2">
      {/* Where the message goes: project and branch, above the card. */}
      {p.left && <div className="flex flex-wrap items-center gap-x-3 gap-y-2 px-1">{p.left}</div>}
      {!!p.queued?.length && <QueuedList {...p} queued={p.queued} />}
      {/* The message on its own: a card with just the text and the Send button. */}
      <div
        className={cn(
          "relative flex flex-col gap-2 rounded-2xl border bg-card p-2 shadow-sm focus-within:ring-2 focus-within:ring-ring/30",
          dragging && "ring-2 ring-primary/50",
        )}
        onDragOver={onDragOver}
        onDragLeave={(e) => {
          // Leaving for one of the card's own children isn't leaving.
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        }}
        onDrop={onDrop}
      >
        {dragging && (
          <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center gap-2 rounded-2xl bg-card/90 text-sm text-muted-foreground">
            <ImagePlus className="size-4" /> Drop images to attach
          </div>
        )}
        {p.images.length > 0 && (
          <div className="flex flex-wrap gap-2 px-1 pt-1">
            {p.images.map((src, i) => (
              <div key={i} className="group/img relative">
                <img src={src} alt={`Attached image ${i + 1}`} className="size-16 rounded-lg border object-cover" />
                <button
                  type="button"
                  title="Remove image"
                  aria-label={`Remove image ${i + 1}`}
                  className="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full border bg-background text-muted-foreground opacity-0 shadow-sm group-hover/img:opacity-100 hover:text-foreground focus-visible:opacity-100"
                  onClick={() => p.onImages(p.images.filter((_, j) => j !== i))}
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
          </div>
        )}
        {imageError && <p className="px-1 text-xs text-destructive">{imageError}</p>}
        <div className="flex items-end gap-2">
          <Textarea
            autoFocus={p.autoFocus}
            value={p.draft}
            placeholder={p.placeholder}
            className={cn("resize-none border-0 bg-transparent shadow-none focus-visible:ring-0 dark:bg-transparent", p.tall ? "min-h-24" : "min-h-14")}
            onChange={(e) => p.onDraft(e.target.value)}
            onPaste={(e) => {
              const files = imageFiles(e.clipboardData.files);
              if (!files.length) return;
              e.preventDefault();
              void attach(files);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                if (canSend) p.onSend();
              }
            }}
          />
          {p.running ? (
            <div className="flex gap-2">
              {(p.draft.trim() || p.images.length > 0) && (
                <Button size="sm" onClick={p.onSend} disabled={!canSend} title="Send once the agent is done">
                  <ListPlus data-icon="inline-start" />
                  Queue
                </Button>
              )}
              <Button variant="outline" size="sm" onClick={p.onStop}>
                <Square data-icon="inline-start" className="fill-current" />
                Stop
              </Button>
            </div>
          ) : (
            <Button size="sm" onClick={p.onSend} disabled={!canSend} title={p.blockedReason}>
              {p.starting ? <Spinner data-icon="inline-start" /> : <ArrowUp data-icon="inline-start" />}
              Send
            </Button>
          )}
        </div>
      </div>
      {/* How it runs: usage, mode, model and agent, below the card. */}
      <div className="flex flex-wrap items-center justify-end gap-2 px-1">
        {p.indicator}
        {modes && p.onPermissionMode && (
          <Picker heading="Mode" label="Permission mode" choices={modes} value={modeValue(p.agent, p.permissionMode)} onChange={p.onPermissionMode} />
        )}
        {modes && p.onModel && (
          <Picker
            heading="Model"
            label="Model"
            choices={modelChoices(agentName, models, p.modelChoice ?? "default")}
            value={p.modelChoice ?? "default"}
            onChange={(m) => {
              p.onModel?.(m);
              // A level the new model doesn't have goes back to its default.
              if (p.effort && !(modelFor(models, m, p.agent)?.efforts ?? []).includes(p.effort)) p.onEffort?.("");
            }}
          />
        )}
        {modes && p.onEffort && <EffortPicker model={modelFor(models, p.modelChoice ?? "default", p.agent)} value={p.effort} onChange={p.onEffort} />}
        {p.agentLocked ? (
          // A session keeps its agent, so there's nothing to choose; say which it is instead of a dead menu.
          <AgentBadge kind={p.agent} className="h-7 px-2.5 text-[0.8rem]">
            {p.agents.find((a) => a.kind === p.agent)?.name ?? p.agent}
          </AgentBadge>
        ) : (
          <Picker heading="Agent" label="Agent" choices={agents} value={p.agent} onChange={(v) => p.onAgent(v as AgentKind)} />
        )}
      </div>
    </div>
  );
}

/** The messages waiting their turn, each of which can be sent now, edited or dropped. */
function QueuedList(p: Props & { queued: QueuedMessage[] }) {
  return (
    <div className="flex flex-col gap-1 rounded-xl border border-dashed px-3 py-2">
      <p className="text-xs text-muted-foreground">
        {p.running ? "Queued: sent when the agent is done" : "Queued, paused: the last run was stopped or didn't finish"}
      </p>
      {p.queued.map((m) => (
        <div key={m.id} className="group flex items-start gap-2 text-sm">
          {!!m.images?.length && (
            <div className="flex shrink-0 gap-1 py-1">
              {m.images.map((src, i) => (
                <img key={i} src={src} alt={`Queued image ${i + 1}`} className="size-8 rounded border object-cover" />
              ))}
            </div>
          )}
          <span className="line-clamp-2 min-w-0 flex-1 py-1 whitespace-pre-wrap">{m.text}</span>
          {!p.running && !p.blockedReason && (
            <Button variant="ghost" size="icon-sm" title="Send now" aria-label="Send now" onClick={() => p.onSendQueued?.(m.id)}>
              <ArrowUp />
            </Button>
          )}
          <Button variant="ghost" size="icon-sm" title="Edit" aria-label="Edit" onClick={() => p.onRemoveQueued?.(m.id, true)}>
            <Pencil />
          </Button>
          <Button variant="ghost" size="icon-sm" title="Remove" aria-label="Remove" onClick={() => p.onRemoveQueued?.(m.id, false)}>
            <X />
          </Button>
        </div>
      ))}
    </div>
  );
}
