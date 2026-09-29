import { useState } from "react";
import { FolderOpen, Plus } from "lucide-react";
import { projectName } from "@/session";
import { Choice, Picker } from "./Composer";
import { FolderPicker } from "./FolderPicker";

interface Props {
  cwd: string;
  /** Folders with sessions already, most recent first. */
  recent: string[];
  onChange: (cwd: string) => void;
}

/** The menu entry that opens the folder browser instead of picking a recent project. */
const BROWSE = "\0browse";

/** The project folder as a picker next to the chat input: recent projects, or the folder browser. */
export function ProjectButton({ cwd, recent, onChange }: Props) {
  const [picking, setPicking] = useState(false);
  const chosen = cwd.trim();
  const folders = chosen && !recent.includes(chosen) ? [chosen, ...recent] : recent;
  const choices: Choice[] = folders.map((p) => ({ value: p, label: projectName(p), description: p }));
  choices.push({
    value: BROWSE,
    label: "Add folder",
    description: "Browse for another folder",
    icon: <Plus className="size-4 text-muted-foreground" />,
    separated: choices.length > 0,
  });
  return (
    <>
      <Picker
        heading="Project"
        label="Project folder"
        align="start"
        choices={choices}
        value={chosen || null}
        placeholder="Choose folder"
        icon={<FolderOpen className="size-4 text-muted-foreground" />}
        onChange={(v) => (v === BROWSE ? setPicking(true) : onChange(v))}
      />
      {picking && (
        <FolderPicker
          start={chosen}
          onClose={() => setPicking(false)}
          onPick={(path) => {
            onChange(path);
            setPicking(false);
          }}
        />
      )}
    </>
  );
}
