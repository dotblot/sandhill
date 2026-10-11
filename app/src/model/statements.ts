// Splits an R or Python script into the statements ⌘↵ runs, as RStudio and
// VS Code do: a call spread over several lines, a pipe chain, a function or
// a Python block is one statement, wherever in it the cursor is.

/** Lines of one statement, 1-based and inclusive. */
export interface Statement {
  from: number;
  to: number;
}

interface LineInfo {
  /** Has code outside comments, or is inside a string. */
  code: boolean;
  /** The line's code without its comment, trimmed. */
  text: string;
  indent: number;
  /** Still inside brackets or a string at the end of the line. */
  open: boolean;
}

/** R operators that, ending a line, continue the expression on the next. */
const R_CONTINUES = /(?:[-+*/^,|&~=<>!:]|%[^%\s]*%|\|>)$/;
/** Python lines that open a block when they end in a colon at the top level. */
const PY_BLOCK_KEEPS = /^(?:else|elif|except|finally)\b/;

export function statements(text: string, language: string): Statement[] {
  const python = language.toLowerCase().startsWith("python");
  const lines = scan(text.split("\n"), python);
  const result: Statement[] = [];
  let i = 0;
  while (i < lines.length) {
    if (!lines[i].code) {
      i++;
      continue;
    }
    const from = i;
    // Brackets, strings and trailing operators carry on to the next line.
    const continues = (n: number) =>
      lines[n].open ||
      (python ? lines[n].text.endsWith("\\") || lines[n].text.startsWith("@") : R_CONTINUES.test(lines[n].text));
    while (i + 1 < lines.length && continues(i)) i++;
    // A Python block runs on while lines are indented further.
    if (python && lines[i].text.endsWith(":")) {
      const base = lines[from].indent;
      let last = i;
      for (let n = i + 1; n < lines.length; n++) {
        const line = lines[n];
        if (!line.code) continue;
        if (line.indent > base || (line.indent === base && PY_BLOCK_KEEPS.test(line.text))) {
          last = n;
          while (last + 1 < lines.length && continues(last)) last++;
          n = last;
        } else break;
      }
      i = last;
    }
    result.push({ from: from + 1, to: i + 1 });
    i++;
  }
  return result;
}

/** The statement on `line` (1-based), or undefined on a blank or comment line. */
export function statementAt(text: string, language: string, line: number): Statement | undefined {
  return statements(text, language).find((s) => s.from <= line && line <= s.to);
}

/** Tracks brackets, strings and comments line by line. */
function scan(source: string[], python: boolean): LineInfo[] {
  let depth = 0;
  /** The quote that opened the string we are in, if any: ', ", `, ''' or """. */
  let quote = "";
  return source.map((line) => {
    const startsInString = quote !== "";
    let end = line.length;
    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      if (quote) {
        if (ch === "\\") i++;
        else if (line.startsWith(quote, i)) {
          i += quote.length - 1;
          quote = "";
        }
        continue;
      }
      if (ch === "#") {
        end = i;
        break;
      }
      if (ch === '"' || ch === "'" || (!python && ch === "`")) {
        const triple = python && line.startsWith(ch.repeat(3), i);
        quote = triple ? ch.repeat(3) : ch;
        if (triple) i += 2;
      } else if (ch === "(" || ch === "[" || ch === "{") depth++;
      else if (ch === ")" || ch === "]" || ch === "}") depth = Math.max(0, depth - 1);
    }
    const text = line.slice(0, end).trim();
    return {
      code: startsInString || text !== "",
      text,
      indent: line.length - line.trimStart().length,
      open: depth > 0 || quote !== "",
    };
  });
}
