// Makes the app icon files in src-tauri/icons from source.png, Huan's
// painting:
//
//   npm run icon
//
// The painting is square and fills its canvas, so it is drawn as a Mac icon:
// clipped to a rounded square in Apple's icon grid (824 of 1024 px,
// centered), with a soft shadow, so it sits at the same size as other apps
// in the Dock. At 16 and 32 px the whole scene blurs, so those sizes show a
// closer crop on the cranes' heads and necks, without the shadow.
// `tauri icon` renders the SVGs; the small sizes then replace the ones in
// icon.icns.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const icons = join(app, "src-tauri", "icons");

// In the painting's own pixels.
const SMALL_CROP = "250 80 750 750";

const png = readFileSync(join(icons, "source.png"));
if (png.toString("latin1", 1, 4) !== "PNG") throw new Error("source.png is not a PNG");
const [width, height] = [png.readUInt32BE(16), png.readUInt32BE(20)];
if (width !== height) throw new Error("source.png is not square");
const href = `data:image/png;base64,${png.toString("base64")}`;

// Apple's grid: the rounded square is 824 px of 1024, corner radius 185.
const square = `<rect x="100" y="100" width="824" height="824" rx="185"/>`;
const svg = (body) =>
  `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1024" height="1024" viewBox="0 0 1024 1024">` +
  `<defs><clipPath id="square">${square}</clipPath>` +
  `<filter id="shadow" x="-10%" y="-10%" width="120%" height="125%"><feDropShadow dx="0" dy="10" stdDeviation="10" flood-color="#000" flood-opacity=".3"/></filter></defs>` +
  body +
  `</svg>`;
const painting = (viewBox) =>
  `<svg x="100" y="100" width="824" height="824" viewBox="${viewBox}" preserveAspectRatio="xMidYMid slice">` +
  `<image width="${width}" height="${height}" xlink:href="${href}"/></svg>`;

const full = svg(`<g filter="url(#shadow)"><g clip-path="url(#square)">${painting(`0 0 ${width} ${height}`)}</g></g>`);
const small = svg(`<g clip-path="url(#square)">${painting(SMALL_CROP)}</g>`);

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
