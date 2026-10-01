import { useEffect, useState } from "react";
import { ExternalLink, RotateCw, X } from "lucide-react";
import { inTauri, openUrl } from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Preview } from "@/lib/preview";
import { ResizeHandle } from "./ResizeHandle";

export const BROWSER_WIDTH = { default: 640, min: 360, max: 1400 };

/** A web page beside the chat, normally a dev server's port. Sites that refuse framing need the external button. */
export function BrowserPanel({ preview, width, onWidth, onClose }: { preview: Preview; width: number; onWidth: (w: number) => void; onClose: () => void }) {
  const [url, setUrl] = useState(preview.url);
  const [address, setAddress] = useState(preview.url);
  // Bumped to reload: the page is cross-origin, so its own location can't be read or reloaded.
  const [reloads, setReloads] = useState(0);
  useEffect(() => {
    setUrl(preview.url);
    setAddress(preview.url);
  }, [preview]);

  const go = () => {
    const typed = address.trim();
    if (!typed) return;
    const next = /^[a-z][a-z0-9+.-]*:\/\//i.test(typed) ? typed : `http://${typed}`;
    setAddress(next);
    if (next === url) setReloads((n) => n + 1);
    else setUrl(next);
  };
  const external = () => (inTauri ? void openUrl(url).catch(() => {}) : void window.open(url, "_blank", "noopener,noreferrer"));

  return (
    <aside style={{ width }} className="relative flex shrink-0 flex-col border-l bg-background" aria-label="Browser">
      <ResizeHandle width={width} onWidth={onWidth} min={BROWSER_WIDTH.min} max={BROWSER_WIDTH.max} edge="left" reset={BROWSER_WIDTH.default} label="Resize browser" />
      <header data-tauri-drag-region className="flex h-(--title-bar-height) shrink-0 items-center gap-1 border-b px-2">
        <Button variant="ghost" size="icon-sm" aria-label="Reload" title="Reload" onClick={() => setReloads((n) => n + 1)}>
          <RotateCw />
        </Button>
        <form className="min-w-0 flex-1" onSubmit={(e) => (e.preventDefault(), go())}>
          <Input
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onFocus={(e) => e.currentTarget.select()}
            spellCheck={false}
            aria-label="Address"
            className="h-7 font-mono text-xs"
          />
        </form>
        <Button variant="ghost" size="icon-sm" aria-label="Open in the default browser" title="Open in the default browser" onClick={external}>
          <ExternalLink />
        </Button>
        <Button variant="ghost" size="icon-sm" aria-label="Close browser" onClick={onClose}>
          <X />
        </Button>
      </header>
      <iframe key={`${url}#${reloads}`} src={url} title="Browser" className="min-h-0 flex-1 bg-white" allow="clipboard-read; clipboard-write; fullscreen" />
    </aside>
  );
}
