import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { newNotebook, notebookLanguage, parseNotebook, serializeNotebook, splitLines } from "./ipynb";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(join(here, "../../../tests/fixtures/jupyter-written.ipynb"), "utf8");

describe("notebooks", () => {
  it("writes back byte-for-byte what Jupyter wrote", () => {
    expect(serializeNotebook(parseNotebook(fixture))).toBe(fixture);
  });

  it("joins multi-line strings for editing", () => {
    const nb = parseNotebook(fixture);
    expect(nb.cells[1].source).toBe("print('hello')\nprint('world')");
    expect(nb.cells[1].outputs[0].text).toBe("hello\nworld\n");
    expect(nb.cells[1].outputs[2].data["text/html"]).toBe("<b>x</b>\n<i>y</i>");
    expect(nb.cells[1].metadata).toEqual({ tags: ["keep"] });
    expect(notebookLanguage(nb)).toBe("python");
  });

  it("keeps fields it doesn't know about", () => {
    const raw = JSON.parse(fixture);
    raw.custom = { a: 1 };
    raw.cells[0].attachments = { "img.png": { "image/png": "AAAA" } };
    const back = JSON.parse(serializeNotebook(parseNotebook(JSON.stringify(raw))));
    expect(back.custom).toEqual({ a: 1 });
    expect(back.cells[0].attachments).toEqual({ "img.png": { "image/png": "AAAA" } });
  });

  it("gives old notebooks cell ids and upgrades them to 4.5", () => {
    const raw = JSON.parse(fixture);
    raw.nbformat_minor = 4;
    for (const c of raw.cells) delete c.id;
    raw.cells[2].id = raw.cells[1].id = undefined;
    const nb = parseNotebook(JSON.stringify(raw));
    const ids = nb.cells.map((c) => c.id);
    expect(new Set(ids).size).toBe(3);
    expect(JSON.parse(serializeNotebook(nb)).nbformat_minor).toBe(5);
  });

  it("rejects files that aren't notebooks", () => {
    expect(() => parseNotebook("{}")).toThrow();
    expect(() => parseNotebook('{"cells": [], "nbformat": 3}')).toThrow(/nbformat 3/);
  });

  it("creates new notebooks for a kernel", () => {
    const nb = newNotebook({ name: "ir", display_name: "R", language: "R" });
    expect(notebookLanguage(nb)).toBe("r");
    expect(parseNotebook(serializeNotebook(nb)).cells).toHaveLength(1);
  });

  it("splits lines like Python", () => {
    expect(splitLines("a\nb\r\nc\rd")).toEqual(["a\n", "b\r\n", "c\r", "d"]);
    expect(splitLines("")).toEqual([]);
    expect(splitLines("x\n")).toEqual(["x\n"]);
  });
});
