import { useEffect, useState } from "react";
import { ContextUsage, PlanUsage, getUsage, onReconnect, onUsage } from "@/api";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";

/** The account's usage limits: fetched once, then kept current by the backend's usage events. */
export function usePlanUsage() {
  const [usage, setUsage] = useState<PlanUsage | null>(null);
  useEffect(() => {
    const load = () => void getUsage().then(setUsage, () => {});
    load();
    const unUsage = onUsage(setUsage);
    const unReconnect = onReconnect(load);
    return () => (unUsage(), unReconnect());
  }, []);
  return usage;
}

/** 35612 -> "35.6k", 1000000 -> "1M". */
function tokens(n: number) {
  const short = (x: number, unit: string) => `${Number(x.toFixed(1))}${unit}`;
  if (n >= 1_000_000) return short(n / 1_000_000, "M");
  if (n >= 1_000) return short(n / 1_000, "k");
  return String(n);
}

const pct = (share: number) => Math.round(Math.min(Math.max(share, 0), 1) * 100);

const WINDOW_NAMES: Record<string, string> = {
  five_hour: "5-hour limit",
  seven_day: "Weekly · all models",
  seven_day_opus: "Weekly · Opus",
  seven_day_sonnet: "Weekly · Sonnet",
};

/** `five_hour` -> "5-hour limit"; names we don't know yet are shown as they come. */
const windowName = (key: string) => WINDOW_NAMES[key] ?? key.replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase());

/** Soon: "Resets in 34 min"; later: "Resets Sun 12:00 AM". */
function resets(at: number | null, now: number) {
  if (!at) return "";
  const mins = Math.max(0, Math.round((at - now) / 60_000));
  if (mins < 60) return `Resets in ${mins} min`;
  if (mins < 5 * 60) return `Resets in ${Math.floor(mins / 60)} hr ${mins % 60} min`;
  return `Resets ${new Date(at).toLocaleString(undefined, { weekday: "short", hour: "numeric", minute: "2-digit" })}`;
}

function ago(at: number, now: number) {
  const mins = Math.round((now - at) / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.round(mins / 60);
  return hours < 48 ? `${hours} hr ago` : `${Math.round(hours / 24)} days ago`;
}

function Bar({ share, className }: { share: number; className?: string }) {
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
      <div className={cn("h-full rounded-full", share >= 0.9 ? "bg-destructive" : "bg-primary", className)} style={{ width: `${pct(share)}%` }} />
    </div>
  );
}

/** A small ring showing how full something is. */
function Ring({ share }: { share: number }) {
  const r = 7;
  const c = 2 * Math.PI * r;
  return (
    <svg viewBox="0 0 18 18" className="size-4 -rotate-90" aria-hidden>
      <circle cx="9" cy="9" r={r} fill="none" strokeWidth="2.5" className="stroke-muted" />
      <circle
        cx="9"
        cy="9"
        r={r}
        fill="none"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeDasharray={`${(pct(share) / 100) * c} ${c}`}
        className={share >= 0.9 ? "stroke-destructive" : "stroke-primary"}
      />
    </svg>
  );
}

/**
 * How full the session's context is and how much of the account's usage limits are used, like
 * Claude's own apps show it. Limits are as of the latest agent run; there is no live source.
 */
export function UsageIndicator({ context, usage }: { context?: ContextUsage | null; usage: PlanUsage | null }) {
  const [now, setNow] = useState(Date.now);
  const contextShare = context?.window ? context.used / context.window : null;
  const windows = Object.entries(usage?.windows ?? {});
  if (!context && !windows.length) return null;
  // The ring shows the context when there is one, else the fullest limit.
  const ringShare = contextShare ?? Math.max(0, ...windows.map(([, w]) => w.utilization));
  const limited = usage && usage.status !== "allowed";

  return (
    <Popover onOpenChange={(open) => open && setNow(Date.now())}>
      <PopoverTrigger
        className="flex size-7 items-center justify-center rounded-md hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
        aria-label="Usage"
        title="Context and usage limits"
      >
        <Ring share={ringShare} />
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-80 gap-4 p-4">
        {context && (
          <section className="flex flex-col gap-1.5">
            <div className="flex items-baseline justify-between gap-2">
              <span className="text-muted-foreground">Context window</span>
              <span className="tabular-nums">
                {tokens(context.used)}
                {context.window ? ` / ${tokens(context.window)} (${pct(contextShare ?? 0)}%)` : " tokens"}
              </span>
            </div>
            {contextShare !== null && <Bar share={contextShare} />}
          </section>
        )}

        {usage && windows.length > 0 && (
          <section className="flex flex-col gap-3">
            <div className="flex items-baseline justify-between gap-2">
              <span className="text-muted-foreground">Plan usage limits</span>
              {limited && <span className="text-xs font-medium text-destructive">Limit reached</span>}
            </div>
            {windows.map(([key, w]) => (
              <div key={key} className="flex flex-col gap-1.5">
                <div className="flex items-baseline justify-between gap-2">
                  <span>{windowName(key)}</span>
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {resets(w.resetsAt, now)} <span className="ml-1 text-foreground">{pct(w.utilization)}%</span>
                  </span>
                </div>
                <Bar share={w.utilization} />
              </div>
            ))}
            <p className="text-xs text-muted-foreground">As of {ago(usage.observedAt, now)} · updated while agents run</p>
          </section>
        )}
      </PopoverContent>
    </Popover>
  );
}
