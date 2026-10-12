import { execFileSync } from "node:child_process";
import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

/**
 * When and from which commit this build was made, shown in the windows so a
 * build can be told from the one before. CI passes the branch's commit,
 * since a pull request's checkout is a merge commit that won't exist later.
 */
function build(): { when: string; commit: string } {
  const now = new Date();
  const day = now.toLocaleString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
  let commit = process.env.SANDHILL_COMMIT ?? "";
  if (!commit) {
    try {
      commit = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
    } catch {
      commit = "";
    }
  }
  return { when: `${day} ${now.toISOString().slice(11, 16)} UTC`, commit: commit.slice(0, 7) };
}

// Tauri serves the built UI in production and this dev server during
// `npm run tauri dev`.
export default defineConfig({
  plugins: [solid()],
  define: { __SANDHILL_BUILD__: JSON.stringify(build()) },
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "safari16", outDir: "dist", chunkSizeWarningLimit: 1500 },
  test: { environment: "jsdom" },
});
