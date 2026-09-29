import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./index.css";

// shadcn themes via a `.dark` class; follow the system setting.
const media = window.matchMedia("(prefers-color-scheme: dark)");
const applyTheme = () => document.documentElement.classList.toggle("dark", media.matches);
applyTheme();
media.addEventListener("change", applyTheme);

createRoot(document.getElementById("root")!).render(<App />);
