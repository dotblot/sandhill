// The Help and Packages panes' logic that needs no screen: help asked for at
// the Console, links in R's help pages, Python docstrings as blocks, and the
// code that installs and removes packages.

import type { Package, Packages } from "../backend/types";

/** What to show in the Help pane. */
export interface HelpRequest {
  /** The function or topic; empty to list a package's topics. */
  topic: string;
  package?: string;
  /** Search for it rather than open its page (R's `??`). */
  search?: boolean;
}

const R_NAME = String.raw`(?:[A-Za-z.][\w.]*|\x60[^\x60]+\x60|"[^"]+"|'[^']+')`;
const PY_NAME = String.raw`[A-Za-z_][\w.]*`;

function unquote(name: string): string {
  return /^(["'`]).*\1$/.test(name) ? name.slice(1, -1) : name;
}

/**
 * Help asked for at the Console, which opens in the Help pane rather than
 * running: R's `?lm`, `?stats::lm`, `??pca`, `help(lm)`, `help("lm",
 * package = "stats")`, `help(package = "stats")`; Python's `?np.mean`,
 * `np.mean?` and `help(np.mean)`.
 */
export function consoleHelp(code: string, language: string): HelpRequest | undefined {
  const text = code.trim();
  if (!text || text.includes("\n")) return undefined;
  if (language.toLowerCase().startsWith("python")) {
    const m =
      new RegExp(`^\\?{1,2}\\s*(${PY_NAME})$`).exec(text) ??
      new RegExp(`^(${PY_NAME})\\s*\\?{1,2}$`).exec(text) ??
      new RegExp(`^help\\(\\s*(${PY_NAME})\\s*\\)$`).exec(text) ??
      new RegExp(`^help\\(\\s*["'](${PY_NAME})["']\\s*\\)$`).exec(text);
    return m ? { topic: m[1] } : undefined;
  }
  let m = /^\?\?\s*(.+)$/.exec(text);
  if (m) return { topic: unquote(m[1].trim()), search: true };
  m = new RegExp(`^\\?\\s*(?:([A-Za-z][\\w.]*):::?)?(${R_NAME})$`).exec(text);
  if (m) return { topic: unquote(m[2]), package: m[1] };
  m = /^help\((.*)\)$/.exec(text);
  if (!m) return undefined;
  const args = m[1].split(",").map((a) => a.trim());
  let topic: string | undefined;
  let pkg: string | undefined;
  for (const [i, arg] of args.entries()) {
    const named = /^(topic|package)\s*=\s*(.+)$/.exec(arg);
    const value = (named ? named[2] : arg).trim();
    if (!new RegExp(`^${R_NAME}$`).test(value)) return undefined;
    const field = named ? named[1] : i === 0 ? "topic" : i === 1 ? "package" : "";
    if (field === "topic") topic = unquote(value);
    else if (field === "package") pkg = unquote(value);
    else return undefined;
  }
  if (topic === undefined && !pkg) return undefined;
  return { topic: topic ?? "", package: pkg };
}

/**
 * The name at a column of a line, for F1 in an editor: R's `stats::filter`
 * (with its package) or `read.csv`, Python's `sc.pp.neighbors`.
 */
export function nameAt(line: string, column: number, language: string): HelpRequest | undefined {
  const python = language.toLowerCase().startsWith("python");
  const part = python ? /[\w.]/ : /[\w.:]/;
  let from = column;
  let to = column;
  while (from > 0 && part.test(line[from - 1])) from--;
  while (to < line.length && part.test(line[to])) to++;
  const name = line.slice(from, to).replace(/^[.:]+|[.:]+$/g, "");
  if (!name || /^\d/.test(name)) return undefined;
  const qualified = /^([A-Za-z][\w.]*):::?(.+)$/.exec(name);
  if (!python && qualified) return { topic: qualified[2], package: qualified[1] };
  return name.includes(":") ? undefined : { topic: name };
}

export type HelpLink =
  | { kind: "topic"; topic: string; package?: string }
  | { kind: "example"; topic: string; package?: string }
  | { kind: "index"; package?: string }
  | { kind: "anchor"; id: string }
  | { kind: "web"; url: string };

function decode(part: string): string {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
}

/**
 * Where a link in an R help page goes. Pages link to topics as
 * `../../stats/help/lm` (or `../help/lm` within their package, or
 * `../../stats/html/lm.html`), to their examples as `../Example/lm`, and to
 * the package's index as `00Index.html`.
 */
export function helpLink(href: string, from?: string): HelpLink | undefined {
  if (/^(https?|mailto|ftp):/i.test(href)) return { kind: "web", url: href };
  if (href.startsWith("#")) return { kind: "anchor", id: decode(href.slice(1)) };
  const path = href.split(/[?#]/)[0];
  let m = /^\.\.\/\.\.\/([^/]+)\/html\/00Index\.html$/.exec(path) ?? /^(?:\.\.\/)*00Index\.html$/.exec(path);
  if (m) return { kind: "index", package: m[1] ?? from };
  m = /^\.\.\/\.\.\/([^/]+)\/(help|html)\/(.+)$/.exec(path);
  if (m) return { kind: "topic", package: m[1], topic: decode(m[3]).replace(m[2] === "html" ? /\.html$/ : /$^/, "") };
  m = /^\.\.\/help\/(.+)$/.exec(path) ?? /^(?!\.\.\/)([^/]+)\.html$/.exec(path);
  if (m) return { kind: "topic", package: from, topic: decode(m[1]) };
  m = /^\.\.\/Example\/(.+)$/.exec(path);
  if (m) return { kind: "example", package: from, topic: decode(m[1]) };
  return undefined;
}

/** The code that runs a topic's examples in R. */
export function exampleCode(topic: string, pkg?: string): string {
  return `example(${JSON.stringify(topic)}${pkg ? `, package = ${JSON.stringify(pkg)}` : ""})`;
}

export type DocBlock =
  | { kind: "heading"; text: string }
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "note"; title: string; text: string }
  | { kind: "defs"; section: string; items: { term: string; text: string }[] };

/** reStructuredText's admonitions, shown as notes. */
const NOTES = /^(note|warning|caution|important|tip|hint|attention|danger|error|seealso|deprecated|versionadded|versionchanged)$/;
/** Its directives whose content is code. */
const CODE_DIRECTIVES = /^(code-block|code|sourcecode|ipython|plot|math|doctest|testcode|parsed-literal)$/;

/** Sections whose entries are names with descriptions under them. */
const DEF_SECTIONS =
  /^(parameters|other parameters|params|args|arguments|keyword args|keyword arguments|kwargs|returns?|yields?|raises|attributes|methods|see also|warns)$/i;
const GOOGLE_HEADING =
  /^(Args|Arguments|Parameters|Params|Keyword Args|Keyword Arguments|Kwargs|Returns?|Yields?|Raises|Attributes|Methods|Examples?|Notes?|See Also|References|Warnings?|Warns|Todo)\s*:\s*$/;

function indentOf(line: string): number {
  return line.length - line.trimStart().length;
}

/**
 * A Python docstring as headings, paragraphs, code and lists of names with
 * their descriptions, from numpydoc's underlined sections (`Parameters`
 * over `----------`), Google style's `Args:`, doctests (`>>>`) and
 * reStructuredText's `::` blocks.
 */
export function docBlocks(doc: string): DocBlock[] {
  const lines = doc.replace(/\r\n?/g, "\n").replace(/\t/g, "    ").split("\n");
  const blocks: DocBlock[] = [];
  let section = "";
  let para: string[] = [];
  const flush = () => {
    if (para.length) blocks.push({ kind: "text", text: joinLines(para) });
    para = [];
  };
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const next = lines[i + 1] ?? "";
    const trimmed = line.trim();
    // numpydoc: a heading over a line of dashes or equals signs.
    if (trimmed && /^\s*(-{3,}|={3,})\s*$/.test(next) && next.trim().length >= trimmed.length - 2 && !trimmed.startsWith(">>>")) {
      flush();
      section = trimmed;
      blocks.push({ kind: "heading", text: trimmed });
      i += 2;
      continue;
    }
    const google = GOOGLE_HEADING.exec(trimmed);
    if (google && indentOf(line) === 0) {
      flush();
      section = google[1];
      blocks.push({ kind: "heading", text: google[1] });
      i++;
      continue;
    }
    if (!trimmed) {
      flush();
      i++;
      continue;
    }
    // Doctests run to the next blank line.
    if (trimmed.startsWith(">>>")) {
      flush();
      const base = indentOf(line);
      const code: string[] = [];
      while (i < lines.length && lines[i].trim()) code.push(lines[i++].slice(base));
      blocks.push({ kind: "code", text: code.join("\n") });
      continue;
    }
    if (DEF_SECTIONS.test(section)) {
      flush();
      const base = indentOf(line);
      const items: { term: string; text: string }[] = [];
      // Text outdented from the entries ends the section (Google style's).
      let ended = false;
      while (i < lines.length) {
        const here = lines[i];
        if (!here.trim()) {
          // A blank line between entries doesn't end the list; the next
          // heading, or text outdented from it, does.
          let j = i + 1;
          while (j < lines.length && !lines[j].trim()) j++;
          if (j >= lines.length) break;
          if (indentOf(lines[j]) < base) {
            ended = true;
            break;
          }
          if (/^\s*(-{3,}|={3,})\s*$/.test(lines[j + 1] ?? "") || (indentOf(lines[j]) === 0 && GOOGLE_HEADING.test(lines[j].trim()))) break;
          i = j;
          continue;
        }
        if (indentOf(here) < base) {
          ended = true;
          break;
        }
        if (indentOf(here) === base) {
          const entry = here.trim();
          // Google style puts the description after the name: "x (int): The x."
          const inline = /^([\w*.]+(?:\s*\([^)]*\))?)\s*:\s+(.+)$/.exec(entry);
          items.push(inline && !/ : /.test(entry) ? { term: inline[1], text: inline[2] } : { term: entry, text: "" });
        } else if (items.length) {
          const last = items[items.length - 1];
          last.text = last.text ? `${last.text}\n${here.trim()}` : here.trim();
        }
        i++;
      }
      blocks.push({ kind: "defs", section, items: items.map((it) => ({ term: it.term, text: joinLines(it.text.split("\n")) })) });
      if (ended) section = "";
      continue;
    }
    // reStructuredText directives: ".. warning::", ".. versionadded:: 1.2", ".. code-block:: python".
    const directive = /^\.\.\s+([\w-]+)::\s*(.*)$/.exec(trimmed);
    if (directive) {
      flush();
      const base = indentOf(line);
      i++;
      const content: string[] = [];
      while (i < lines.length && (!lines[i].trim() || indentOf(lines[i]) > base)) content.push(lines[i++]);
      while (content.length && !content[content.length - 1].trim()) content.pop();
      // Options (":linenos:") aren't content.
      const body = content.filter((l) => !/^\s*:[\w-]+:/.test(l));
      const strip = Math.min(...body.filter((l) => l.trim()).map(indentOf));
      const text = body.map((l) => l.slice(strip)).join("\n").replace(/^\n+/, "");
      const [, name, arg] = directive;
      if (NOTES.test(name)) {
        const titles: Record<string, string> = { seealso: "See also", versionadded: "Added in version", versionchanged: "Changed in version", deprecated: "Deprecated since version" };
        const title = name in titles ? `${titles[name]}${arg ? ` ${arg}` : ""}` : name[0].toUpperCase() + name.slice(1);
        blocks.push({ kind: "note", title, text: joinLines([name in titles ? "" : arg, ...text.split("\n")]) });
      } else if (CODE_DIRECTIVES.test(name)) {
        if (text) blocks.push({ kind: "code", text });
      } else if (text) {
        blocks.push({ kind: "text", text: joinLines(text.split("\n")) });
      }
      continue;
    }
    // reStructuredText: an indented block after a line ending in "::".
    if (/::$/.test(trimmed) && i + 1 < lines.length) {
      para.push(trimmed.replace(/::$/, trimmed === "::" ? "" : ":"));
      flush();
      const base = indentOf(line);
      i++;
      while (i < lines.length && !lines[i].trim()) i++;
      const code: string[] = [];
      while (i < lines.length && (!lines[i].trim() || indentOf(lines[i]) > base)) code.push(lines[i++]);
      while (code.length && !code[code.length - 1].trim()) code.pop();
      const strip = Math.min(...code.filter((l) => l.trim()).map(indentOf));
      if (code.length) blocks.push({ kind: "code", text: code.map((l) => l.slice(strip)).join("\n") });
      continue;
    }
    para.push(line);
    i++;
  }
  flush();
  return blocks;
}

/** Hard-wrapped lines as one paragraph, keeping list items on lines of their own. */
function joinLines(lines: string[]): string {
  let out = "";
  for (const raw of lines) {
    const line = raw.trim();
    if (!line) continue;
    if (!out) out = line;
    else if (/^([-*•]|\d+[.)])\s/.test(line)) out += `\n${line}`;
    else out += ` ${line}`;
  }
  return tidyDirectives(out);
}

