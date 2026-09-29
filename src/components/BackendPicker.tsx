import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { canSwitchBackend, remoteUrl, setBackend } from "@/api";

const MODES = [
  { value: "local", label: "On this machine" },
  { value: "remote", label: "On a remote bach-server" },
];

export function BackendPicker() {
  const [url, setUrl] = useState(remoteUrl ?? "ws://localhost:3421");
  const [mode, setMode] = useState(remoteUrl ? "remote" : "local");
  if (!canSwitchBackend) return <p className="text-xs text-muted-foreground">Agents on {remoteUrl}</p>;
  const changed = mode === "local" ? remoteUrl !== null : url !== remoteUrl;
  return (
    <div className="flex flex-col gap-1.5">
      <Label className="text-xs text-muted-foreground">Agents run</Label>
      <Select items={MODES} value={mode} onValueChange={(v) => v && setMode(v)}>
        <SelectTrigger className="w-full" size="sm">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {MODES.map((m) => (
            <SelectItem key={m.value} value={m.value}>
              {m.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {mode === "remote" && (
        <Input value={url} onChange={(e) => setUrl(e.target.value)} spellCheck={false} className="h-7 font-mono text-xs" />
      )}
      {changed && (
        <Button size="sm" onClick={() => setBackend(mode === "local" ? null : url.trim())}>
          Apply
        </Button>
      )}
    </div>
  );
}
