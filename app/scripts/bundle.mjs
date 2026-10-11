// Builds Sandhill.app with sandhilld inside it, next to the app's own
// binary, where the app looks for it first.
//
//   npm run bundle                 -> target/release/bundle/macos/Sandhill.app
//   npm run bundle -- --no-bundle  (just the release binaries)
//   npm run bundle -- --servers    (also Linux builds for servers; required)
//
// Linux builds of sandhilld go into the app's resources (`daemons/`), from
// where the app installs them on servers over ssh. They are static (musl)
// and cross-compiled with cargo-zigbuild, which needs zig: CI passes
// --servers so a failure there is loud; locally they are built when
// cargo-zigbuild is installed and skipped with a note otherwise.
//
// The daemon is only bundled here, not in tauri.conf.json, so that plain
// `cargo build` and `npm run tauri dev` don't need a release daemon first.
// "-" signs ad hoc: enough for macOS to run it, not notarized.

import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, rmSync } from "node:fs";
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

const SERVER_TARGETS = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"];
const args = process.argv.slice(2);
const requireServers = args.includes("--servers");
const tauriArgs = args.filter((a) => a !== "--servers");
const servers = join(binaries, "servers");
rmSync(servers, { recursive: true, force: true });
const configs = ["--config", "src-tauri/tauri.bundle.json"];
if (requireServers || hasZigbuild()) {
  mkdirSync(servers, { recursive: true });
  for (const target of SERVER_TARGETS) {
    run("cargo", ["zigbuild", "--release", "-p", "sandhill-daemon", "--target", target], root);
    const build = join(servers, `sandhilld-${target}`);
    copyFileSync(join(root, "target", target, "release", "sandhilld"), build);
    chmodSync(build, 0o755);
  }
  configs.push("--config", JSON.stringify({ bundle: { resources: { "binaries/servers/*": "daemons/" } } }));
} else {
  console.warn("cargo-zigbuild not found: building without Linux daemons, so the app can't install sandhilld on servers");
}

run("npx", ["tauri", "build", ...configs, ...tauriArgs], app);

function hasZigbuild() {
  try {
    execFileSync("cargo-zigbuild", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}
