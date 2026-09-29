import { useEffect, useRef, useState } from "react";
import { ArrowUp, Folder, Home } from "lucide-react";
import { DirListing, listDir } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";

interface Props {
  start: string;
  onPick: (path: string) => void;
  onClose: () => void;
}

/** Browses folders on the backend host, i.e. where the agent will actually run. */
export function FolderPicker({ start, onPick, onClose }: Props) {
  const [listing, setListing] = useState<DirListing>();
  const [pathInput, setPathInput] = useState(start);
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [showHidden, setShowHidden] = useState(false);
  const latest = useRef(0);

  async function go(path: string | undefined, hidden = showHidden) {
    const req = ++latest.current;
    setLoading(true);
    try {
      const l = await listDir(path, hidden);
      if (req !== latest.current) return; // a newer navigation superseded this one
      setListing(l);
      setPathInput(l.path);
      setError(undefined);
    } catch (e) {
      if (req !== latest.current) return;
      setError(String((e as Error).message ?? e));
      // First open with a stale/unknown path: fall back to the home folder.
      if (!listing && path) return void go(undefined, hidden);
    }
    if (req === latest.current) setLoading(false);
  }

  useEffect(() => {
    void go(start.trim() || undefined);
  }, []);

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="flex h-[min(34rem,80vh)] flex-col gap-3 sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Choose a project folder</DialogTitle>
        </DialogHeader>

        <div className="flex gap-1.5">
          <Button variant="outline" size="icon" title="Parent folder" disabled={!listing?.parent} onClick={() => void go(listing!.parent!)}>
            <ArrowUp />
          </Button>
          <Button variant="outline" size="icon" title="Home folder" disabled={!listing} onClick={() => void go(listing!.home)}>
            <Home />
          </Button>
          <Input
            aria-label="Folder path"
            value={pathInput}
            spellCheck={false}
            className="font-mono text-xs"
            onChange={(e) => setPathInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void go(pathInput)}
          />
        </div>

        {error && <p className="text-xs text-destructive">{error}</p>}

        <div className="relative min-h-0 flex-1 overflow-y-auto rounded-lg border p-1">
          {listing?.entries.map((d) => (
            <button
              key={d.path}
              onClick={() => void go(d.path)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent"
            >
              <Folder className="size-4 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">{d.name}</span>
              {d.git && <Badge variant="outline">git</Badge>}
            </button>
          ))}
          {listing && !loading && listing.entries.length === 0 && (
            <p className="p-8 text-center text-sm text-muted-foreground">No subfolders</p>
          )}
          {loading && (
            <div className="absolute inset-0 flex items-center justify-center bg-popover/60">
              <Spinner className="size-5 text-muted-foreground" />
            </div>
          )}
        </div>

        <DialogFooter className="items-center sm:justify-between">
          <div className="flex items-center gap-2">
            <Checkbox
              id="show-hidden"
              checked={showHidden}
              onCheckedChange={(checked) => {
                setShowHidden(checked);
                void go(listing?.path, checked);
              }}
            />
            <Label htmlFor="show-hidden" className="text-xs text-muted-foreground">
              Show hidden
            </Label>
          </div>
          <div className="flex gap-2">
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button disabled={!listing} onClick={() => onPick(listing!.path)}>
              Choose this folder
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
