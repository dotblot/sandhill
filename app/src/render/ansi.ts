// Terminal text for outputs: ANSI colors (tracebacks, colored logs) and the
// carriage returns progress bars use to redraw a line.

export interface Span {
  text: string;
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

const BASIC = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

/** Applies `\r` (return to line start, overwrite) and `\b` (erase one character). */
export function consoleText(text: string): string {
  if (!text.includes("\r") && !text.includes("\b")) return text;
  return text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => {
      let current = "";
      for (const part of line.split("\r")) {
        let segment = "";
        for (const ch of part) segment = ch === "\b" ? segment.slice(0, -1) : segment + ch;
        current = segment + current.slice(segment.length);
      }
      return current;
    })
    .join("\n");
}

/** Splits text into styled spans, dropping escape sequences that aren't colors. */
export function parseAnsi(text: string): Span[] {
  const spans: Span[] = [];
  let style: Omit<Span, "text"> = {};
  // CSI ... final byte, or OSC ... BEL/ST.
  const re = /\x1b\[([0-9;?]*)([@-~])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (m.index > last) spans.push({ text: text.slice(last, m.index), ...style });
    last = re.lastIndex;
    if (m[2] === "m") style = applySgr(style, m[1]);
  }
  if (last < text.length) spans.push({ text: text.slice(last), ...style });
  return spans.filter((s) => s.text.length > 0);
}

function applySgr(prev: Omit<Span, "text">, params: string): Omit<Span, "text"> {
  const codes = params === "" ? [0] : params.split(";").map((p) => parseInt(p || "0", 10));
  let s = { ...prev };
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) s = {};
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 22) s.bold = s.dim = undefined;
    else if (c === 23) s.italic = undefined;
    else if (c === 24) s.underline = undefined;
    else if (c >= 30 && c <= 37) s.fg = `ansi-${BASIC[c - 30]}`;
    else if (c >= 90 && c <= 97) s.fg = `ansi-bright-${BASIC[c - 90]}`;
    else if (c >= 40 && c <= 47) s.bg = `ansi-${BASIC[c - 40]}`;
    else if (c >= 100 && c <= 107) s.bg = `ansi-bright-${BASIC[c - 100]}`;
    else if (c === 39) s.fg = undefined;
    else if (c === 49) s.bg = undefined;
    else if (c === 38 || c === 48) {
      let color: string | undefined;
      if (codes[i + 1] === 5 && i + 2 < codes.length) {
        color = xterm256(codes[i + 2]);
        i += 2;
      } else if (codes[i + 1] === 2 && i + 4 < codes.length) {
        color = `rgb(${codes[i + 2]},${codes[i + 3]},${codes[i + 4]})`;
        i += 4;
      }
      if (c === 38) s.fg = color;
      else s.bg = color;
    }
  }
  return s;
}

function xterm256(n: number): string {
  if (n < 8) return `ansi-${BASIC[n]}`;
  if (n < 16) return `ansi-bright-${BASIC[n - 8]}`;
  if (n < 232) {
    const v = [0, 95, 135, 175, 215, 255];
    const i = n - 16;
    return `rgb(${v[Math.floor(i / 36)]},${v[Math.floor(i / 6) % 6]},${v[i % 6]})`;
  }
  const g = 8 + (n - 232) * 10;
  return `rgb(${g},${g},${g})`;
}

/** Renders terminal text into `parent` as styled spans. */
export function appendAnsi(parent: HTMLElement, text: string) {
  for (const span of parseAnsi(consoleText(text))) {
    if (!span.fg && !span.bg && !span.bold && !span.dim && !span.italic && !span.underline) {
      parent.append(span.text);
      continue;
    }
    const el = document.createElement("span");
    el.textContent = span.text;
    for (const [color, prop, cls] of [
      [span.fg, "color", "fg"],
      [span.bg, "backgroundColor", "bg"],
    ] as const) {
      if (!color) continue;
      if (color.startsWith("ansi-")) el.classList.add(`${color}-${cls}`);
      else el.style[prop] = color;
    }
    if (span.bold) el.classList.add("ansi-bold");
    if (span.dim) el.classList.add("ansi-dim");
    if (span.italic) el.classList.add("ansi-italic");
    if (span.underline) el.classList.add("ansi-underline");
    parent.append(el);
  }
}
