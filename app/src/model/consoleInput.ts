// What the Console's input needs to know about the code typed into it:
// whether Enter should run it or start a new line, and the code run before,
// for ↑ and ↓.

/**
 * Whether `code` is complete, so Enter runs it. It isn't while a bracket or
 * string is open, after an operator that wants more (R's `|>`, `+`, `<-`,
 * a trailing comma), or, in Python, after a `:` or `\` or inside an indented
 * block, which a blank line ends, as in IPython.
 */
export function isComplete(code: string, language: string): boolean {
  if (!code.trim()) return true;
  const python = language.toLowerCase().startsWith("python");
  const scan = scanCode(code, python);
  if (scan.open) return false;
  const last = scan.lastLine.trimEnd();
  if (python) {
    if (/[:\\]$/.test(last) || /^\s*@/.test(last)) return false;
    // In a block, a blank line ends it.
    const lines = code.split("\n");
    const indented = lines.length > 1 && /^\s+\S/.test(lines[lines.length - 1]);
    return !indented;
  }
  if (/(\|>|%[^%\s]*%|<<?-|->>?|[-+*/^&|~,=<>!?$@:])$/.test(last)) return false;
  return !waitsForBody(last);
}

/** An R `function (…)`, `if (…)`, `for (…)`, `while (…)` or `\(…)` ending the line, or `else`. */
function waitsForBody(line: string): boolean {
  if (/(^|[^\w.])(else|repeat)$/.test(line)) return true;
  const heads = /(?:\\|\b(?:function|if|for|while)\s*)\(/g;
  let open = -1;
  for (let m = heads.exec(line); m; m = heads.exec(line)) open = m.index + m[0].length - 1;
  if (open < 0) return false;
  let depth = 0;
  for (let i = open; i < line.length; i++) {
    if (line[i] === "(") depth++;
    else if (line[i] === ")" && --depth === 0) return i === line.length - 1;
  }
  return false;
}

interface Scan {
  /** A bracket or string is still open. */
  open: boolean;
  /** The last line with its comment taken off. */
  lastLine: string;
}

function scanCode(code: string, python: boolean): Scan {
  let depth = 0;
  let quote = "";
  let line = "";
  for (let i = 0; i < code.length; i++) {
    const c = code[i];
    if (quote) {
      line += c;
      if (c === "\\" && quote !== "`") {
        line += code[++i] ?? "";
        continue;
      }
      if (quote.length === 3 ? code.startsWith(quote, i) : c === quote) {
        if (quote.length === 3) {
          line += quote.slice(1);
          i += 2;
        }
        quote = "";
      }
      continue;
    }
    if (c === "#") {
      while (i + 1 < code.length && code[i + 1] !== "\n") i++;
      continue;
    }
    if (c === "\n") {
      line = "";
      continue;
    }
    line += c;
    if (python && (code.startsWith('"""', i) || code.startsWith("'''", i))) {
      quote = code.slice(i, i + 3);
      line += quote.slice(1);
      i += 2;
    } else if (c === '"' || c === "'" || (!python && c === "`")) {
      quote = c;
    } else if ("([{".includes(c)) {
      depth++;
    } else if (")]}".includes(c)) {
      depth = Math.max(0, depth - 1);
    }
  }
  return { open: depth > 0 || !!quote, lastLine: line };
}

/** The code run before, for ↑ and ↓, newest last. */
export class InputHistory {
  private at: number;
  /** What was typed before going back, for coming forward again. */
  private draft = "";

  constructor(private items: string[] = []) {
    this.at = items.length;
  }

  get entries(): readonly string[] {
    return this.items;
  }

  /** Adds code that ran, and goes back to the end. */
  push(code: string) {
    const text = code.replace(/\s+$/, "");
    if (text && this.items[this.items.length - 1] !== text) this.items.push(text);
    this.reset();
  }

  /** The code run before the one shown, or undefined at the start. */
  older(current: string): string | undefined {
    if (this.at === 0) return undefined;
    if (this.at === this.items.length) this.draft = current;
    return this.items[--this.at];
  }

  /** The code run after the one shown, then what was being typed. */
  newer(): string | undefined {
    if (this.at >= this.items.length) return undefined;
    this.at++;
    return this.at === this.items.length ? this.draft : this.items[this.at];
  }

  reset() {
    this.at = this.items.length;
    this.draft = "";
  }
}
