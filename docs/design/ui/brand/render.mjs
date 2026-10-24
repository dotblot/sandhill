// Render the brand SVGs to PNG. Run `python3 build.py` first.
//   node docs/design/ui/brand/render.mjs
// Uses Playwright's Chromium; set PLAYWRIGHT to its module path if it isn't installed here.
import { readFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const { chromium } = await import(process.env.PLAYWRIGHT || "playwright");
const here = dirname(fileURLToPath(import.meta.url));
const jobs = [
  ["crane/sandhill-crane-light.svg", "png/sandhill-crane-light-1024.png", 1024],
  ["crane/sandhill-crane-dark.svg", "png/sandhill-crane-dark-1024.png", 1024],
  ["crane/sandhill-crane-on-light.svg", "png/sandhill-crane-on-light-1024.png", 1024],
  ["crane/sandhill-crane-on-dark.svg", "png/sandhill-crane-on-dark-1024.png", 1024],
  ["crane/sandhill-crane-template.svg", "png/sandhill-crane-template-18@2x.png", 36],
  ...[64, 32, 24].map(s => [`small/sandhill-mark-${s}.svg`, `png/sandhill-mark-${s}@2x.png`, s * 2]),
  ["small/sandhill-mark-16.svg", "png/sandhill-mark-16@2x.png", 32],
  // app icon sizes macOS asks for; 16 and 32 use the small drawing
  ...[1024, 512, 256, 128, 64].map(s => ["app-icon/app-icon-dark.svg", `app-icon/png/app-icon-${s}.png`, s]),
  ...[32, 16].map(s => ["app-icon/app-icon-dark-small.svg", `app-icon/png/app-icon-${s}.png`, s]),
  ["app-icon/app-icon-light.svg", "app-icon/png/app-icon-light-1024.png", 1024],
];
const browser = await chromium.launch();
const page = await browser.newPage();
for (const [src, out, size] of jobs) {
  const svg = readFileSync(join(here, src), "utf8").replace(/^(<svg[^>]*?) width="\d+" height="\d+"/, "$1");
  await page.setViewportSize({ width: size, height: size });
  await page.setContent(`<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${svg}`);
  mkdirSync(dirname(join(here, out)), { recursive: true });
  await page.screenshot({ path: join(here, out), omitBackground: true });
}
await browser.close();
console.log(jobs.length, "PNGs written");
