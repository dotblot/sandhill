// A plot's bytes, for copying and saving: read from the daemon, and drawn
// onto a canvas when it is wanted as a PNG at another size.

import { type Blob as BlobText, isBlobRef } from "../backend/types";
import type { Workspace } from "../model/workspace";
import type { PlotFormat, PlotItem } from "../model/plots";

/** An image's data as the daemon keeps it: base64 for PNG, JPEG and GIF, text for SVG. */
export async function imageData(ws: Workspace, item: PlotItem, mime = item.mime): Promise<string> {
  const value = item.images[mime];
  if (isBlobRef(value)) {
    return (await ws.call<BlobText>("get_blob", { hash: value.sandhill_blob, near: item.cell?.file })).data;
  }
  return typeof value === "string" ? value : "";
}

/** The width and height a plot was drawn at, from the browser's copy of it. */
export function naturalSize(img: HTMLImageElement): { width: number; height: number } | undefined {
  return img.naturalWidth > 0 && img.naturalHeight > 0 ? { width: img.naturalWidth, height: img.naturalHeight } : undefined;
}

/**
 * The file to write for a plot in `format`: the kernel's own bytes, or for
 * an SVG plot saved as PNG, the SVG drawn at `width` × `height`.
 */
export async function plotFile(
  ws: Workspace,
  item: PlotItem,
  format: PlotFormat,
  size: { width: number; height: number },
): Promise<{ text: string; base64: boolean }> {
  if (format === "svg" || item.mime !== "image/svg+xml") return { text: await imageData(ws, item), base64: format !== "svg" };
  const png = await drawSvg(await imageData(ws, item), size);
  return { text: toBase64(withDpi(new Uint8Array(await png.arrayBuffer()), DPI)), base64: true };
}

/** The resolution sizes are given at, so a 1200 × 900 PNG is 4 × 3 in. */
export const DPI = 300;

/**
 * Marks a PNG as `dpi` dots per inch (a pHYs chunk after the header), so
 * Word or PowerPoint places it at its size in inches. One that has a pHYs
 * already is left as it is.
 */
export function withDpi(png: Uint8Array, dpi: number): Uint8Array {
  const view = new DataView(png.buffer, png.byteOffset, png.byteLength);
  // Chunks start after the 8-byte signature: length, type, data, CRC.
  for (let at = 8; at + 8 <= png.length; ) {
    const type = String.fromCharCode(...png.subarray(at + 4, at + 8));
    if (type === "pHYs") return png;
    if (type === "IDAT" || type === "IEND") break;
    at += 12 + view.getUint32(at);
  }
  const perMetre = Math.round(dpi / 0.0254);
  const chunk = new Uint8Array(21);
  const out = new DataView(chunk.buffer);
  out.setUint32(0, 9);
  chunk.set([0x70, 0x48, 0x59, 0x73], 4);
  out.setUint32(8, perMetre);
  out.setUint32(12, perMetre);
  chunk[16] = 1; // the unit is the metre
  out.setUint32(17, crc32(chunk.subarray(4, 17)));
  // The header (IHDR) is always first: 8 + 4 + 4 + 13 + 4 bytes in.
  const head = 33;
  const result = new Uint8Array(png.length + chunk.length);
  result.set(png.subarray(0, head));
  result.set(chunk, head);
  result.set(png.subarray(head), head + chunk.length);
  return result;
}

let crcTable: Uint32Array | undefined;

function crc32(bytes: Uint8Array): number {
  if (!crcTable) {
    crcTable = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c >>> 0;
    }
  }
  let crc = 0xffffffff;
  for (const b of bytes) crc = crcTable[(crc ^ b) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

/** The plot as a PNG for the clipboard. */
export async function plotPng(ws: Workspace, item: PlotItem, size: { width: number; height: number }): Promise<Blob> {
  const data = await imageData(ws, item);
  if (item.mime === "image/svg+xml") return drawSvg(data, size);
  const bytes = fromBase64(data);
  if (item.mime === "image/png") return new Blob([bytes], { type: "image/png" });
  // JPEG and GIF go onto the clipboard as PNG, which every app takes.
  return draw(new Blob([bytes], { type: item.mime }), size);
}

/** Puts a PNG on the clipboard; the promise form keeps Safari's click permission. */
export async function copyPng(png: Promise<Blob>): Promise<void> {
  if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) {
    throw new Error("this window can't put images on the clipboard");
  }
  await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
}

/** Draws an SVG on white at `width` × `height`, keeping its shape and centring it. */
function drawSvg(svg: string, size: { width: number; height: number }): Promise<Blob> {
  return draw(new Blob([svg], { type: "image/svg+xml" }), size);
}

async function draw(source: Blob, size: { width: number; height: number }): Promise<Blob> {
  // A URL of our own, so the canvas isn't tainted and can be read back.
  const url = URL.createObjectURL(source);
  try {
    const img = new Image();
    img.decoding = "async";
    img.src = url;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = size.width;
    canvas.height = size.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("could not draw the plot");
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, size.width, size.height);
    const natural = naturalSize(img) ?? size;
    const box = fit(natural, size, true);
    ctx.drawImage(img, (size.width - box.width) / 2, (size.height - box.height) / 2, box.width, box.height);
    return await new Promise<Blob>((resolve, reject) =>
      canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("could not make a PNG"))), "image/png"),
    );
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** The largest size of `natural`'s shape inside `box`; smaller than natural only unless `grow`. */
export function fit(
  natural: { width: number; height: number },
  box: { width: number; height: number },
  grow: boolean,
): { width: number; height: number } {
  if (natural.width <= 0 || natural.height <= 0 || box.width <= 0 || box.height <= 0) return { width: 0, height: 0 };
  let k = Math.min(box.width / natural.width, box.height / natural.height);
  if (!grow) k = Math.min(k, 1);
  return { width: Math.round(natural.width * k), height: Math.round(natural.height * k) };
}

export function toBase64(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}

function fromBase64(data: string): Uint8Array<ArrayBuffer> {
  const text = atob(data.replace(/\s/g, ""));
  const bytes = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i);
  return bytes;
}