/** reStructuredText directives inside a paragraph, as words: ".. deprecated:: 3.0" reads "Deprecated since version 3.0:". */
function tidyDirectives(text: string): string {
  const words: Record<string, string> = { deprecated: "Deprecated since version", versionadded: "Added in version", versionchanged: "Changed in version" };
  return text.replace(/\.\.\s+([\w-]+)::\s*(\S*)/g, (_, name: string, arg: string) =>
    name in words ? `${words[name]} ${arg.replace(/[.:]$/, "")}:` : `${name[0].toUpperCase()}${name.slice(1)}: ${arg}`.trimEnd(),
  );
}

export type Inline = { text: string } | { code: string } | { em: string } | { ref: string; label: string } | { url: string; label: string };

/**
 * Text with its inline code (``x``, `x`) and Sphinx references
 * (:func:`pandas.concat`, :class:`~anndata.AnnData`) marked, the
 * references so they can be followed.
 */
export function inlineParts(text: string): Inline[] {
  const parts: Inline[] = [];
  const re = /`([^`<]+?)\s*<(https?:\/\/[^>\s]+)>`_{1,2}|:(?:py:)?(?:func|class|meth|mod|attr|obj|data|exc|ref|term|doc):`~?([^`]+)`|``([^`]+)``|`([^`]+)`|(?<![\w*])\*{1,2}([^*\s](?:[^*]*[^*\s])?)\*{1,2}(?![\w*])/g;
  let at = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > at) parts.push({ text: text.slice(at, m.index) });
    if (m[1] !== undefined) {
      parts.push({ url: m[2], label: m[1] });
    } else if (m[3] !== undefined) {
      const named = /^(.*?)\s*<([^>]+)>$/.exec(m[3]);
      const target = named ? named[2] : m[3];
      const label = named ? named[1] : m[0].includes("`~") ? target.split(".").pop()! : target;
      parts.push(/^[\w.]+$/.test(target) ? { ref: target.replace(/\(\)$/, ""), label } : { code: label });
    } else if (m[6] !== undefined) {
      parts.push({ em: m[6] });
    } else {
      parts.push({ code: m[4] ?? m[5] });
    }
    at = m.index + m[0].length;
  }
  if (at < text.length) parts.push({ text: text.slice(at) });
  return parts;
}

/**
 * A call's signature, one argument to a line when it is long: `(adata,
 * target_sum=None, …)` from inspect.signature.
 */
export function signatureLines(name: string, signature: string, width = 60): string {
  const short = name.split(".").pop() ?? name;
  const flat = `${short}${signature}`;
  if (flat.length <= width) return flat;
  const inner = signature.replace(/^\(/, "").replace(/\)(\s*->.*)?$/, "");
  const tail = /\)(\s*->.*)?$/.exec(signature)?.[1] ?? "";
  const args = splitTop(inner);
  return `${short}(\n${args.map((a) => `    ${a.trim()},`).join("\n")}\n)${tail}`;
}

/** Splits at commas outside brackets and strings. */
function splitTop(text: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote = "";
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = "";
    } else if (c === "'" || c === '"') quote = c;
    else if ("([{".includes(c)) depth++;
    else if (")]}".includes(c)) depth--;
    else if (c === "," && depth === 0) {
      out.push(text.slice(start, i));
      start = i + 1;
    }
  }
  if (text.slice(start).trim()) out.push(text.slice(start));
  return out;
}

