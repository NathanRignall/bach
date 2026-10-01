import { createContext, useContext, useEffect, useId, useState, useSyncExternalStore } from "react";
import Markdown, { type Components, defaultUrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import { AlertCircle, ArrowDown, ArrowUp, Ban, MessageCircleQuestion, Bot, Brain, CheckCircle2, ChevronRight, Cog, EyeOff, Folder, GitBranch, ImageOff, Network, MessageSquarePlus, RotateCcw, ScrollText, ServerCog, ShieldAlert, ShieldCheck, ShieldX, Wrench, XCircle } from "lucide-react";
import { AgentKind, Decision, Forwarding, TaskView, readImage } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { describe } from "@/lib/attachments";
import { ImageLinks, Place, reveal } from "@/lib/imageLinks";
import { cn } from "@/lib/utils";
import { FileChip } from "@/components/FileChip";
import { PortLink } from "@/components/Ports";
import { STATUS, StatusIcon } from "@/components/TasksPanel";
import { ApprovalBlock, Block, ToolBlock, isSubagent } from "@/session";

/** What the transcript can ask the app to do on the user's behalf. */
export interface TranscriptActions {
  decide: (requestId: string, decision: Decision, answers?: Record<string, string>) => Promise<void>;
  /** Sends a message again; with no text, the last one. Not offered while a run is going. */
  retry?: (text?: string, images?: string[]) => void;
  /** The agent the transcript is with. */
  agent?: AgentKind;
  /** Output so far of tool calls still running, by call id. */
  outputs?: Record<string, string>;
  /** Background tasks, so a task tool call can show the live state of the task it touched. */
  tasks?: TaskView[];
  forwarding?: Forwarding | null;
  /** Opens the tasks panel on this task's log. */
  showTask?: (id: string) => void;
  /** Opens another session (one this transcript's agent started). */
  showSession?: (id: string) => void;
  /** The session works in a git repository (so sessions it starts get worktrees by default). */
  inRepo?: boolean;
  /** The images shown so far, so a tool's image and the same one in the reply can point at each other. */
  images?: ImageLinks;
}
export const TranscriptContext = createContext<TranscriptActions>({ decide: async () => {} });

/** Where the block being shown sits in the transcript; unset outside one (no linking then). */
export const BlockPlace = createContext<Place | undefined>(undefined);

/** Re-renders when the transcript's images change, so links appear as their other half loads. */
function useImageLinks(): ImageLinks | undefined {
  const { images } = useContext(TranscriptContext);
  useSyncExternalStore(images?.subscribe ?? noSubscribe, images?.snapshot ?? noSnapshot);
  return images;
}
const noSubscribe = () => () => {};
const noSnapshot = () => 0;


const summaryClass =
  "group/trigger flex w-full items-center gap-2 rounded-lg border bg-card px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent";
const preClass = "max-h-72 overflow-auto rounded-lg border bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all";
const proseClass =
  "prose prose-sm max-w-none dark:prose-invert prose-a:text-primary prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5 prose-code:font-normal prose-code:before:content-none prose-code:after:content-none prose-pre:border prose-pre:bg-muted prose-pre:text-foreground prose-pre:[&_code]:bg-transparent prose-pre:[&_code]:p-0";
const chevron = "size-3.5 shrink-0 transition-transform group-data-[panel-open]/trigger:rotate-90";

/** react-markdown's URL sanitising, except that `file://` images survive for {@link MarkdownImage}. */
const keepFileUrls = (url: string, key: string) => (key === "src" && url.startsWith("file://") ? url : defaultUrlTransform(url));

// Defined once: a new component per render would remount every image in the transcript.
const markdownComponents: Components = {
  a: (props) => <a {...props} target="_blank" rel="noopener noreferrer" />,
  img: ({ src, alt }) => <MarkdownImage key={String(src)} src={typeof src === "string" ? src : undefined} alt={alt} />,
};

/** Markdown, as the agent's messages show it. */
function Prose({ text, className }: { text: string; className?: string }) {
  return (
    <div className={proseClass + (className ? " " + className : "")}>
      <Markdown remarkPlugins={[remarkGfm]} urlTransform={keepFileUrls} components={markdownComponents}>
        {text}
      </Markdown>
    </div>
  );
}

/** `mcp__github__get_issue` -> `github · get_issue`; other names are unchanged. */
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
        <Prose text={block.text} />
      );

    case "thinking":
      // Open while it's being written, so it can be followed.
      return (
        <Collapsible defaultOpen={block.streaming}>
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
      return isSubagent(block) ? (
        <SubagentCard block={block} live={live} />
      ) : block.name.startsWith(BACH) ? (
        <BachToolCard block={block} live={live} />
      ) : (
        <ToolCard block={block} live={live} />
      );
  }
}

