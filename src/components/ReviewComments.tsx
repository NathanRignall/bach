import { type ReactNode, useEffect, useRef, useState } from "react";
import { HelpCircle, MessageSquarePlus, Pencil, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { type Draft, rangeLabel } from "@/lib/review";

/** An inline card in the diff, kept in view when a wide line scrolls the diff sideways. */
export function InlineCard({ children }: { children: ReactNode }) {
  return <div className="sticky left-0 my-1 ml-2 w-[min(46rem,calc(100cqw-1.5rem))] rounded-lg border bg-card p-2 font-sans text-xs shadow-xs">{children}</div>;
}

/** What can be done with the lines selected in the diff. */
export function SelectionActions({
  label,
  onComment,
  onExplain,
  onCancel,
  busy,
}: {
  /** What is selected, e.g. "lines 4–9". */
  label: string;
  onComment: () => void;
  /** Left out where asking the agent isn't possible. */
  onExplain?: () => void;
  onCancel: () => void;
  busy?: boolean;
}) {
  return (
    <InlineCard>
      <div className="flex items-center gap-1.5">
        <span className="mr-auto px-1 text-muted-foreground">{label}</span>
        <Button size="xs" variant="outline" onClick={onComment} disabled={busy}>
          <MessageSquarePlus /> Comment
        </Button>
        {onExplain && (
          <Button size="xs" variant="outline" onClick={onExplain} disabled={busy} title="Ask the agent why it made this change">
            <HelpCircle /> Explain this
          </Button>
        )}
        <Button size="icon-xs" variant="ghost" onClick={onCancel} title="Clear selection (Esc)" aria-label="Clear selection">
          <X />
        </Button>
      </div>
    </InlineCard>
  );
}

/** A text box for a comment; Cmd/Ctrl+Enter saves, Esc cancels. */
export function CommentBox({ initial = "", label, onSave, onCancel }: { initial?: string; label: string; onSave: (text: string) => void; onCancel: () => void }) {
  const [text, setText] = useState(initial);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  const ok = !!text.trim();
  return (
    <InlineCard>
      <Textarea
        ref={ref}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={`Comment on ${label}…`}
        aria-label={`Comment on ${label}`}
        className="min-h-14 text-xs md:text-xs"
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel();
          else if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && ok) onSave(text);
        }}
      />
      <div className="mt-1.5 flex justify-end gap-1.5">
        <Button size="xs" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="xs" onClick={() => onSave(text)} disabled={!ok}>
          Add comment
        </Button>
      </div>
    </InlineCard>
  );
}

/** A drafted comment, with edit and delete. `showPath` when it isn't under its file's own lines. */
export function DraftComment({ draft, showPath, onEdit, onRemove }: { draft: Draft; showPath?: boolean; onEdit: (text: string) => void; onRemove: () => void }) {
  const [editing, setEditing] = useState(false);
  const label = (showPath ? `${draft.anchor.path}, ` : "") + rangeLabel(draft.anchor);
  if (editing)
    return (
      <CommentBox
        initial={draft.text}
        label={label}
        onSave={(t) => {
          onEdit(t);
          setEditing(false);
        }}
        onCancel={() => setEditing(false)}
      />
    );
  return (
    <InlineCard>
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="mb-0.5 flex items-center gap-1.5 text-muted-foreground">
            <span className="rounded bg-primary/15 px-1.5 py-px text-[10px] font-medium text-primary">Draft</span>
            <span className="truncate font-mono text-[11px]">{label}</span>
          </div>
          <p className="text-sm whitespace-pre-wrap">{draft.text}</p>
        </div>
        <Button size="icon-xs" variant="ghost" onClick={() => setEditing(true)} title="Edit" aria-label="Edit comment">
          <Pencil />
        </Button>
        <Button size="icon-xs" variant="ghost" onClick={onRemove} title="Delete" aria-label="Delete comment">
          <Trash2 />
        </Button>
      </div>
    </InlineCard>
  );
}