/** What a package's row says about it: attached, imported or loaded. */
export function packageStatus(pkg: Package, language: string): string {
  if (pkg.attached) return language.toLowerCase() === "r" ? "attached" : "imported";
  return pkg.loaded ? "loaded" : "";
}

/** Packages whose name or title has `text`, names first. */
export function filterPackages(packages: Package[], text: string): Package[] {
  const q = text.trim().toLowerCase();
  if (!q) return packages;
  const named = packages.filter((p) => p.name.toLowerCase().includes(q));
  const titled = packages.filter((p) => !p.name.toLowerCase().includes(q) && p.title.toLowerCase().includes(q));
  return [...named, ...titled];
}

/**
 * The names typed to install: R's package names (separated by spaces or
 * commas), or pip's requirements (`scanpy[leiden]>=1.10`). Undefined when
 * one isn't a name.
 */
export function packageNames(text: string, language: string): string[] | undefined {
  const python = language.toLowerCase().startsWith("python");
  const names = text.split(python ? /\s+/ : /[\s,]+/).filter(Boolean);
  const valid = python ? /^[A-Za-z0-9][\w.\-]*(\[[\w.,\-]+\])?([<>=!~]=?[\w.*+\-]+(,[<>=!~]=?[\w.*+\-]+)*)?$/ : /^[A-Za-z][A-Za-z0-9.]*$/;
  return names.length && names.every((n) => valid.test(n)) ? names : undefined;
}

