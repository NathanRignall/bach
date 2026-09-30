// Tells the user when a session needs them while they're looking elsewhere: a native notification
// in the Mac app (click opens the session), the browser's Notification API on the web page (where
// the browser allows it), a count in the dock badge or tab title, and marks the open session seen.
// What counts as "waiting" or "done" is the backend's: this only reacts to the events it pushes.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Session, inTauri, markSeen, onSessionEvent } from "@/api";
import { awaitingApproval } from "@/session";

/** The Mac app shows native notifications; other desktops (and the page) use the browser's. */
const nativeNotifications = inTauri && /Mac/.test(navigator.userAgent);

const useWindowFocus = () => {
  const [focused, setFocused] = useState(() => document.hasFocus());
  useEffect(() => {
    const update = () => setFocused(document.hasFocus());
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  return focused;
};

function show(title: string, body: string, sessionId: string, open: (id: string) => void) {
  if (nativeNotifications) return void invoke("notify", { title, body, sessionId }).catch(() => {});
  if (inTauri || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const n = new Notification(title, { body, tag: sessionId });
  n.onclick = () => {
    window.focus();
    open(sessionId);
    n.close();
  };
}

/**
 * `sessions` and `activeId` are what the UI shows; `open` switches to a session (a notification
 * was clicked).
 */
export function useAttention(sessions: Session[], activeId: string | undefined, open: (id: string) => void) {
  const focused = useWindowFocus();
  const latest = useRef({ sessions, activeId, open });
  latest.current = { sessions, activeId, open };

  // The open session, looked at in a window that's in front, has been seen.
  const active = sessions.find((s) => s.id === activeId);
  useEffect(() => {
    if (active?.unseen && focused) void markSeen(active.id).catch(() => {});
  }, [active?.id, active?.unseen, focused]);

  // The sessions waiting on the user, as a count on the dock icon (Mac app) and in the tab title.
  const waiting = sessions.filter((s) => !s.archived && awaitingApproval(s)).length;
  useEffect(() => {
    document.title = waiting ? `(${waiting}) Waiting for you — Bach` : "Bach";
    if (inTauri) void getCurrentWindow().setBadgeCount(waiting || undefined).catch(() => {});
  }, [waiting]);

  // Notifications, for what happens in sessions the user isn't looking at.
  useEffect(() => {
    // The browser only asks for permission in answer to a click.
    const ask = () => {
      if (!inTauri && typeof Notification !== "undefined" && Notification.permission === "default") void Notification.requestPermission();
    };
    window.addEventListener("pointerdown", ask, { once: true });

    const titleOf = (id: string) => latest.current.sessions.find((s) => s.id === id)?.title ?? "Session";
    const watching = (id: string) => document.hasFocus() && latest.current.activeId === id;
    const last = new Map<string, Session>();
    const un = onSessionEvent((e) => {
      if (e.type === "deleted") return void last.delete(e.sessionId);
      if (e.type === "entry") {
        const a = e.entry.entry;
        if (a.type !== "agent" || a.event.type !== "approval" || watching(e.sessionId)) return;
        const question = a.event.toolName === "AskUserQuestion";
        const detail = a.event.description ?? (question ? "" : a.event.toolName);
        show(question ? "Question from the agent" : "Approval needed", [titleOf(e.sessionId), detail].filter(Boolean).join(" — "), e.sessionId, latest.current.open);
      } else if (e.type === "changed") {
        const s = e.session;
        const before = last.get(s.id) ?? latest.current.sessions.find((x) => x.id === s.id);
        last.set(s.id, s);
        // A run just ended (stopping it by hand leaves nothing to report).
        if (s.unseen && s.unseen !== before?.unseen && !watching(s.id)) {
          show(s.unseen === "failed" ? "Session failed" : "Session finished", s.title, s.id, latest.current.open);
        }
      }
    });
    const unClick = inTauri ? listen<string>("bach-open-session", (e) => latest.current.open(e.payload)) : undefined;
    return () => {
      un();
      void unClick?.then((f) => f());
      window.removeEventListener("pointerdown", ask);
    };
  }, []);
}
