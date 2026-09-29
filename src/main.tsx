import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { macTitleBar } from "./api";
import { App } from "./App";
import "./index.css";

// shadcn themes via a `.dark` class; follow the system setting.
const media = window.matchMedia("(prefers-color-scheme: dark)");
const applyTheme = () => document.documentElement.classList.toggle("dark", media.matches);
applyTheme();
media.addEventListener("change", applyTheme);

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
