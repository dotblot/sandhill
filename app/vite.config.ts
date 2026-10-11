import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

// Tauri serves the built UI in production and this dev server during
// `npm run tauri dev`.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "safari16", outDir: "dist" },
  test: { environment: "jsdom" },
});
