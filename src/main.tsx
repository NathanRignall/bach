import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { inTauri, macTitleBar, openUrl } from "./api";
import { App } from "./App";
import { initTheme } from "./lib/theme";
import "./index.css";

// shadcn themes via a `.dark` class and an accent attribute; see Settings > Appearance.
initTheme();

// The Mac app's top bar matches the native title bar macOS lays out.
if (macTitleBar) {
  invoke<{ height: number; controlsEnd: number } | null>("title_bar")
    .then((bar) => {
      if (!bar) return;
      document.documentElement.style.setProperty("--title-bar-height", `${bar.height}px`);
      document.documentElement.style.setProperty("--traffic-lights-end", `${bar.controlsEnd}px`);
    })
    .catch(console.error);
}

// A file dropped anywhere but the composer would replace the page with it.
for (const type of ["dragover", "drop"]) {
  window.addEventListener(type, (e) => {
    const drag = e as DragEvent;
    if (drag.defaultPrevented || !drag.dataTransfer?.types.includes("Files")) return;
    drag.preventDefault();
    drag.dataTransfer.dropEffect = "none";
  });
}

// The app's webview can't open links in new windows, so they'd do nothing: send links that leave
// the app to the default browser instead.
if (inTauri) {
  document.addEventListener("click", (e) => {
    const a = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
    if (!a || e.defaultPrevented || !/^(https?|mailto):/.test(a.href)) return;
    if (a.origin === location.origin && a.target !== "_blank") return;
    e.preventDefault();
    openUrl(a.href).catch(console.error);
  });
}

createRoot(document.getElementById("root")!).render(<App />);
