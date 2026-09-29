import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { macTitleBar } from "./api";
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

createRoot(document.getElementById("root")!).render(<App />);
