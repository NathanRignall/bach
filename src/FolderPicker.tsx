import { useEffect, useRef, useState } from "react";
import { DirListing, listDir } from "./api";

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
  const [showHidden, setShowHidden] = useState(false);
  const latest = useRef(0);

  async function go(path: string | undefined, hidden = showHidden) {
    const req = ++latest.current;
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
      if (!listing && path) void go(undefined, hidden);
    }
  }

  useEffect(() => {
    void go(start.trim() || undefined);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, []);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-label="Choose a folder">
        <div className="picker-nav">
          <button title="Parent folder" disabled={!listing?.parent} onClick={() => void go(listing!.parent!)}>
            ↑
          </button>
          <button title="Home folder" disabled={!listing} onClick={() => void go(listing!.home)}>
            ~
          </button>
          <input
            value={pathInput}
            spellCheck={false}
            onChange={(e) => setPathInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void go(pathInput)}
          />
        </div>
        {error && <div className="picker-error">{error}</div>}
        <div className="picker-list">
          {listing?.entries.map((d) => (
            <button key={d.path} onClick={() => void go(d.path)}>
              <span className="folder-name">{d.name}</span>
              {d.git && <span className="badge">git</span>}
            </button>
          ))}
          {listing && listing.entries.length === 0 && <div className="picker-empty">No subfolders</div>}
        </div>
        <div className="picker-foot">
          <label>
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => {
                setShowHidden(e.target.checked);
                void go(listing?.path, e.target.checked);
              }}
            />
            Show hidden
          </label>
          <span className="spacer" />
          <button onClick={onClose}>Cancel</button>
          <button className="primary" disabled={!listing} onClick={() => onPick(listing!.path)}>
            Choose this folder
          </button>
        </div>
      </div>
    </div>
  );
}
