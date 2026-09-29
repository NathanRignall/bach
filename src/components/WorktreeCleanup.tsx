import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import { WorktreeEntry, listWorktrees, removeWorktree } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { Session } from "@/api";

interface Props {
  sessions: Session[];
  onClose: () => void;
}

/** Lists worktrees Bach created on the backend host and removes them, guarding unsaved work. */
export function WorktreeCleanup({ sessions, onClose }: Props) {
  const [entries, setEntries] = useState<WorktreeEntry[]>();
  const [error, setError] = useState<string>();
  const [deleteBranch, setDeleteBranch] = useState(true);
  const [confirming, setConfirming] = useState<string>();
  const [removing, setRemoving] = useState<string>();

  const refresh = () =>
    listWorktrees()
      .then((e) => (setEntries(e), setError(undefined)))
      .catch((e) => setError(String(e.message ?? e)));

  useEffect(() => void refresh(), []);

  async function remove(w: WorktreeEntry) {
    const unsafe = w.dirty || w.unmerged > 0;
    if (unsafe && confirming !== w.path) return setConfirming(w.path);
    setConfirming(undefined);
    setRemoving(w.path);
    try {
      // The backend marks sessions that ran there as no longer continuable.
      await removeWorktree({ path: w.path, discard: unsafe, deleteBranch });
      setError(undefined);
    } catch (e) {
      setError(String((e as Error).message ?? e));
    }
    setRemoving(undefined);
    await refresh();
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="flex max-h-[80vh] flex-col gap-3 sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Clean up worktrees</DialogTitle>
          <DialogDescription>
            Worktrees created for sessions on this backend. Removing one deletes its folder; your own checkout is never touched.
          </DialogDescription>
        </DialogHeader>

        {error && <p className="text-xs text-destructive">{error}</p>}

        <div className="min-h-24 flex-1 overflow-y-auto rounded-lg border">
          {!entries && !error && (
            <div className="flex h-24 items-center justify-center">
              <Spinner className="size-5 text-muted-foreground" />
            </div>
          )}
          {entries?.length === 0 && <p className="p-8 text-center text-sm text-muted-foreground">No worktrees to clean up</p>}
          <ul className="divide-y">
            {entries?.map((w) => {
              const users = sessions.filter((s) => s.workdir === w.path && !s.workdirRemoved);
              const unsafe = w.dirty || w.unmerged > 0;
              const isConfirming = confirming === w.path;
              return (
                <li key={w.path} className="flex items-center gap-3 p-3">
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm">
                      <span className="text-muted-foreground">{w.repo} / </span>
                      <span className="font-mono">{w.branch ?? "(detached)"}</span>
                    </p>
                    <p className="truncate font-mono text-[11px] text-muted-foreground" title={w.path}>
                      {w.path}
                    </p>
                    <div className="mt-1 flex flex-wrap gap-1">
                      {w.dirty && <Badge variant="destructive">uncommitted changes</Badge>}
                      {w.unmerged > 0 && (
                        <Badge variant="destructive">
                          {w.unmerged} unmerged commit{w.unmerged > 1 ? "s" : ""}
                        </Badge>
                      )}
                      {!unsafe && <Badge variant="outline">clean</Badge>}
                      {users.length > 0 && (
                        <Badge variant="secondary" title={users.map((s) => s.title).join(", ")}>
                          used by {users.length} session{users.length > 1 ? "s" : ""}
                        </Badge>
                      )}
                    </div>
                  </div>
                  <Button
                    variant={isConfirming ? "destructive" : "outline"}
                    size="sm"
                    disabled={removing !== undefined}
                    onClick={() => void remove(w)}
                    onBlur={() => setConfirming(undefined)}
                  >
                    {removing === w.path ? <Spinner data-icon="inline-start" /> : <Trash2 data-icon="inline-start" />}
                    {isConfirming ? "Discard & remove?" : "Remove"}
                  </Button>
                </li>
              );
            })}
          </ul>
        </div>

        <div className="flex items-center gap-2">
          <Checkbox id="delete-branch" checked={deleteBranch} onCheckedChange={setDeleteBranch} />
          <Label htmlFor="delete-branch" className="text-xs text-muted-foreground">
            Also delete the branch
          </Label>
        </div>
      </DialogContent>
    </Dialog>
  );
}
