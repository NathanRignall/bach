import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, File as FileIcon, Folder, FolderOpen } from "lucide-react";
import { cn } from "@/lib/utils";

/** Anything with a `/`-separated path relative to the tree's root. */
export interface TreeFile {
  path: string;
}

interface Dir<T> {
  /** Shown name: several folders when they only contain each other (`src/components`). */
  name: string;
  /** Full path, without a trailing `/` (`""` for the root). */
  path: string;
  dirs: Dir<T>[];
  files: { name: string; file: T }[];
}

const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });

function buildTree<T extends TreeFile>(files: T[]): Dir<T> {
  const root: Dir<T> = { name: "", path: "", dirs: [], files: [] };
  const dirs = new Map<string, Dir<T>>([["", root]]);
  for (const file of files) {
    const parts = file.path.split("/");
    let dir = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const path = parts.slice(0, i + 1).join("/");
      let sub = dirs.get(path);
      if (!sub) {
        sub = { name: parts[i], path, dirs: [], files: [] };
        dirs.set(path, sub);
        dir.dirs.push(sub);
      }
      dir = sub;
    }
    dir.files.push({ name: parts[parts.length - 1], file });
  }
  const tidy = (d: Dir<T>): Dir<T> => {
    // Fold chains of single folders into one row, as editors do.
    while (d.path && !d.files.length && d.dirs.length === 1) {
      const only = d.dirs[0];
      d = { ...only, name: `${d.name}/${only.name}` };
    }
    d.dirs = d.dirs.map(tidy).sort(byName);
    d.files.sort(byName);
    return d;
  };
  return tidy(root);
}

type Row<T> = { kind: "dir"; dir: Dir<T>; depth: number; open: boolean; parent?: string } | { kind: "file"; name: string; file: T; depth: number; parent?: string };

export interface FileTreeProps<T extends TreeFile> {
  files: T[];
  /** The selected file's path. The tree opens the folders it's in and scrolls it into view. */
  selected?: string;
  onSelect: (file: T) => void;
  /** Folders start collapsed instead of expanded. */
  collapsed?: boolean;
  /** Extras for a file's row: shown before its name (instead of the file icon) and after it. */
  decorate?: (file: T) => { before?: ReactNode; after?: ReactNode; className?: string; title?: string };
  /** Buttons shown over the right end of a file's row while it is hovered or focused. */
  actions?: (file: T) => ReactNode;
  /** For screen readers, e.g. "Changed files". */
  label: string;
  className?: string;
}

/**
 * Files as a collapsible folder tree. Folders that only hold one folder share a row. Arrow keys
 * move through the rows; left and right close and open folders.
 */
