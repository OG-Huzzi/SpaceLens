/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Phase 0 scaffold. Tauri dev/build wiring (`tauri dev`, updater, icons)
// is added in Phase 1 alongside MSVC-based CI; `vite build` output in
// `dist/` is what the Tauri shell will serve (see src-tauri/tauri.conf.json).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", target: "es2021" },
  test: {
    environment: "jsdom",
    pool: "vmThreads",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    restoreMocks: true,
    clearMocks: true
  }
});
