// Builds Sandhill.app with sandhilld inside it, next to the app's own
// binary, where the app looks for it first.
//
//   npm run bundle                 -> target/release/bundle/macos/Sandhill.app
//   npm run bundle -- --no-bundle  (just the release binaries)
//
// The daemon is only bundled here, not in tauri.conf.json, so that plain
// `cargo build` and `npm run tauri dev` don't need a release daemon first.
// "-" signs ad hoc: enough for macOS to run it, not notarized.

import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(app, "..");
const run = (cmd, args, cwd) => execFileSync(cmd, args, { cwd, stdio: "inherit" });

const host = /^host: (\S+)$/m.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))?.[1];
if (!host) throw new Error("cannot tell the Rust target from `rustc -vV`");

run("cargo", ["build", "--release", "-p", "sandhill-daemon"], root);
const binaries = join(app, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
const sidecar = join(binaries, `sandhilld-${host}`);
copyFileSync(join(root, "target", "release", "sandhilld"), sidecar);
chmodSync(sidecar, 0o755);

run("npx", ["tauri", "build", "--config", "src-tauri/tauri.bundle.json", ...process.argv.slice(2)], app);
