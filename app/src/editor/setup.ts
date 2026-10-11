// Shared CodeMirror configuration for Quarto documents and notebook cells.

import { autocompletion, closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { yamlFrontmatter } from "@codemirror/lang-yaml";
import {
  bracketMatching,
  indentOnInput,
  indentUnit,
  type Language,
  StreamLanguage,
  syntaxHighlighting,
} from "@codemirror/language";
import { r } from "@codemirror/legacy-modes/mode/r";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import type { Extension } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { highlightCode, tagHighlighter, tags as t } from "@lezer/highlight";

const rLanguage = StreamLanguage.define(r);
const pythonLanguage = python().language;

/** Editor language for a kernel or chunk language name. */
export function languageFor(name: string | undefined): Language | undefined {
  switch (name?.toLowerCase()) {
    case "r":
      return rLanguage;
    case "python":
    case "python3":
      return pythonLanguage;
    default:
      return undefined;
  }
}

/**
 * Markdown with R and Python highlighting inside ```{r} and ```{python}
 * chunks, and YAML in the front matter.
 */
export function quartoLanguage(): Extension {
  return yamlFrontmatter({
    content: markdown({
      codeLanguages: (info) => languageFor(/^\{?\s*([A-Za-z0-9_]+)/.exec(info)?.[1]) ?? null,
    }),
  });
}

/**
 * Classes for syntax tokens, colored in styles.css. Editors and the
 * highlighted code shown for cells not being edited share them.
 */
const highlighter = tagHighlighter([
  { tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword], class: "tok-keyword" },
  { tag: [t.string, t.special(t.string), t.regexp], class: "tok-string" },
  { tag: [t.number, t.bool, t.null, t.atom], class: "tok-number" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.meta], class: "tok-comment" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], class: "tok-function" },
  { tag: [t.definition(t.variableName), t.definition(t.propertyName), t.className, t.typeName], class: "tok-definition" },
  { tag: [t.operator, t.punctuation], class: "tok-operator" },
  { tag: t.heading, class: "tok-heading" },
  { tag: t.emphasis, class: "tok-emphasis" },
  { tag: t.strong, class: "tok-strong" },
  { tag: [t.link, t.url], class: "tok-link" },
  { tag: t.monospace, class: "tok-string" },
  { tag: t.processingInstruction, class: "tok-instruction" },
]);

/**
 * Code as highlighted lines laid out like an editor's, for showing a cell
 * without making an editor for it. Each line is a `div`, as in CodeMirror.
 */
export function highlightedLines(code: string, language: Language | undefined): HTMLElement[] {
  const lines: HTMLElement[] = [];
  let line = document.createElement("div");
  const putText = (text: string, classes: string) => {
    if (!classes) return line.append(text);
    const span = document.createElement("span");
    span.className = classes;
    span.textContent = text;
    line.append(span);
  };
  const putBreak = () => {
    if (!line.firstChild) line.append(document.createElement("br"));
    lines.push(line);
    line = document.createElement("div");
  };
  if (language) {
    highlightCode(code, language.parser.parse(code), highlighter, putText, putBreak);
  } else {
    code.split("\n").forEach((text, i) => {
      if (i) putBreak();
      putText(text, "");
    });
  }
  putBreak();
  return lines;
}

const theme = EditorView.theme({
  "&": { color: "var(--text)", backgroundColor: "transparent", fontSize: "var(--fs-code)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.3" },
  ".cm-content": { caretColor: "var(--text)" },
  ".cm-gutters": { backgroundColor: "transparent", color: "var(--faint)", border: "none", fontSize: "var(--fs-chrome)" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 12px 0 10px", minWidth: "34px" },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--accent-soft) 45%, transparent)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text)" },
  ".cm-cursor": { borderLeftColor: "var(--text)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--sel) !important",
  },
  ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--sel) 55%, transparent)" },
  ".cm-matchingBracket": { backgroundColor: "color-mix(in srgb, var(--sel) 55%, transparent)", outline: "none" },
  ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--text)" },
  ".cm-panels input, .cm-panels button": { fontSize: "var(--fs-chrome)" },
  ".cm-tooltip": { backgroundColor: "var(--bg)", border: "1px solid var(--line)", borderRadius: "6px" },
});

/** Extensions every editor gets; callers add a language and their own keymap first. */
export function baseExtensions(opts: { lineNumbers?: boolean; wrap?: boolean } = {}): Extension[] {
  return [
    opts.lineNumbers === false ? [] : [lineNumbers(), highlightActiveLineGutter()],
    history(),
    drawSelection(),
    indentOnInput(),
    indentUnit.of("  "),
    bracketMatching(),
    closeBrackets(),
    autocompletion({ activateOnTyping: false }),
    highlightActiveLine(),
    highlightSelectionMatches(),
    syntaxHighlighting(highlighter),
    theme,
    opts.wrap ? EditorView.lineWrapping : [],
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, indentWithTab]),
  ];
}
