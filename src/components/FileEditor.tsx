import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { AlertTriangle, Save, X } from "lucide-react";
import { ApiError, FileContent, readFile, writeFile } from "@/api";
import type { CodeEditorHandle } from "@/components/CodeEditor";
import { GitError } from "@/components/GitBar";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";

// The editor is a sizeable library; it loads when the first file is edited.
const CodeEditor = lazy(() => import("@/components/CodeEditor").then((m) => ({ default: m.CodeEditor })));

/** Edits not saved yet, by session and file: closing the editor by switching files doesn't lose them. */
const drafts = new Map<string, { text: string; version: string }>();
const draftKey = (sessionId: string, path: string) => `${sessionId}\0${path}`;

/** The file has edits that weren't saved. */
export const hasDraft = (sessionId: string, path: string) => drafts.has(draftKey(sessionId, path));

const lineCount = (text: string) => text.split("\n").length - (text.endsWith("\n") ? 1 : 0);

/**
 * Edits the working-tree file `path` of the session, in place, and saves it on the backend host.
 * If the file changed on disk since it was opened (the agent edited it, say), saving shows the
 * conflict and lets the user reload or overwrite. `line` (1-based) opens the editor there.
 * Leaving unsaved edits (switching files) keeps them for next time.
 */
export function FileEditor({
  sessionId,
  path,
  line,
  maxHeight,
  onClose,
  onSaved,
}: {
  sessionId: string;
  path: string;
  line?: number;
  /** A CSS length the editor scrolls beyond; otherwise it fills its parent. */
  maxHeight?: string;
  onClose: () => void;
  onSaved: () => void;
}) {
  const key = draftKey(sessionId, path);
  // What the edit started from: the file as read, or the draft left earlier (and the version it was based on).
  const [base, setBase] = useState<{ text: string; version: string } | undefined>(() => drafts.get(key));
  const [loadError, setLoadError] = useState<string>();
  const [notEditable, setNotEditable] = useState<string>();
  const dirty = useRef(hasDraft(sessionId, path));
  const [changed, setChanged] = useState(dirty.current);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string>();
  /** The file as it is on disk now, once saving found it differs from what was opened. */
  const [conflict, setConflict] = useState<FileContent>();
  const editor = useRef<CodeEditorHandle>(null);

  useEffect(() => {
    if (base) return;
    let live = true;
    readFile(sessionId, path).then(
      (f) => {
        if (!live) return;
        if (f.text === null) setNotEditable(f.binary ? "This is a binary file." : "This file is too large to edit.");
        else if (!f.editable) setNotEditable("This file isn't valid UTF-8, so editing it here could corrupt it.");
        else setBase({ text: f.text, version: f.version });
      },
      (e) => live && setLoadError(String((e as Error)?.message ?? e)),
    );
    return () => {
      live = false;
    };
  }, [sessionId, path]);

  const finish = (after: () => void) => {
    dirty.current = false;
    drafts.delete(key);
    after();
  };

  const save = async (overwrite = false) => {
    if (!base || !editor.current) return;
    setSaving(true);
    setSaveError(undefined);
    try {
      await writeFile({ sessionId, path, text: editor.current.getText(), expectedVersion: base.version, overwrite });
      finish(onSaved);
    } catch (e) {
      const err = ApiError.from(e);
      if (err.code !== "conflict") return setSaveError(err.message);
      try {
        const disk = await readFile(sessionId, path);
        if (disk.text === null || !disk.editable) setSaveError(`${path} changed on disk and can no longer be edited here.`);
        else setConflict(disk);
      } catch (e2) {
        setSaveError(String((e2 as Error)?.message ?? e2));
      }
    } finally {
      setSaving(false);
    }
  };

  /** Drops the edits for what the agent (or anything else) left on disk. */
  const reload = () => {
    if (!conflict?.text) return;
    dirty.current = false;
    drafts.delete(key);
    setBase({ text: conflict.text, version: conflict.version });
    setChanged(false);
    setConflict(undefined);
  };

  if (notEditable || loadError) {
    return (
      <div className="flex items-center gap-3 p-4 text-xs">
        <span className={loadError ? "text-destructive" : "text-muted-foreground"}>{loadError ?? notEditable}</span>
        <Button size="xs" variant="outline" className="ml-auto" onClick={onClose}>
          Close
        </Button>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" aria-label={`Editing ${path}`}>
      <div className="flex shrink-0 items-center gap-2 border-b bg-muted/40 px-3 py-1.5 text-xs">
        <span className="text-muted-foreground">
          {changed ? "Editing · unsaved changes" : "Editing"}
          <span className="hidden sm:inline"> · the agent is told about it with your next message</span>
        </span>
        <div className="ml-auto flex items-center gap-2">
          <Button size="xs" variant="ghost" disabled={saving} onClick={() => finish(onClose)} title={changed ? "Close without saving" : "Close the editor"}>
            <X /> {changed ? "Discard" : "Close"}
          </Button>
          <Button size="xs" disabled={!base || !changed || saving} onClick={() => void save()} title="Write the file to disk (Ctrl/⌘-S)">
            {saving ? <Spinner /> : <Save />} Save
          </Button>
        </div>
      </div>
      {conflict && (
        <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-amber-500/40 bg-amber-500/12 px-3 py-2 text-xs">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-amber-600 dark:text-amber-400" />
          <div className="min-w-0 flex-1">
            <p className="font-medium">This file changed on disk since you opened it.</p>
            <p className="text-muted-foreground">
              Probably the agent: it has {lineCount(conflict.text ?? "")} {lineCount(conflict.text ?? "") === 1 ? "line" : "lines"} now, your version{" "}
              {lineCount(editor.current?.getText() ?? "")}. Reload to edit what is there (your edits are dropped), or overwrite it with yours.
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <Button size="xs" variant="outline" disabled={saving} onClick={() => setConflict(undefined)}>
              Keep editing
            </Button>
            <Button size="xs" variant="outline" disabled={saving} onClick={reload}>
              Reload from disk
            </Button>
            <Button size="xs" variant="destructive" disabled={saving} onClick={() => void save(true)}>
              {saving && <Spinner />} Overwrite
            </Button>
          </div>
        </div>
      )}
      {saveError && <GitError className="m-2" message={saveError} onDismiss={() => setSaveError(undefined)} />}
      <div className="flex min-h-0 flex-1 flex-col font-mono" style={{ maxHeight }}>
        {base ? (
          <Suspense fallback={<Loading />}>
            <CodeEditor
              // Reloading from disk is another document.
              key={base.version}
              ref={editor}
              path={path}
              text={base.text}
              line={line}
              onChange={() => {
                dirty.current = true;
                setChanged(true);
              }}
              onSave={() => void save()}
              onUnmount={(text) => {
                if (dirty.current) drafts.set(key, { text, version: base.version });
              }}
            />
          </Suspense>
        ) : (
          <Loading />
        )}
      </div>
    </div>
  );
}

function Loading() {
  return (
    <div className="flex items-center gap-2 p-4 text-xs text-muted-foreground">
      <Spinner /> Loading…
    </div>
  );
}
