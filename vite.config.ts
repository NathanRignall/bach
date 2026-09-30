import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "./src") } },
  clearScreen: false,
  server: { port: 3420, strictPort: true, host: "0.0.0.0" },
  // The highlight worker loads each language's parser when first needed, which needs ES modules.
  worker: { format: "es" },
});
