// Shared CodeMirror configuration for Quarto documents and notebook cells.

import { autocompletion, closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import {
  bracketMatching,
  HighlightStyle,
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
import { tags as t } from "@lezer/highlight";

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

/** Markdown with R and Python highlighting inside ```{r} and ```{python} chunks. */
export function quartoLanguage(): Extension {
  return markdown({
    codeLanguages: (info) => languageFor(/^\{?\s*([A-Za-z0-9_]+)/.exec(info)?.[1]) ?? null,
  });
}

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword], color: "var(--syn-keyword)" },
  { tag: [t.string, t.special(t.string), t.regexp], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--syn-number)" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.meta], color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--syn-function)" },
  { tag: [t.definition(t.variableName), t.className, t.typeName], color: "var(--syn-definition)" },
  { tag: [t.operator, t.punctuation], color: "var(--syn-operator)" },
  { tag: t.heading, fontWeight: "600", color: "var(--syn-heading)" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strong, fontWeight: "600" },
  { tag: [t.link, t.url], color: "var(--syn-link)" },
  { tag: t.monospace, color: "var(--syn-string)" },
  { tag: t.processingInstruction, color: "var(--syn-comment)" },
]);

const theme = EditorView.theme({
  "&": { color: "var(--text)", backgroundColor: "transparent", fontSize: "13px" },
  "&.cm-focused": { outline: "none" },
  ".cm-content": { fontFamily: "var(--font-mono)", caretColor: "var(--text)" },
  ".cm-gutters": { backgroundColor: "transparent", color: "var(--text-faint)", border: "none" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--active-line)" },
  ".cm-cursor": { borderLeftColor: "var(--text)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--selection) !important",
  },
  ".cm-selectionMatch": { backgroundColor: "var(--selection-match)" },
  ".cm-matchingBracket": { backgroundColor: "var(--selection-match)", outline: "none" },
  ".cm-panels": { backgroundColor: "var(--surface)", color: "var(--text)" },
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
    syntaxHighlighting(highlight),
    theme,
    opts.wrap ? EditorView.lineWrapping : [],
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, indentWithTab]),
  ];
}
