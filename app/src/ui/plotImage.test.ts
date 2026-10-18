import { crc32 } from "node:zlib";
import { describe, expect, it } from "vitest";
import { fit, toBase64, withDpi } from "./plotImage";

// A 1 × 1 PNG.
const PNG = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="),
  (c) => c.charCodeAt(0),
);

function chunks(png: Uint8Array): { type: string; data: Uint8Array; crc: number }[] {
  const view = new DataView(png.buffer, png.byteOffset, png.byteLength);
  const found = [];
  for (let at = 8; at < png.length; ) {
    const length = view.getUint32(at);
    found.push({
      type: String.fromCharCode(...png.subarray(at + 4, at + 8)),
      data: png.subarray(at + 8, at + 8 + length),
      crc: view.getUint32(at + 8 + length),
    });
    at += 12 + length;
  }
  return found;
}

describe("plot images", () => {
  it("marks a PNG as 300 dpi, once", () => {
    const marked = withDpi(PNG, 300);
    const list = chunks(marked);
    expect(list.map((c) => c.type)).toEqual(["IHDR", "pHYs", "IDAT", "IEND"]);
    const phys = list[1];
    const view = new DataView(phys.data.buffer, phys.data.byteOffset, phys.data.byteLength);
    // 300 dpi is 11811 dots per metre, the same both ways.
    expect([view.getUint32(0), view.getUint32(4), phys.data[8]]).toEqual([11811, 11811, 1]);
    expect(phys.crc).toBe(crc32(marked.subarray(8 + 25 + 4, 8 + 25 + 4 + 13)));
    expect(withDpi(marked, 72)).toBe(marked);
  });

  it("fits a plot in the room it has, growing only when asked", () => {
    expect(fit({ width: 400, height: 300 }, { width: 200, height: 200 }, false)).toEqual({ width: 200, height: 150 });
    expect(fit({ width: 400, height: 300 }, { width: 1000, height: 600 }, false)).toEqual({ width: 400, height: 300 });
    expect(fit({ width: 400, height: 300 }, { width: 1000, height: 600 }, true)).toEqual({ width: 800, height: 600 });
    expect(fit({ width: 0, height: 0 }, { width: 10, height: 10 }, true)).toEqual({ width: 0, height: 0 });
  });

  it("writes bytes as base64", () => {
    expect(toBase64(PNG)).toBe(btoa(String.fromCharCode(...PNG)));
    expect(toBase64(new Uint8Array(100_000)).length).toBe(Math.ceil(100_000 / 3) * 4);
  });
});
