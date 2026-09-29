import { useState } from "react";
import { FolderOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { FolderPicker } from "./FolderPicker";

interface Props {
  value: string;
  locked: boolean;
  onCommit: (v: string) => void;
}

/** Edits apply on blur/Enter so the session doesn't hop between project groups while typing. */
export function CwdInput({ value, locked, onCommit }: Props) {
  const [draft, setDraft] = useState(value);
  const [picking, setPicking] = useState(false);
  return (
    <>
      <Input
        aria-label="Project folder"
        className="flex-1 font-mono text-xs"
        placeholder="Project folder — required"
        value={draft}
        disabled={locked}
        title={locked ? "The project can't change once the agent session has started" : undefined}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => onCommit(draft.trim())}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        spellCheck={false}
      />
      <Button variant="outline" disabled={locked} onClick={() => setPicking(true)}>
        <FolderOpen data-icon="inline-start" />
        Browse…
      </Button>
      {picking && (
        <FolderPicker
          start={draft}
          onClose={() => setPicking(false)}
          onPick={(path) => {
            setDraft(path);
            onCommit(path);
            setPicking(false);
          }}
        />
      )}
    </>
  );
}