function rVector(names: string[]): string {
  const quoted = names.map((n) => JSON.stringify(n));
  return quoted.length === 1 ? quoted[0] : `c(${quoted.join(", ")})`;
}

/**
 * The Console code that installs packages: BiocManager when it is installed
 * (so Bioconductor's packages work too), otherwise install.packages(), with
 * a CRAN mirror set if there isn't one; pip, or conda when asked, in Python.
 */
export function installCode(language: string, names: string[], where: Pick<Packages, "cran" | "bioconductor">, conda = false): string {
  if (language.toLowerCase().startsWith("python")) {
    return conda ? `%conda install -y -c conda-forge -c bioconda ${names.join(" ")}` : `%pip install ${names.join(" ")}`;
  }
  const mirror = where.cran ? "" : `options(repos = c(CRAN = "https://cloud.r-project.org"))\n`;
  return where.bioconductor
    ? `${mirror}BiocManager::install(${rVector(names)}, update = FALSE, ask = FALSE)`
    : `${mirror}install.packages(${rVector(names)})`;
}

/** The Console code that removes packages: conda's own with conda's tool. */
export function removeCode(language: string, packages: Package[]): string {
  if (!language.toLowerCase().startsWith("python")) return `remove.packages(${rVector(packages.map((p) => p.name))})`;
  const conda = packages.filter((p) => p.installer === "conda").map((p) => p.name);
  const pip = packages.filter((p) => p.installer !== "conda").map((p) => p.name);
  return [conda.length ? `%conda remove -y ${conda.join(" ")}` : "", pip.length ? `%pip uninstall -y ${pip.join(" ")}` : ""].filter(Boolean).join("\n");
}

/** The topic to open for a package: R's package overview, Python's module. */
export function packageTopic(pkg: Package, language: string): HelpRequest {
  if (language.toLowerCase() === "r") return { topic: "", package: pkg.name };
  const modules = pkg.modules ?? [];
  const module = modules.find((m) => m.toLowerCase() === pkg.name.toLowerCase().replace(/-/g, "_")) ?? modules.find((m) => !m.startsWith("_")) ?? pkg.name.replace(/-/g, "_");
  return { topic: module };
}
