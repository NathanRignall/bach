import { PointerEvent, KeyboardEvent } from "react";
import { cn } from "@/lib/utils";

interface Props {
  width: number;
  onWidth: (w: number) => void;
  min: number;
  max: number;
  /** The panel's edge this handle sits on; dragging towards the panel's inside shrinks it. */
  edge: "left" | "right";
  /** Double-clicking restores this. */
  reset: number;
  label: string;
}

/** A drag strip on a panel's edge (the panel must be `relative`). Also works from the keyboard. */
export function ResizeHandle({ width, onWidth, min, max, edge, reset, label }: Props) {
  const clamp = (w: number) => Math.min(max, Math.max(min, Math.round(w)));

  function onPointerDown(e: PointerEvent<HTMLDivElement>) {
    e.preventDefault();
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startWidth = width;
    const sign = edge === "right" ? 1 : -1;
    const move = (m: globalThis.PointerEvent) => onWidth(clamp(startWidth + sign * (m.clientX - startX)));
    const stop = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", stop);
      el.removeEventListener("pointercancel", stop);
      document.body.style.removeProperty("cursor");
    };
    // The cursor stays a resize cursor even when the pointer outruns the strip.
    document.body.style.cursor = "col-resize";
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", stop);
    el.addEventListener("pointercancel", stop);
  }

  function onKeyDown(e: KeyboardEvent) {
    const step = e.shiftKey ? 48 : 16;
    const grow = (edge === "right" ? "ArrowRight" : "ArrowLeft") === e.key;
    const shrink = (edge === "right" ? "ArrowLeft" : "ArrowRight") === e.key;
    if (!grow && !shrink) return;
    e.preventDefault();
    onWidth(clamp(width + (grow ? step : -step)));
  }

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={width}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      title="Drag to resize, double-click to reset"
      onPointerDown={onPointerDown}
      onDoubleClick={() => onWidth(clamp(reset))}
      onKeyDown={onKeyDown}
      className={cn(
        "group/resize absolute inset-y-0 z-30 w-1.5 cursor-col-resize touch-none outline-none",
        edge === "right" ? "-right-[3px]" : "-left-[3px]",
      )}
    >
      <div className="mx-auto h-full w-px bg-transparent transition-colors group-hover/resize:bg-primary/50 group-focus-visible/resize:bg-primary/50 group-active/resize:bg-primary" />
    </div>
  );
}