export function FileTree<T extends TreeFile>({ files, selected, onSelect, collapsed = false, decorate, actions, label, className }: FileTreeProps<T>) {
  const tree = useMemo(() => buildTree(files), [files]);
  // Folders opened or closed away from how they start, by path.
  const [toggled, setToggled] = useState(new Set<string>());
  const isOpen = (path: string) => collapsed === toggled.has(path);
  const setOpen = (path: string, open: boolean) =>
    isOpen(path) !== open &&
    setToggled((t) => {
      const next = new Set(t);
      next.has(path) ? next.delete(path) : next.add(path);
      return next;
    });

  const rows = useMemo(() => {
    const rows: Row<T>[] = [];
    const walk = (d: Dir<T>, depth: number) => {
      const parent = d.path || undefined;
      for (const dir of d.dirs) {
        const open = isOpen(dir.path);
        rows.push({ kind: "dir", dir, depth, open, parent });
        if (open) walk(dir, depth + 1);
      }
      for (const { name, file } of d.files) rows.push({ kind: "file", name, file, depth, parent });
    };
    walk(tree, 0);
    return rows;
  }, [tree, toggled, collapsed]);

  const ref = useRef<HTMLDivElement>(null);
  const rowFor = (key: string) => ref.current?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);

  // Reveal the selected file: open the folders it's in, then scroll to it. Once per selection
  // (not on every refresh of the files), as soon as it's among them.
  const revealed = useRef<string>(undefined);
  useEffect(() => {
    if (!selected || revealed.current === selected || !files.some((f) => f.path === selected)) return;
    revealed.current = selected;
    const ancestors: string[] = [];
    let d: Dir<T> | undefined = tree;
    while (d) {
      d = d.dirs.find((sub) => selected.startsWith(`${sub.path}/`));
      if (d) ancestors.push(d.path);
    }
    const closed = ancestors.filter((p) => !isOpen(p));
    if (closed.length)
      setToggled((t) => {
        const next = new Set(t);
        for (const p of closed) next.has(p) ? next.delete(p) : next.add(p);
        return next;
      });
    requestAnimationFrame(() => rowFor(`f:${selected}`)?.scrollIntoView({ block: "nearest" }));
  }, [selected, tree]);

  const keyOf = (r: Row<T>) => (r.kind === "dir" ? `d:${r.dir.path}` : `f:${r.file.path}`);
  // One row takes part in tab order: the selected file, or else the first row.
  const tabStop = selected && rows.some((r) => r.kind === "file" && r.file.path === selected) ? `f:${selected}` : rows[0] && keyOf(rows[0]);

  const onKeyDown = (e: KeyboardEvent) => {
    const i = rows.findIndex((r) => keyOf(r) === (document.activeElement as HTMLElement | null)?.dataset.key);
    const row = rows[i];
    if (!row) return;
    const focus = (r: Row<T> | undefined) => r && rowFor(keyOf(r))?.focus();
    const parentRow = () => rows.find((r) => r.kind === "dir" && r.dir.path === row.parent);
    const handled = () => e.preventDefault();
    switch (e.key) {
      case "ArrowDown":
        handled();
        return focus(rows[i + 1]);
      case "ArrowUp":
        handled();
        return focus(rows[i - 1]);
      case "Home":
        handled();
        return focus(rows[0]);
      case "End":
        handled();
        return focus(rows[rows.length - 1]);
      case "ArrowRight":
        if (row.kind !== "dir") return;
        handled();
        return row.open ? focus(rows[i + 1]) : setOpen(row.dir.path, true);
      case "ArrowLeft":
        handled();
        return row.kind === "dir" && row.open ? setOpen(row.dir.path, false) : focus(parentRow());
    }
  };

  const indent = (depth: number) => ({ paddingLeft: `${depth * 12 + 8}px` });
  return (
    <div ref={ref} role="tree" aria-label={label} onKeyDown={onKeyDown} className={cn("py-1 text-xs", className)}>
      {rows.map((r) => {
        const key = keyOf(r);
        if (r.kind === "dir") {
          const Chevron = r.open ? ChevronDown : ChevronRight;
          const Icon = r.open ? FolderOpen : Folder;
          return (
            <button
              key={key}
              type="button"
              role="treeitem"
              aria-expanded={r.open}
              data-key={key}
              tabIndex={key === tabStop ? 0 : -1}
              onClick={() => setOpen(r.dir.path, !r.open)}
              title={r.dir.path}
              style={indent(r.depth)}
              className="flex w-full items-center gap-1 py-1 pr-3 text-left text-muted-foreground outline-none hover:bg-muted focus-visible:bg-muted focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset"
            >
              <Chevron className="size-3.5 shrink-0" />
              <Icon className="size-3.5 shrink-0" />
              <span className="min-w-0 flex-1 truncate">{r.dir.name}</span>
            </button>
          );
        }
        const extra = decorate?.(r.file);
        const isSelected = r.file.path === selected;
        const extraActions = actions?.(r.file);
        const row = (
          <button
            key={extraActions ? undefined : key}
            type="button"
            role="treeitem"
            aria-selected={isSelected}
            data-key={key}
            tabIndex={key === tabStop ? 0 : -1}
            onClick={() => onSelect(r.file)}
            title={extra?.title ?? r.file.path}
            // Files line up with their folder's name, past the chevron.
            style={indent(r.depth + 1.25)}
            className={cn(
              "flex w-full items-center gap-1.5 py-1 pr-3 text-left outline-none hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset",
              isSelected && "bg-accent text-accent-foreground hover:bg-accent",
            )}
          >
            {extra?.before ?? <FileIcon className="size-3.5 shrink-0 text-muted-foreground" />}
            <span className={cn("min-w-0 flex-1 truncate", extra?.className)}>{r.name}</span>
            {extra?.after}
          </button>
        );
        if (!extraActions) return row;
        return (
          <div key={key} className="group/row relative">
            {row}
            <div className="absolute inset-y-0 right-1 hidden items-center bg-muted group-focus-within/row:flex group-hover/row:flex">{extraActions}</div>
          </div>
        );
      })}
    </div>
  );
}
