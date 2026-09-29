import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 3420, strictPort: true, host: "0.0.0.0" },
});
