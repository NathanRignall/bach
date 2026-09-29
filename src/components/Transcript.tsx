import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { AlertCircle, Brain, CheckCircle2, ChevronRight, Wrench, XCircle } from "lucide-react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Spinner } from "@/components/ui/spinner";
import { Block } from "@/session";

const summaryClass =
  "group/trigger flex w-full items-center gap-2 rounded-lg border bg-card px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent";
const preClass = "max-h-72 overflow-auto rounded-lg border bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all";

export function BlockView({ block }: { block: Block }) {
  switch (block.kind) {
    case "user":
      return (
        <div className="ml-auto w-fit max-w-[85%] rounded-2xl bg-secondary px-4 py-2 text-sm whitespace-pre-wrap">{block.text}</div>
      );

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
            <ChevronRight className="size-3.5 transition-transform group-data-[panel-open]/trigger:rotate-90" />
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
      return (
        <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          <AlertCircle className="mt-0.5 size-4 shrink-0" />
          <span className="whitespace-pre-wrap">{block.text}</span>
        </div>
      );

    case "tool": {
      const pending = block.output === undefined;
      return (
        <Collapsible>
          <CollapsibleTrigger className={summaryClass}>
            <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[panel-open]/trigger:rotate-90" />
            <Wrench className="size-3.5 shrink-0" />
            <span className="font-semibold text-foreground">{block.name}</span>
            <code className="min-w-0 flex-1 truncate font-mono">{JSON.stringify(block.input)}</code>
            {pending ? (
              <Spinner className="size-3.5 shrink-0 text-primary" />
            ) : block.isError ? (
              <XCircle className="size-3.5 shrink-0 text-destructive" aria-label="Failed" />
            ) : (
              <CheckCircle2 className="size-3.5 shrink-0 text-muted-foreground" aria-label="Done" />
            )}
          </CollapsibleTrigger>
          <CollapsibleContent className="mt-2 flex flex-col gap-2">
            <pre className={preClass}>{JSON.stringify(block.input, null, 2)}</pre>
            {!pending && <pre className={preClass + (block.isError ? " border-destructive/40" : "")}>{block.output}</pre>}
          </CollapsibleContent>
        </Collapsible>
      );
    }
  }
}