/**
 * Images a tool returned (under its card) or the user sent (above their message); click one to
 * see it full size.
 */
function Images({ images, alt, className, thumbClass = "max-h-72" }: { images: string[]; alt: string; className?: string; thumbClass?: string }) {
  return (
    <div className={cn("flex flex-wrap gap-2", className)}>
      {images.map((src, i) => (
        <Thumbnail key={i} src={src} alt={alt} thumbClass={thumbClass} />
      ))}
    </div>
  );
}

/** One image, click to see it full size. Only inline elements, so it can sit in a paragraph. */
function Thumbnail({ src, alt, thumbClass, onError, id }: { src: string; alt: string; thumbClass: string; onError?: () => void; id?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button id={id} onClick={() => setOpen(true)} title="View full size" className="not-prose inline-block overflow-hidden rounded-lg border bg-muted align-top transition-shadow">
        <img src={src} alt={alt} onError={onError} className={cn("block max-w-full object-contain", thumbClass)} />
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[90vh] w-auto max-w-[90vw] overflow-auto p-2 sm:max-w-[90vw]">
          <DialogTitle className="sr-only">{alt}</DialogTitle>
          <img src={src} alt={alt} className="block max-h-[85vh] max-w-full object-contain select-text" />
        </DialogContent>
      </Dialog>
    </>
  );
}

/** Files already read, by path, so re-renders and repeated links don't fetch them again. */
const fileImages = new Map<string, Promise<string>>();

/** `/abs/path.png`, `~/path.png` or `file:///abs/path.png` -> the path; anything else -> null. */
function localPath(src: string): string | null {
  if (src.startsWith("file://")) return decodeURIComponent(src.slice("file://".length));
  return src.startsWith("/") || src.startsWith("~/") ? decodeURIComponent(src) : null;
}

/**
 * An image in the agent's markdown. Agents often link to files on the machine they run on, which
 * the app (maybe on another computer) can't load by path, so those are read through the backend.
 * An image that still can't be shown is replaced by its alt text and where it points.
 */
