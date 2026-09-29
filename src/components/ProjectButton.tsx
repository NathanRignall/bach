import { useState } from "react";
import { FolderOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { projectName } from "@/session";
import { FolderPicker } from "./FolderPicker";

interface Props {
  cwd: string;
  onChange: (cwd: string) => void;
}

/** The project folder as a button next to the chat input; opens the folder browser. */
export function ProjectButton({ cwd, onChange }: Props) {
  const [picking, setPicking] = useState(false);
  const chosen = cwd.trim();
  return (
    <>
      <Button variant="outline" size="sm" title={chosen || "Choose a project folder"} aria-label="Project folder" onClick={() => setPicking(true)}>
        <FolderOpen data-icon="inline-start" />
        <span className="max-w-40 truncate">{chosen ? projectName(chosen) : "Choose folder"}</span>
      </Button>
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
