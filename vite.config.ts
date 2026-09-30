import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Fixed port so src-tauri/tauri.conf.json can point at it.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "chrome110", chunkSizeWarningLimit: 5000 /* Monaco is ~4 MB; fine for a local app */ },
});