function MarkdownImage({ src, alt }: { src?: string; alt?: string }) {
  const path = src ? localPath(src) : null;
  const [url, setUrl] = useState(path ? undefined : src);
  const [failed, setFailed] = useState(!src);
  useEffect(() => {
    if (!path) return;
    let cancelled = false;
    let load = fileImages.get(path);
    if (!load) {
      load = readImage(path);
      load.catch(() => fileImages.delete(path)); // let a later render try again
      fileImages.set(path, load);
    }
    load.then(
      (u) => !cancelled && setUrl(u),
      () => !cancelled && setFailed(true),
    );
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (failed)
    return (
      <span className="inline-flex max-w-full items-center gap-1.5 rounded-md border bg-muted px-2 py-0.5 align-middle text-xs text-muted-foreground not-prose">
        <ImageOff className="size-3.5 shrink-0" />
        {alt && <span className="text-foreground">{alt}</span>}
        <code className="truncate">{path ?? src}</code>
      </span>
    );
  if (!url) return <Spinner className="inline-block size-4 align-middle" />;
  return <ReplyImage src={url} alt={alt ?? ""} onError={() => setFailed(true)} />;
}

/** An image in the agent's reply, with a link back to the tool call that showed it first, if one did. */
function ReplyImage({ src, alt, onError }: { src: string; alt: string; onError: () => void }) {
  const anchor = `img-${useId()}`;
  const place = useContext(BlockPlace);
  const links = useImageLinks();
  useEffect(() => (place ? links?.add("reply", src, { place, anchor }) : undefined), [links, src, place?.turn, place?.index, anchor]);
  const viewed = place && links?.viewedIn(src, place);
  const thumb = <Thumbnail id={anchor} src={src} alt={alt} thumbClass="max-h-72" onError={onError} />;
  if (!viewed) return thumb;
  return (
    <span className="not-prose inline-flex max-w-full flex-col items-start gap-1 align-top">
      {thumb}
      <button onClick={() => reveal(viewed.anchor)} className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ArrowUp className="size-3" />
        Viewed with <span className="font-medium">{viewed.tool}</span>
      </button>
    </span>
  );
}

/**
 * The images a tool returned. One the agent then also showed in its reply shrinks to a small
 * thumbnail that points there, since the reply is where it was meant to be seen.
 */
function ToolImages({ images, tool, id }: { images: string[]; tool: string; id: string }) {
  return (
    <div className="mt-2 flex flex-wrap gap-2">
      {images.map((src, i) => (
        <ToolImage key={i} src={src} tool={tool} anchor={`tool-img-${id}-${i}`} />
      ))}
    </div>
  );
}

function ToolImage({ src, tool, anchor }: { src: string; tool: string; anchor: string }) {
  const place = useContext(BlockPlace);
  const links = useImageLinks();
  useEffect(() => (place ? links?.add("tool", src, { place, anchor, tool }) : undefined), [links, src, place?.turn, place?.index, anchor, tool]);
  const shown = place && links?.shownIn(src, place);
  if (!shown) return <Thumbnail id={anchor} src={src} alt={`Image from ${tool}`} thumbClass="max-h-72" />;
  return (
    <div className="flex items-center gap-2">
      <Thumbnail id={anchor} src={src} alt={`Image from ${tool}`} thumbClass="max-h-14" />
      <button onClick={() => reveal(shown.anchor)} className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ArrowDown className="size-3" />
        Shown in reply
      </button>
    </div>
  );
}

function ToolCard({ block, live }: { block: ToolBlock; live: boolean }) {
  const pending = block.output === undefined;
  const { outputs } = useContext(TranscriptContext);
  // The last few lines while it runs, so a long command shows it's getting somewhere.
  const partial = pending && live ? outputs?.[block.id]?.trimEnd().split("\n").slice(-6).join("\n") : undefined;
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
    {partial && <pre className={preClass + " mt-2 max-h-32"} aria-label="Output so far">{partial}</pre>}
    {!!block.images?.length && <ToolImages images={block.images} tool={toolLabel(block.name)} id={block.id} />}
    </div>
  );
}

/** Tools of Bach's own MCP server: background tasks, and starting sessions. */
const BACH = "mcp__bach__";

/** What a call to Bach's MCP server did, in words, and the task it was about (if any). */
function describeBachTool(name: string, input: unknown, taskName?: string): { verb: string; target?: string; detail?: string } {
  const a = (input ?? {}) as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
  const ports = [...(Array.isArray(a.ports) ? a.ports : []), ...(a.port ? [a.port] : [])].map((p) => `:${p}`).join(" ");
  const task = taskName ?? str(a.id);
  switch (name.slice(BACH.length)) {
    case "task_start":
      return { verb: "Start", target: str(a.name) ?? taskName ?? str(a.command), detail: str(a.command) };
    case "compose_start":
      return { verb: "Compose", target: str(a.name) ?? taskName ?? str(a.file), detail: str(a.file) };
    case "task_process": {
      const action = str(a.action) ?? "control";
      return { verb: action[0].toUpperCase() + action.slice(1), target: str(a.process), detail: task && `in ${task}` };
    }
    case "task_stop":
      return { verb: "Stop", target: task };
    case "task_logs":
      return { verb: "Logs", target: task, detail: str(a.process) };
    case "task_list":
      return { verb: "List tasks" };
    case "port_info":
      return { verb: "Ports", target: ports || "all listeners" };
    case "http_check":
      return { verb: "Check", target: str(a.url) };
    case "start_session":
      return { verb: "New session", target: str(a.prompt)?.split("\n")[0], detail: [str(a.agent), str(a.model)].filter(Boolean).join(" · ") || undefined };
    default:
      return { verb: toolLabel(name), detail: JSON.stringify(input) };
  }
}

/**
 * A call to Bach's own MCP server, said in words. Task calls are tied to the live task they started
 * or touched, so its state, ports and log are one click away; a started session opens from here.
 */
function BachToolCard({ block, live }: { block: ToolBlock; live: boolean }) {
  const { tasks, forwarding, showTask, showSession } = useContext(TranscriptContext);
  const pending = block.output === undefined;
  const input = (block.input ?? {}) as { id?: unknown };
  // The start tools only learn the id from their result: `Task <id> "<name>": ...`.
  const said = /^Task (\S+) "(.*?)":/.exec(block.output ?? "");
  const id = typeof input.id === "string" ? input.id : said?.[1];
  const task = id ? tasks?.find((t) => t.id === id) : undefined;
  // A removed task is only known by the name the result gave it.
  const { verb, target, detail } = describeBachTool(block.name, block.input, task?.name ?? (said?.[1] === id ? said?.[2] : undefined));
  const starts = /^(task|compose)_start$/.test(block.name.slice(BACH.length));
  // `start_session` says `Started session <id> ...`.
  const session = !block.isError && block.name === BACH + "start_session" ? /^Started session (\S+)/.exec(block.output ?? "")?.[1] : undefined;

  return (
    <div className="rounded-lg border bg-card">
      <Collapsible>
        <CollapsibleTrigger className="group/trigger flex w-full items-center gap-2 rounded-lg px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent">
          <ChevronRight className={chevron} />
          {block.name === BACH + "start_session" ? (
            <MessageSquarePlus className="size-3.5 shrink-0 text-foreground" aria-label="Session" />
          ) : (
            <ServerCog className="size-3.5 shrink-0 text-foreground" aria-label="Background task" />
          )}
          <span className="shrink-0 font-semibold whitespace-nowrap text-foreground">{verb}</span>
          {target && <span className="min-w-0 truncate font-medium text-foreground">{target}</span>}
          <span className="min-w-0 flex-1 truncate font-mono" title={detail}>
            {detail !== target ? detail : undefined}
          </span>
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
        <CollapsibleContent className="flex flex-col gap-2 px-3 pb-2">
          <pre className={preClass}>{JSON.stringify(block.input, null, 2)}</pre>
          {!pending && <pre className={preClass + (block.isError ? " border-destructive/40" : "")}>{block.output}</pre>}
        </CollapsibleContent>
      </Collapsible>

      {starts && task && (
        <div className="flex flex-wrap items-center gap-1.5 border-t px-3 py-1.5 text-xs text-muted-foreground">
          <span className="[&_svg]:size-3.5">
            <StatusIcon task={task} />
          </span>
          <span>{STATUS[task.status]}</span>
          {task.ports.map((p) => (
            <PortLink key={p} port={p} forwarding={forwarding ?? null} />
          ))}
          {task.missingPorts.length > 0 && (
            <span className="text-destructive">not listening: {task.missingPorts.map((p) => `:${p}`).join(" ")}</span>
          )}
          {showTask && (
            <Button variant="ghost" size="xs" className="ml-auto" onClick={() => showTask(task.id)}>
              <ScrollText data-icon="inline-start" />
              Logs
            </Button>
          )}
        </div>
      )}

      {session && showSession && (
        <div className="flex items-center border-t px-3 py-1.5">
          <Button variant="ghost" size="xs" className="ml-auto" onClick={() => showSession(session)}>
            <MessageSquarePlus data-icon="inline-start" />
            Open session
          </Button>
        </div>
      )}
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
        {!!images?.length && <Attachments attachments={images} />}
        {text && <div className="w-fit rounded-2xl bg-secondary px-4 py-2 text-sm whitespace-pre-wrap">{text}</div>}
      </div>
    </div>
  );
}

/** What the user sent with a message: images as thumbnails, PDFs and text files by name. */
function Attachments({ attachments }: { attachments: string[] }) {
  const shown = attachments.map((src) => ({ src, ...describe(src) }));
  const images = shown.flatMap((a) => (a.image ? [a.src] : []));
  const files = shown.flatMap((a) => (a.image ? [] : [a]));
  return (
    <>
      {!!images.length && <Images images={images} alt="Image you sent" className="justify-end" thumbClass="max-h-40" />}
      {!!files.length && (
        <div className="flex flex-wrap justify-end gap-2">
          {files.map((f, i) => (
            <FileChip key={i} name={f.name} pdf={f.pdf} className="h-9" />
          ))}
        </div>
      )}
    </>
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
  // opencode only remembers "always" while its server runs; "for this session" says as much.
  if (agent === "opencode") return undefined;
  if (agent === "codex") {
    if (toolName === "Edit") return undefined;
    return toolName.startsWith("mcp__") ? "Codex stops asking for this tool (its own config)" : "Adds the command to Codex's own rules (~/.codex/rules)";
  }
  return "Saves to this project's settings";
}

const DECIDED: Record<string, string> = {
  allow: "Allowed once",
  allow_session: "Allowed for this session",
  allow_always: "Always allowed",
  deny: "Denied",
  expired: "No longer needed",
};

/** What the agent asks to do with one of Bach's own tools, as a sentence ("wants to …"). */
function bachAsk(name: string, input: unknown): string {
  const a = (input ?? {}) as Record<string, unknown>;
  switch (name.slice(BACH.length)) {
    case "start_session":
      return "start a new session";
    case "task_start":
      return "start a background task";
    case "compose_start":
      return "run a process-compose project";
    case "task_stop":
      return "stop a background task";
    case "task_process":
      return `${typeof a.action === "string" ? a.action : "control"} a process`;
    case "task_list":
      return "list background tasks";
    case "task_logs":
      return "read a task's output";
    case "port_info":
      return "see what is listening on ports";
    case "http_check":
      return "check a local URL";
    default:
      return `use ${toolLabel(name)}`;
  }
}

/** A small label with an icon, for the settings an approval asks for. */
function Chip({ icon: Icon, children }: { icon: typeof Bot; children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-md border bg-background px-2 py-0.5 text-xs text-muted-foreground">
      <Icon className="size-3.5 shrink-0" />
      {children}
    </span>
  );
}

/** The body of an approval for one of Bach's own tools: what it would do, not its JSON. */
function BachApprovalBody({ name, input }: { name: string; input: unknown }) {
  const { agent = "claude", inRepo, tasks } = useContext(TranscriptContext);
  const a = (input ?? {}) as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
  const ports = [...(Array.isArray(a.ports) ? a.ports : []), ...(a.port ? [a.port] : [])].map((p) => `:${p}`);
  const task = str(a.id) && tasks?.find((t) => t.id === a.id);

  switch (name.slice(BACH.length)) {
    case "start_session": {
      const other = str(a.agent) && a.agent !== agent ? (a.agent as AgentKind) : undefined;
      const worktree = typeof a.worktree === "boolean" ? a.worktree : inRepo;
      return (
        <>
          <div className="max-h-72 overflow-auto rounded-lg border bg-background px-3 py-2">
            <Prose text={str(a.prompt) ?? ""} />
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Chip icon={Bot}>{other ? AGENT_NAMES[other] : `${AGENT_NAMES[agent]}, like this session`}</Chip>
            <Chip icon={Brain}>{str(a.model) ?? (other ? "Default model" : "Same model")}</Chip>
            <Chip icon={worktree ? GitBranch : Folder}>{worktree ? "New worktree and branch" : "In the project folder"}</Chip>
          </div>
          <p className="text-xs text-muted-foreground">It works on its own and shows up in the sidebar.</p>
        </>
      );
    }
    case "task_start":
    case "compose_start": {
      const what = str(a.command) ?? str(a.file);
      return (
        <>
          {what && <pre className={preClass + " bg-background"}>{what}</pre>}
          <div className="flex flex-wrap items-center gap-1.5">
            {str(a.name) && <Chip icon={ServerCog}>{str(a.name)}</Chip>}
            {str(a.cwd) && <Chip icon={Folder}>{str(a.cwd)}</Chip>}
            {ports.length > 0 && <Chip icon={Network}>{ports.join(" ")}</Chip>}
            {a.interactive === false && <Chip icon={EyeOff}>Only for the agent</Chip>}
          </div>
          <p className="text-xs text-muted-foreground">It keeps running after this turn; you can stop it from Background tasks.</p>
        </>
      );
    }
    case "task_stop":
    case "task_process":
    case "task_logs":
      return (
        <div className="flex flex-wrap items-center gap-1.5">
          <Chip icon={ServerCog}>{task ? task.name : str(a.id) ?? "unknown task"}</Chip>
          {str(a.process) && <Chip icon={Cog}>{str(a.process)}</Chip>}
        </div>
      );
    default: {
      const { main, rest } = describeInput(input);
      return <pre className={preClass + " bg-background"}>{main || rest}</pre>;
    }
  }
}

/** An answered approval for one of Bach's tools, in the words its tool card uses. */
function BachDecided({ name, input }: { name: string; input: unknown }) {
  const { tasks } = useContext(TranscriptContext);
  const id = (input as { id?: unknown } | null)?.id;
  const { verb, target } = describeBachTool(name, input, tasks?.find((t) => t.id === id)?.name);
  return (
    <>
      <span className="shrink-0 font-semibold whitespace-nowrap text-foreground">{verb}</span>
      {target && <span className="min-w-0 flex-1 truncate">{target}</span>}
    </>
  );
}

/** A tool the agent wants to use. It waits here until the user answers. */
function ApprovalCard({ block, live }: { block: ApprovalBlock; live: boolean }) {
  const { decide, agent = "claude" } = useContext(TranscriptContext);
  const [busy, setBusy] = useState(false);
  const { main, rest } = describeInput(block.input);
  const always = alwaysSavesTo(agent, block.toolName);
  const bach = block.toolName.startsWith(BACH);

  if (block.decision || !live) {
    const decision = block.decision ?? "expired";
    const Icon = decision === "deny" ? ShieldX : decision === "expired" ? Ban : ShieldCheck;
    return (
      <p className="flex items-center gap-2 px-1 text-xs text-muted-foreground" title={block.rules.join("\n")}>
        <Icon className="size-3.5 shrink-0" />
        <span>{DECIDED[decision]}</span>
        {bach ? (
          <BachDecided name={block.toolName} input={block.input} />
        ) : (
          <>
            <span className="font-semibold text-foreground">{toolLabel(block.toolName)}</span>
            <code className="min-w-0 flex-1 truncate font-mono">{main ?? rest?.replace(/\s+/g, " ")}</code>
          </>
        )}
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
          {bach ? (
            <>
              {AGENT_NAMES[agent]} wants to <span className="font-semibold">{bachAsk(block.toolName, block.input)}</span>
            </>
          ) : (
            <>
              {AGENT_NAMES[agent]} wants to use <span className="font-semibold">{toolLabel(block.toolName)}</span>
            </>
          )}
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
      {bach ? (
        <BachApprovalBody name={block.toolName} input={block.input} />
      ) : (
        <>
          {main && <pre className={preClass + " bg-background"}>{main}</pre>}
          {rest && <pre className={preClass + " bg-background"}>{rest}</pre>}
        </>
      )}
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
  /** `preview`: a mockup or snippet to show while the option is highlighted. */
  options: { label: string; description?: string; preview?: string }[];
}

const OTHER = "\u0000other"; // can't collide with an option label

/** A question from the agent (its AskUserQuestion tool), answered by picking options or typing. */
function QuestionCard({ block, live }: { block: ApprovalBlock; live: boolean }) {
  const { decide, agent = "claude" } = useContext(TranscriptContext);
  const questions = ((block.input as { questions?: Question[] })?.questions ?? []).filter((q) => q.question);
  // Chosen labels per question (OTHER = the free-text choice) and the free text itself.
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});
  // The option last hovered or focused per question, whose preview is shown.
  const [highlighted, setHighlighted] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  const answerFor = (q: Question): string => {
    const labels = (picked[q.question] ?? []).flatMap((l) => (l === OTHER ? [(other[q.question] ?? "").trim()] : [l]));
    return labels.filter(Boolean).join(", ");
  };

  if (block.decision || !live) {
    const decision = block.decision ?? "expired";
    // In the order they were asked (the answers come back as an unordered map).
    const asked = questions.map((q) => q.question);
    const order = (q: string) => (asked.includes(q) ? asked.indexOf(q) : asked.length);
    const answered = decision === "allow" && block.answers && Object.entries(block.answers).sort(([a], [b]) => order(a) - order(b));
    return (
      <div className="flex flex-col gap-1 px-1 text-xs text-muted-foreground">
        {answered ? (
          answered.map(([q, a]) => (
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
        // Previews follow the highlighted option, else the last one picked, else the first.
        const withPreview = q.options.filter((o) => o.preview);
        const shown = [highlighted[q.question], ...[...on].reverse()]
          .map((l) => withPreview.find((o) => o.label === l))
          .find(Boolean) ?? withPreview[0];
        const highlight = (label: string) => setHighlighted((h) => ({ ...h, [q.question]: label }));
        return (
          <fieldset key={q.question} className="@container flex flex-col gap-2">
            <legend className="mb-1 flex items-center gap-2 text-sm font-medium">
              {q.header && <Badge variant="outline">{q.header}</Badge>}
              {q.question}
            </legend>
            <div className={cn("grid gap-2", shown && "@xl:grid-cols-2")}>
              <div className="flex flex-col gap-2">
                {q.options.map((o) => (
                  <label
                    key={o.label}
                    onMouseEnter={() => highlight(o.label)}
                    onFocus={() => highlight(o.label)}
                    className={cn(
                      "flex cursor-pointer items-start gap-3 rounded-lg border bg-background px-3 py-2 text-sm hover:bg-accent has-[:checked]:border-primary",
                      shown && o === shown && "bg-accent",
                    )}
                  >
                    <input type={type} name={q.question} className="mt-1 accent-primary" checked={on.includes(o.label)} onChange={() => toggle(q, o.label)} />
                    <span className="flex flex-col">
                      <span className="font-medium">{o.label}</span>
                      {o.description && <span className="text-xs text-muted-foreground">{o.description}</span>}
                    </span>
                  </label>
                ))}
              </div>
              {shown && (
                <pre aria-label={`Preview of ${shown.label}`} className="m-0 max-h-96 overflow-auto rounded-lg border bg-background p-3 font-mono text-xs leading-snug whitespace-pre">
                  {shown.preview}
                </pre>
              )}
            </div>
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
