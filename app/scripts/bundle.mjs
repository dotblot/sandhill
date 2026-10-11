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
// Ark, Posit's R kernel (MIT), goes into `daemons/` too: the app installs it
// on servers that have R, so R runs with no setup there. Its Linux builds
// are downloaded from its GitHub releases with the `gh` CLI (CI sets
// GH_TOKEN); SANDHILL_ARK_VERSION picks a release, otherwise the newest.
//
// The daemon is only bundled here, not in tauri.conf.json, so that plain
// `cargo build` and `npm run tauri dev` don't need a release daemon first.
// "-" signs ad hoc: enough for macOS to run it, not notarized.

import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
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
  try {
    bundleArk(servers);
  } catch (err) {
    if (requireServers) throw err;
    console.warn(`cannot download Ark (${err.message}): building without it, so R on servers needs a kernel installed there`);
  }
  configs.push("--config", JSON.stringify({ bundle: { resources: { "binaries/servers/*": "daemons/" } } }));
} else {
  console.warn("cargo-zigbuild not found: building without Linux daemons, so the app can't install sandhilld on servers");
}

run("npx", ["tauri", "build", ...configs, ...tauriArgs], app);

// Ark's Linux builds link to glibc, so they are named for gnu triples.
function bundleArk(dir) {
  const repo = "posit-dev/ark";
  const releases = JSON.parse(execFileSync("gh", ["api", `repos/${repo}/releases?per_page=30`], { encoding: "utf8" }));
  const wanted = process.env.SANDHILL_ARK_VERSION;
  const release = wanted
    ? releases.find((r) => r.tag_name === wanted || r.tag_name === `v${wanted}`)
    : (releases.find((r) => !r.draft && !r.prerelease) ?? releases.find((r) => !r.draft));
  if (!release) throw new Error(wanted ? `no Ark release ${wanted}` : "no Ark release found");
  console.log(`Bundling Ark ${release.tag_name}`);
  const work = mkdtempSync(join(tmpdir(), "sandhill-ark-"));
  try {
    for (const [arch, triple] of [["x64", "x86_64-unknown-linux-gnu"], ["arm64", "aarch64-unknown-linux-gnu"]]) {
      // ark-<version>-linux-x64.zip, not ark-<version>-debug-linux-x64.zip.
      const asset = release.assets.find((a) => a.name.endsWith(`-linux-${arch}.zip`) && !a.name.includes("-debug-"));
      if (!asset) {
        if (arch === "x64") throw new Error(`Ark ${release.tag_name} has no linux-x64 build`);
        console.warn(`Ark ${release.tag_name} has no linux-${arch} build: skipped`);
        continue;
      }
      const into = join(work, arch);
      run("gh", ["release", "download", release.tag_name, "--repo", repo, "--pattern", asset.name, "--dir", into]);
      run("unzip", ["-q", "-o", join(into, asset.name), "-d", into]);
      const ark = readdirSync(into, { recursive: true }).find((f) => f === "ark" || f.endsWith("/ark"));
      if (!ark) throw new Error(`${asset.name} has no ark program`);
      const build = join(dir, `ark-${triple}`);
      copyFileSync(join(into, ark), build);
      chmodSync(build, 0o755);
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

function hasZigbuild() {
  try {
    execFileSync("cargo-zigbuild", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}
