// Finds the executable chunks of a Quarto (.qmd) or R Markdown (.Rmd)
// document: fenced blocks whose info string is in braces, like ```{r}.
// Plain ``` blocks are shown as code but never run, as in Quarto.

export interface Chunk {
  /** 0-based position among executable chunks. */
  index: number;
  /** Lower-case engine name from the header: "r", "python", ... */
  language: string;
  label?: string;
  /** Stable id for saved outputs: the label, else `chunk-<position>`. */
  cellId: string;
  /** Offsets of the opening fence line's start and the closing fence line's end. */
  from: number;
  to: number;
  /** 1-based line numbers of the fences. `closeLine` is the last line if unclosed. */
  openLine: number;
  closeLine: number;
  closed: boolean;
  /** The code between the fences, without the fences. */
  code: string;
  codeFrom: number;
  codeTo: number;
}

const OPEN = /^\s{0,3}(`{3,}|~{3,})\s*(.*)$/;
const ENGINE = /^\{\s*([A-Za-z0-9_]+)\s*(.*?)\s*\}\s*$/;

export function parseChunks(text: string): Chunk[] {
  const chunks: Chunk[] = [];
  const lines = splitWithOffsets(text);
  let i = 0;
  while (i < lines.length) {
    const open = OPEN.exec(lines[i].text);
    if (!open) {
      i++;
      continue;
    }
    const fence = open[1];
    const start = i;
    let end = start + 1;
    while (end < lines.length && !isClosingFence(lines[end].text, fence)) end++;
    const closed = end < lines.length;
    const engine = ENGINE.exec(open[2]);
    if (engine) {
      const codeLines = lines.slice(start + 1, end);
      const codeFrom = lines[start].end + lineBreakLength(text, lines[start].end);
      const codeTo = codeLines.length ? codeLines[codeLines.length - 1].end : codeFrom;
      const code = text.slice(codeFrom, codeTo);
      const language = engine[1].toLowerCase();
      const label = labelFrom(engine[2], code);
      const index = chunks.length;
      const lastLine = closed ? end : lines.length - 1;
      chunks.push({
        index,
        language,
        label,
        cellId: label ?? `chunk-${index + 1}`,
        from: lines[start].start,
        to: lines[lastLine].end,
        openLine: start + 1,
        closeLine: lastLine + 1,
        closed,
        code,
        codeFrom,
        codeTo,
      });
    }
    i = closed ? end + 1 : lines.length;
  }
  return chunks;
}

/** Chunk options from the R Markdown header (`{r name, echo=FALSE}`) or `#| label:` lines. */
function labelFrom(headerRest: string, code: string): string | undefined {
  for (const line of code.split("\n")) {
    const option = /^\s*#\|\s*label\s*:\s*(.+?)\s*$/.exec(line);
    if (option) return unquote(option[1]);
    if (!/^\s*#\|/.test(line)) break;
  }
  const rest = headerRest.replace(/^,/, "").trim();
  if (!rest) return undefined;
  const named = /(?:^|,)\s*label\s*=\s*("[^"]*"|'[^']*'|[^,\s]+)/.exec(rest);
  if (named) return unquote(named[1]);
  const first = rest.split(",")[0].trim();
  return first && !first.includes("=") ? unquote(first) : undefined;
}

function unquote(s: string): string {
  return s.replace(/^(["'])(.*)\1$/, "$2");
}

function isClosingFence(line: string, fence: string): boolean {
  const m = /^\s{0,3}(`{3,}|~{3,})\s*$/.exec(line);
  return !!m && m[1][0] === fence[0] && m[1].length >= fence.length;
}

interface Line {
  text: string;
  start: number;
  end: number;
}

function splitWithOffsets(text: string): Line[] {
  const lines: Line[] = [];
  let start = 0;
  for (;;) {
    const nl = text.indexOf("\n", start);
    const rawEnd = nl === -1 ? text.length : nl;
    const end = rawEnd > start && text[rawEnd - 1] === "\r" ? rawEnd - 1 : rawEnd;
    lines.push({ text: text.slice(start, end), start, end });
    if (nl === -1) break;
    start = nl + 1;
  }
  return lines;
}

function lineBreakLength(text: string, at: number): number {
  if (text.startsWith("\r\n", at)) return 2;
  return at < text.length ? 1 : 0;
}

/** The `jupyter:` kernel named in the YAML front matter, if any. */
export function frontMatterKernel(text: string): string | undefined {
  const m = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text);
  if (!m) return undefined;
  const kernel = /^jupyter\s*:\s*([^\s#]+)\s*$/m.exec(m[1]);
  if (kernel) return unquote(kernel[1]);
  const nested = /^jupyter\s*:\s*\n\s+kernelspec\s*:\s*\n(?:\s+.*\n)*?\s+name\s*:\s*(\S+)/m.exec(m[1]);
  return nested ? unquote(nested[1]) : undefined;
}

/** Normalizes code for comparing what ran with what is in the editor now. */
export function sameCode(a: string, b: string): boolean {
  const norm = (s: string) => s.replace(/\r\n/g, "\n").replace(/[ \t]+$/gm, "").trim();
  return norm(a) === norm(b);
}
