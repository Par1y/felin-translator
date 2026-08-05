import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev-server port and doesn't want Vite clearing the
// screen (so Rust errors stay visible). See v2.tauri.app/start/frontend/vite.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // Only expose VITE_* and TAURI_ENV_* to the frontend.
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // Chromium on Windows, WebKit elsewhere.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    // Vite 8 (rolldown) minifies with oxc by default; don't force "esbuild"
    // (that path requires esbuild to be installed separately).
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
