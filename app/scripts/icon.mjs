// Makes the app icon files in src-tauri/icons from source.svg, Huan's
// artwork:
//
//   npm run icon
//
// The artwork is fitted into Apple's icon grid (824 of 1024 px, centered),
// so it sits at the same size as other apps in the Dock. At 16 and 32 px the
// whole scene blurs into an orange blob, so those sizes are drawn from a
// simpler crop: no reeds or ripples, and closer on the cranes and the sun.
// `tauri icon` renders the SVGs; the small sizes then replace the ones in
// icon.icns.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const icons = join(app, "src-tauri", "icons");

// In the artwork's own coordinates (its viewBox is 0 0 1254 1254).
const SMALL_CROP = "230 30 880 880";
const SMALL_DROPS = ["water-ripples", "reeds-left", "reeds-right"];

const svg = readFileSync(join(icons, "source.svg"), "utf8");
const viewBox = /viewBox="([^"]+)"/.exec(svg);
if (!viewBox) throw new Error("source.svg has no viewBox");
const [x, y, w, h] = viewBox[1].trim().split(/[\s,]+/).map(Number);
if (w !== h) throw new Error("source.svg is not square");
const grid = (w * 1024) / 824;
const pad = (grid - w) / 2;
const full = svg.replace(viewBox[0], `viewBox="${x - pad} ${y - pad} ${grid} ${grid}"`);

let small = svg.replace(viewBox[0], `viewBox="${SMALL_CROP}"`);
for (const id of SMALL_DROPS) {
  const group = new RegExp(`<g id="${id}"[^>]*>[\\s\\S]*?</g>`);
  if (!group.test(small)) throw new Error(`source.svg has no group "${id}" to leave out at small sizes`);
  small = small.replace(group, "");
}

const work = mkdtempSync(join(tmpdir(), "sandhill-icon-"));
try {
  writeFileSync(join(work, "full.svg"), full);
  writeFileSync(join(work, "small.svg"), small);
  const tauri = (...args) => execFileSync("npx", ["tauri", "icon", ...args], { cwd: app, stdio: "inherit" });
  tauri(join(work, "full.svg"), "-o", join(work, "full"));
  tauri(join(work, "small.svg"), "-o", join(work, "small"), "-p", "16", "-p", "32");

  for (const name of ["128x128.png", "128x128@2x.png", "icon.png"]) {
    copyFileSync(join(work, "full", name), join(icons, name));
  }
  const small16 = readFileSync(join(work, "small", "16x16.png"));
  const small32 = readFileSync(join(work, "small", "32x32.png"));
  writeFileSync(join(icons, "32x32.png"), small32);

  // 16 and 32 px, and 16 pt on Retina (32 px), from the small crop. The
  // older RGB-and-mask entries for those sizes go, so they can't win.
  const entries = readIcns(readFileSync(join(work, "full", "icon.icns")))
    .filter(([type]) => !["is32", "s8mk", "il32", "l8mk", "icp4", "icp5", "ic11"].includes(type));
  entries.push(["icp4", small16], ["icp5", small32], ["ic11", small32]);
  writeFileSync(join(icons, "icon.icns"), writeIcns(entries));
} finally {
  rmSync(work, { recursive: true, force: true });
}

// An .icns file is "icns", its length, then entries of a 4-letter type, a
// length (including these 8 bytes) and the data.
function readIcns(data) {
  if (data.toString("latin1", 0, 4) !== "icns") throw new Error("not an icns file");
  const entries = [];
  for (let at = 8; at < data.length; ) {
    const length = data.readUInt32BE(at + 4);
    entries.push([data.toString("latin1", at, at + 4), data.subarray(at + 8, at + length)]);
    at += length;
  }
  return entries;
}

function writeIcns(entries) {
  const parts = entries.map(([type, body]) => {
    const head = Buffer.alloc(8);
    head.write(type, 0, "latin1");
    head.writeUInt32BE(body.length + 8, 4);
    return Buffer.concat([head, body]);
  });
  const head = Buffer.alloc(8);
  head.write("icns", 0, "latin1");
  head.writeUInt32BE(8 + parts.reduce((n, p) => n + p.length, 0), 4);
  return Buffer.concat([head, ...parts]);
}
