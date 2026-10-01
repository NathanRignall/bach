// Which images in a transcript are the same picture: one a tool returned (an agent reading a
// screenshot) and one the agent then put in its reply. The transcript links the two rather than
// hide either, so the reply stays as the agent wrote it.

/** Where a block sits: which turn (counted by the user's messages) and its index in the transcript. */
export interface Place {
  turn: number;
  index: number;
}

export type ImageKind = "tool" | "reply";

export interface ImageSpot {
  place: Place;
  /** The element's DOM id, to scroll to it. */
  anchor: string;
  /** The tool that returned it, for a tool's image. */
  tool?: string;
}

/** A `data:` URL's payload, which is the same for the same file however the MIME type was named. */
function payload(src: string): string | null {
  const comma = src.indexOf(",");
  return src.startsWith("data:") && comma > 0 ? src.slice(comma + 1) : null;
}

export class ImageLinks {
  private spots: Record<ImageKind, Map<string, Set<ImageSpot>>> = { tool: new Map(), reply: new Map() };
  private listeners = new Set<() => void>();
  private version = 0;

  /** Records an image shown at `spot`; returns what forgets it again. */
  add(kind: ImageKind, src: string, spot: ImageSpot): () => void {
    const key = payload(src);
    if (!key) return () => {};
    const map = this.spots[kind];
    const set = map.get(key) ?? new Set();
    map.set(key, set.add(spot));
    this.changed();
    return () => {
      set.delete(spot);
      if (!set.size) map.delete(key);
      this.changed();
    };
  }

  /** The first reply after `place`, in the same turn, that shows `src` too. */
  shownIn(src: string, place: Place): ImageSpot | undefined {
    return this.find("reply", src, (s) => s.place.turn === place.turn && s.place.index > place.index, (a, b) => a.index - b.index);
  }

  /** The last tool before `place`, in the same turn, that returned `src`. */
  viewedIn(src: string, place: Place): ImageSpot | undefined {
    return this.find("tool", src, (s) => s.place.turn === place.turn && s.place.index < place.index, (a, b) => b.index - a.index);
  }

  private find(kind: ImageKind, src: string, ok: (s: ImageSpot) => boolean, order: (a: Place, b: Place) => number) {
    const key = payload(src);
    const set = key ? this.spots[kind].get(key) : undefined;
    return [...(set ?? [])].filter(ok).sort((a, b) => order(a.place, b.place))[0];
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  /** Changes whenever an image is added or forgotten, for `useSyncExternalStore`. */
  snapshot = () => this.version;

  private changed() {
    this.version++;
    this.listeners.forEach((l) => l());
  }
}

/** Scrolls to the element with DOM id `anchor` and outlines it for a moment, so the eye finds it. */
export function reveal(anchor: string) {
  const el = document.getElementById(anchor);
  if (!el) return;
  el.scrollIntoView({ behavior: "smooth", block: "center" });
  const flash = ["ring-2", "ring-primary", "ring-offset-2", "ring-offset-background"];
  el.classList.add(...flash);
  setTimeout(() => el.classList.remove(...flash), 1600);
}
