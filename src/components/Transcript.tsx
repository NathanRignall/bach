import { createContext, useContext, useEffect, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { AlertCircle, Ban, MessageCircleQuestion, Bot, Brain, CheckCircle2, ChevronRight, RotateCcw, ShieldAlert, ShieldCheck, ShieldX, Wrench, XCircle } from "lucide-react";
import { AgentKind, Decision } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { ApprovalBlock, Block, ToolBlock, isSubagent } from "@/session";

/** What the transcript can ask the app to do on the user's behalf. */
export interface TranscriptActions {
  decide: (requestId: string, decision: Decision, answers?: Record<string, string>) => Promise<void>;
  /** Sends a message again; with no text, the last one. Not offered while a run is going. */
  retry?: (text?: string, images?: string[]) => void;
  /** The agent the transcript is with. */
  agent?: AgentKind;
}
export const TranscriptContext = createContext<TranscriptActions>({ decide: async () => {} });


const summaryClass =
  "group/trigger flex w-full items-center gap-2 rounded-lg border bg-card px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent";
const preClass = "max-h-72 overflow-auto rounded-lg border bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all";
const chevron = "size-3.5 shrink-0 transition-transform group-data-[panel-open]/trigger:rotate-90";

/** `mcp__satie__task_start` -> `satie · task_start`; other names are unchanged. */
function toolLabel(name: string): string {
  const m = /^mcp__(.+?)__(.+)$/.exec(name);
  return m ? `${m[1]} · ${m[2]}` : name;
}

function formatDuration(ms: number) {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

function formatTokens(n: number) {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

/** Counts up from `since`, so a long-running call visibly is still alive. */
function Elapsed({ since }: { since?: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return since ? <span className="tabular-nums">{formatDuration(now - since)}</span> : null;
}

/**
 * `live` is whether the session is running right now. A call with no result while the session
 * isn't running was cut off (stopped, or the server restarted), so it must not spin forever.
 */
export function BlockView({ block, live }: { block: Block; live: boolean }) {
  switch (block.kind) {
    case "user":
      return <UserMessage text={block.text} images={block.images} />;

    case "text":
      return (
        <div className="prose prose-sm max-w-none dark:prose-invert prose-a:text-primary prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5 prose-code:font-normal prose-code:before:content-none prose-code:after:content-none prose-pre:border prose-pre:bg-muted prose-pre:text-foreground prose-pre:[&_code]:bg-transparent prose-pre:[&_code]:p-0">
          <Markdown
            remarkPlugins={[remarkGfm]}
            components={{ a: (props) => <a {...props} target="_blank" rel="noopener noreferrer" /> }}
          >
            {block.text}
          </Markdown>
        </div>
      );

    case "thinking":
      return (
        <Collapsible>
          <CollapsibleTrigger className={summaryClass}>
            <ChevronRight className={chevron} />
            <Brain className="size-3.5" />
            Thinking
          </CollapsibleTrigger>
          <CollapsibleContent>
            <p className="mt-2 px-3 text-xs whitespace-pre-wrap text-muted-foreground">{block.text}</p>
          </CollapsibleContent>
        </Collapsible>
      );

    case "note":
      return <p className="text-center text-xs text-muted-foreground">{block.text}</p>;

    case "error":
      return <ErrorMessage text={block.text} retryText={block.retryText} />;

    case "approval":
      return block.toolName === "AskUserQuestion" ? (
        <QuestionCard block={block} live={live} />
      ) : (
        <ApprovalCard block={block} live={live} />
      );

    case "tool":
      return isSubagent(block) ? <SubagentCard block={block} live={live} /> : <ToolCard block={block} live={live} />;
  }
}

/**
 * Images a tool returned (under its card) or the user sent (above their message); click one to
 * see it full size.
 */
function Images({ images, alt, className, thumbClass = "max-h-72" }: { images: string[]; alt: string; className?: string; thumbClass?: string }) {
  const [open, setOpen] = useState<string>();
  return (
    <>
      <div className={cn("flex flex-wrap gap-2", className)}>
        {images.map((src, i) => (
          <button key={i} onClick={() => setOpen(src)} title="View full size" className="overflow-hidden rounded-lg border bg-muted">
            <img src={src} alt={alt} className={cn("block max-w-full object-contain", thumbClass)} />
          </button>
        ))}
      </div>
      <Dialog open={!!open} onOpenChange={(o) => !o && setOpen(undefined)}>
        <DialogContent className="max-h-[90vh] w-auto max-w-[90vw] overflow-auto p-2 sm:max-w-[90vw]">
          <DialogTitle className="sr-only">{alt}</DialogTitle>
          {open && <img src={open} alt={alt} className="block max-h-[85vh] max-w-full object-contain select-text" />}
        </DialogContent>
      </Dialog>
    </>
  );
}

function ToolCard({ block, live }: { block: ToolBlock; live: boolean }) {
  const pending = block.output === undefined;
  return (
    <div>
    <Collapsible>
      <CollapsibleTrigger className={summaryClass}>
        <ChevronRight className={chevron} />
        <Wrench className="size-3.5 shrink-0" />
        <span className="font-semibold text-foreground">{toolLabel(block.name)}</span>
        <code className="min-w-0 flex-1 truncate font-mono">{JSON.stringify(block.input)}</code>
        {pending && live ? (
          <>
            <Elapsed since={block.startedAt} />
            <Spinner className="size-3.5 shrink-0 text-primary" />
          </>
        ) : pending ? (
          <Ban className="size-3.5 shrink-0" aria-label="Interrupted" />
        ) : block.isError ? (
          <XCircle className="size-3.5 shrink-0 text-destructive" aria-label="Failed" />
        ) : (
          <CheckCircle2 className="size-3.5 shrink-0 text-muted-foreground" aria-label="Done" />
        )}
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-2 flex flex-col gap-2">
        <pre className={preClass}>{JSON.stringify(block.input, null, 2)}</pre>
        {!pending && (block.output || !block.images?.length) && (
          <pre className={preClass + (block.isError ? " border-destructive/40" : "")}>{block.output}</pre>
        )}
      </CollapsibleContent>
    </Collapsible>
    {!!block.images?.length && <Images images={block.images} alt={`Image from ${toolLabel(block.name)}`} className="mt-2" />}
    </div>
  );
}

/** A sub-agent (or other long-running task): live status, counters, and its own nested steps. */
function SubagentCard({ block, live }: { block: ToolBlock; live: boolean }) {
  const t = block.task ?? {};
  const input = (block.input ?? {}) as { description?: string; subagent_type?: string };
  const title = t.title ?? input.description ?? block.name;
  const agentType = t.agentType ?? input.subagent_type;

  // A backgrounded task returns its tool result at once, so trust the task status when present.
  const status = t.status ?? (block.output !== undefined ? (block.isError ? "failed" : "completed") : "running");
  const running = status === "running" && live;
  const stopped = status === "running" && !live;
  const failed = status === "failed" || status === "error";

  // Open while it runs so progress is visible; the user can still collapse it.
  const [open, setOpen] = useState(running);
  useEffect(() => {
    if (running) setOpen(true);
  }, [running]);

  const stats = [
    t.toolUses !== undefined && `${t.toolUses} tool${t.toolUses === 1 ? "" : "s"}`,
    t.tokens !== undefined && `${formatTokens(t.tokens)} tokens`,
  ].filter(Boolean);
  const result = t.summary ?? block.output;

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className={summaryClass + " py-2"}>
        <ChevronRight className={chevron} />
        <Bot className="size-4 shrink-0 text-foreground" />
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">{title}</span>
        {agentType && <Badge variant="outline">{agentType}</Badge>}
        {stats.length > 0 && <span className="hidden shrink-0 sm:inline">{stats.join(" · ")}</span>}
        {running ? (
          <>
            <Elapsed since={block.startedAt} />
            <Spinner className="size-3.5 shrink-0 text-primary" />
          </>
        ) : stopped ? (
          <Ban className="size-3.5 shrink-0" aria-label="Interrupted" />
        ) : failed ? (
          <XCircle className="size-3.5 shrink-0 text-destructive" aria-label="Failed" />
        ) : (
          <>
            {t.durationMs !== undefined && <span className="tabular-nums">{formatDuration(t.durationMs)}</span>}
            <CheckCircle2 className="size-3.5 shrink-0" aria-label="Done" />
          </>
        )}
      </CollapsibleTrigger>

      {running && t.activity && <p className="mt-1 truncate pl-9 text-xs text-muted-foreground">{t.activity}</p>}

      <CollapsibleContent className="mt-2 flex flex-col gap-3 border-l-2 pl-4">
        {block.children?.map((c, i) => <BlockView key={i} block={c} live={live} />)}
        {result && (
          <div className="rounded-lg bg-muted p-3 text-xs whitespace-pre-wrap break-words">
            <p className="mb-1 font-medium text-muted-foreground">Result</p>
            {result}
          </div>
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

function UserMessage({ text, images }: { text: string; images?: string[] }) {
  const { retry } = useContext(TranscriptContext);
  return (
    <div className="group/msg ml-auto flex max-w-[85%] items-center gap-1.5">
      {retry && (
        <Button
          variant="ghost"
          size="icon-xs"
          className="opacity-0 group-hover/msg:opacity-100 focus-visible:opacity-100"
          title="Send this message again"
          aria-label="Retry message"
          onClick={() => retry(text, images)}
        >
          <RotateCcw />
        </Button>
      )}
      <div className="flex flex-col items-end gap-1.5">
        {!!images?.length && <Images images={images} alt="Image you sent" className="justify-end" thumbClass="max-h-40" />}
        {text && <div className="w-fit rounded-2xl bg-secondary px-4 py-2 text-sm whitespace-pre-wrap">{text}</div>}
      </div>
    </div>
  );
}

function ErrorMessage({ text, retryText }: { text: string; retryText?: string }) {
  const { retry } = useContext(TranscriptContext);
  return (
    <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
      <AlertCircle className="mt-0.5 size-4 shrink-0" />
      <span className="min-w-0 flex-1 whitespace-pre-wrap">{text}</span>
      {retry && (
        <Button variant="outline" size="xs" onClick={() => retry(retryText)}>
          <RotateCcw data-icon="inline-start" />
          Retry
        </Button>
      )}
    </div>
  );
}

const AGENT_NAMES: Record<AgentKind, string> = { claude: "Claude", codex: "Codex", opencode: "opencode" };

/** What the tool would do, in a form worth reading before saying yes. */
function describeInput(input: unknown): { main?: string; rest?: string } {
  if (!input || typeof input !== "object") return { main: String(input ?? "") };
  // `description` is shown on its own above the preview; a diff (Codex's file changes) as is.
  const { command, file_path, path, description: _d, diff, ...others } = input as Record<string, unknown>;
  const main = [command, file_path, path].find((v): v is string => typeof v === "string");
  const json = Object.keys(others).length ? JSON.stringify(others, null, 2) : undefined;
  const rest = typeof diff === "string" && diff ? diff : json;
  return { main: main ?? (rest ? undefined : ""), rest: main ? rest : (rest ?? JSON.stringify(input, null, 2)) };
}

/** Where "Always allow" keeps the rule, or nothing when the agent has no lasting rule for it. */
function alwaysSavesTo(agent: AgentKind, toolName: string): string | undefined {
  if (agent === "codex") return toolName === "Edit" ? undefined : "Adds the command to Codex's own rules (~/.codex/rules)";
  return "Saves to this project's settings";
}

const DECIDED: Record<string, string> = {
  allow: "Allowed once",
  allow_session: "Allowed for this session",
  allow_always: "Always allowed",
  deny: "Denied",
  expired: "No longer needed",
};

/** A tool the agent wants to use. It waits here until the user answers. */
function ApprovalCard({ block, live }: { block: ApprovalBlock; live: boolean }) {
  const { decide, agent = "claude" } = useContext(TranscriptContext);
  const [busy, setBusy] = useState(false);
  const { main, rest } = describeInput(block.input);
  const always = alwaysSavesTo(agent, block.toolName);

  if (block.decision || !live) {
    const decision = block.decision ?? "expired";
    const Icon = decision === "deny" ? ShieldX : decision === "expired" ? Ban : ShieldCheck;
    return (
      <p className="flex items-center gap-2 px-1 text-xs text-muted-foreground" title={block.rules.join("\n")}>
        <Icon className="size-3.5 shrink-0" />
        <span>{DECIDED[decision]}</span>
        <span className="font-semibold text-foreground">{toolLabel(block.toolName)}</span>
        <code className="min-w-0 flex-1 truncate font-mono">{main ?? rest?.replace(/\s+/g, " ")}</code>
      </p>
    );
  }

  const answer = async (d: Decision) => {
    setBusy(true);
    await decide(block.requestId, d).finally(() => setBusy(false));
  };

  return (
    <div role="group" aria-label="Approval needed" className="flex flex-col gap-3 rounded-xl border border-primary/40 bg-primary/5 p-4">
      <div className="flex items-center gap-2 text-sm">
        <ShieldAlert className="size-4 shrink-0 text-primary" />
        <span>
          {AGENT_NAMES[agent]} wants to use <span className="font-semibold">{toolLabel(block.toolName)}</span>
        </span>
        {block.reason && <span className="text-xs text-muted-foreground">· {block.reason}</span>}
      </div>
      {block.description && <p className="text-sm text-muted-foreground">{block.description}</p>}
      {block.directories.length > 0 && (
        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <AlertCircle className="mt-0.5 size-3.5 shrink-0 text-primary" />
          <span>
            This also reaches outside the project folder: <span className="font-mono">{block.directories.join(", ")}</span>. Allowing it
            never grants access to that folder beyond this command.
          </span>
        </p>
      )}
      {main && <pre className={preClass + " bg-background"}>{main}</pre>}
      {rest && <pre className={preClass + " bg-background"}>{rest}</pre>}
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={busy} onClick={() => void answer("allow")}>
          Allow
        </Button>
        {block.rules.length > 0 && (
          <>
            <Button variant="outline" size="sm" disabled={busy} title={block.rules.join("\n")} onClick={() => void answer("allow_session")}>
              Allow for this session
            </Button>
            {always && (
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                title={`${always}:\n` + block.rules.join("\n")}
                onClick={() => void answer("allow_always")}
              >
                Always allow
              </Button>
            )}
          </>
        )}
        <Button variant="ghost" size="sm" className="ml-auto text-destructive" disabled={busy} onClick={() => void answer("deny")}>
          Deny
        </Button>
      </div>
    </div>
  );
}

interface Question {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: { label: string; description?: string }[];
}

const OTHER = "\u0000other"; // can't collide with an option label

/** A question from the agent (its AskUserQuestion tool), answered by picking options or typing. */
function QuestionCard({ block, live }: { block: ApprovalBlock; live: boolean }) {
  const { decide, agent = "claude" } = useContext(TranscriptContext);
  const questions = ((block.input as { questions?: Question[] })?.questions ?? []).filter((q) => q.question);
  // Chosen labels per question (OTHER = the free-text choice) and the free text itself.
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  const answerFor = (q: Question): string => {
    const labels = (picked[q.question] ?? []).flatMap((l) => (l === OTHER ? [(other[q.question] ?? "").trim()] : [l]));
    return labels.filter(Boolean).join(", ");
  };

  if (block.decision || !live) {
    const decision = block.decision ?? "expired";
    const answered = decision === "allow" && block.answers;
    return (
      <div className="flex flex-col gap-1 px-1 text-xs text-muted-foreground">
        {answered ? (
          Object.entries(block.answers!).map(([q, a]) => (
            <p key={q} className="flex items-start gap-2">
              <MessageCircleQuestion className="mt-0.5 size-3.5 shrink-0" />
              <span>
                {q} <span className="font-semibold text-foreground">→ {a}</span>
              </span>
            </p>
          ))
        ) : (
          <p className="flex items-center gap-2">
            <Ban className="size-3.5 shrink-0" />
            {decision === "deny" ? "Skipped a question" : "Question no longer needed"}
          </p>
        )}
      </div>
    );
  }

  const complete = questions.length > 0 && questions.every((q) => answerFor(q));
  const toggle = (q: Question, label: string) =>
    setPicked((p) => {
      const cur = p[q.question] ?? [];
      const next = q.multiSelect ? (cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label]) : [label];
      return { ...p, [q.question]: next };
    });
  const submit = async () => {
    setBusy(true);
    const answers = Object.fromEntries(questions.map((q) => [q.question, answerFor(q)]));
    await decide(block.requestId, "allow", answers).finally(() => setBusy(false));
  };

  return (
    <div role="group" aria-label="Question from the agent" className="flex flex-col gap-4 rounded-xl border border-primary/40 bg-primary/5 p-4">
      <div className="flex items-center gap-2 text-sm">
        <MessageCircleQuestion className="size-4 shrink-0 text-primary" />
        {AGENT_NAMES[agent]} has {questions.length === 1 ? "a question" : "some questions"}
      </div>

      {questions.map((q) => {
        const on = picked[q.question] ?? [];
        const type = q.multiSelect ? "checkbox" : "radio";
        return (
          <fieldset key={q.question} className="flex flex-col gap-2">
            <legend className="mb-1 flex items-center gap-2 text-sm font-medium">
              {q.header && <Badge variant="outline">{q.header}</Badge>}
              {q.question}
            </legend>
            {q.options.map((o) => (
              <label
                key={o.label}
                className="flex cursor-pointer items-start gap-3 rounded-lg border bg-background px-3 py-2 text-sm hover:bg-accent has-[:checked]:border-primary"
              >
                <input type={type} name={q.question} className="mt-1 accent-primary" checked={on.includes(o.label)} onChange={() => toggle(q, o.label)} />
                <span className="flex flex-col">
                  <span className="font-medium">{o.label}</span>
                  {o.description && <span className="text-xs text-muted-foreground">{o.description}</span>}
                </span>
              </label>
            ))}
            <label className="flex cursor-pointer items-center gap-3 rounded-lg border bg-background px-3 py-2 text-sm hover:bg-accent has-[:checked]:border-primary">
              <input type={type} name={q.question} className="accent-primary" checked={on.includes(OTHER)} onChange={() => toggle(q, OTHER)} />
              <span className="font-medium">Other</span>
              <Input
                aria-label={`Other answer for: ${q.question}`}
                className="h-7 flex-1"
                placeholder="Type your own answer"
                value={other[q.question] ?? ""}
                onFocus={() => !on.includes(OTHER) && toggle(q, OTHER)}
                onChange={(e) => setOther((o) => ({ ...o, [q.question]: e.target.value }))}
              />
            </label>
          </fieldset>
        );
      })}

      <div className="flex items-center gap-2">
        <Button size="sm" disabled={!complete || busy} onClick={() => void submit()}>
          Send answer
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => void decide(block.requestId, "deny")}>
          Skip
        </Button>
      </div>
    </div>
  );
}
