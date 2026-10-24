// Makes the app icon files in src-tauri/icons from the Sandhill crane:
//
//   npm run icon
//
// source.svg is the app icon (an 824 px rounded tile on Apple's 1024 px
// grid, with the crane) and source-small.svg the version drawn for 16 and
// 32 px. Both are written by docs/design/ui/brand/build.py; change them
// there. This adds the soft shadow macOS icons have to the large sizes, and
// `tauri icon` renders them; the small sizes then replace the ones in
// icon.icns.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const icons = join(app, "src-tauri", "icons");

const source = (name) => {
  const svg = readFileSync(join(icons, name), "utf8");
  if (!svg.includes('viewBox="0 0 1024 1024"')) throw new Error(`${name} is not a 1024 px icon`);
  return svg;
};
// The tile drawn again, as a shadow under itself.
const withShadow = (svg) =>
  svg.replace(
    /<rect x="100" y="100"/,
    '<filter id="shadow" x="-10%" y="-10%" width="120%" height="125%"><feDropShadow dx="0" dy="10" stdDeviation="10" flood-color="#000" flood-opacity=".3"/></filter>' +
      '<rect x="100" y="100" width="824" height="824" rx="185" filter="url(#shadow)"/><rect x="100" y="100"',
  );
const full = withShadow(source("source.svg"));
const small = source("source-small.svg");

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
