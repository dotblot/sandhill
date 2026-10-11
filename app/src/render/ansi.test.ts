import { describe, expect, it } from "vitest";
import { appendAnsi, consoleText, parseAnsi } from "./ansi";

describe("consoleText", () => {
  it("redraws lines on carriage return like a terminal", () => {
    expect(consoleText("10%\r50%\r100%\ndone\n")).toBe("100%\ndone\n");
    expect(consoleText("abcdef\rXY")).toBe("XYcdef");
    expect(consoleText("line\r\nnext")).toBe("line\nnext");
  });

  it("applies backspaces", () => {
    expect(consoleText("abc\b\bX")).toBe("aX");
  });
});

describe("parseAnsi", () => {
  it("styles text and resets", () => {
    const spans = parseAnsi("\x1b[1;31mError\x1b[0m: bad");
    expect(spans).toEqual([
      { text: "Error", bold: true, fg: "ansi-red" },
      { text: ": bad" },
    ]);
  });

  it("supports 256 and true colors", () => {
    expect(parseAnsi("\x1b[38;5;196mx")[0].fg).toBe("rgb(255,0,0)");
    expect(parseAnsi("\x1b[48;2;1;2;3mx")[0].bg).toBe("rgb(1,2,3)");
  });

  it("drops other escape sequences", () => {
    expect(parseAnsi("a\x1b[2Kb\x1b]0;title\x07c").map((s) => s.text).join("")).toBe("abc");
  });

  it("renders spans into the DOM", () => {
    const pre = document.createElement("pre");
    appendAnsi(pre, "plain \x1b[32mgreen\x1b[0m");
    expect(pre.textContent).toBe("plain green");
    expect(pre.querySelector(".ansi-green-fg")?.textContent).toBe("green");
  });
});
