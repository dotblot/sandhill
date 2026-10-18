// A conversation exported from the chat: Markdown to read or keep with the
// project, a standalone HTML page that also has the plots, or JSON with every
// entry the daemon kept (what an import would read back).

import DOMPurify from "dompurify";
import { marked } from "marked";
import { askView, buildChat, type ChatEntry, type ChatItem, type Step, stepView } from "./chat";

export type ExportFormat = "md" | "html" | "json";

export const EXPORT_FORMATS: { format: ExportFormat; label: string }[] = [
  { format: "md", label: "Markdown" },
  { format: "html", label: "HTML" },
  { format: "json", label: "JSON" },
];

/** What the export says about the conversation. */
export interface ExportInfo {
  title: string;
  /** "Claude", "Codex". */
  agent: string;
  /** The agent's kind, as the daemon names it: `claude`, `codex`. */
  kind: string;
  model: string;
  documents: string[];
  at: Date;
}

/** One part of the conversation: Markdown, and the plots that go with it. */
interface Block {
  md: string;
  images: string[];
}

export function exportConversation(format: ExportFormat, entries: ChatEntry[], info: ExportInfo): string {
  switch (format) {
    case "md":
      return exportMarkdown(entries, info);
    case "html":
      return exportHtml(entries, info);
    case "json":
      return exportJson(entries, info);
  }
}

export function exportMarkdown(entries: ChatEntry[], info: ExportInfo): string {
  const parts = blocks(entries, info).map((b) => {
    if (!b.images.length) return b.md;
    const plots = b.images.length === 1 ? "1 plot" : `${b.images.length} plots`;
    return `${b.md}\n\n*${plots}, in the HTML export*`;
  });
  return `${[heading(info), ...parts].join("\n\n")}\n`;
}

export function exportHtml(entries: ChatEntry[], info: ExportInfo): string {
  const body = [heading(info), ...blocks(entries, info)]
    .map((b) => {
      const block = typeof b === "string" ? { md: b, images: [] } : b;
      const images = block.images.map((src) => `<img src="${escapeAttr(src)}" alt="Plot">`).join("\n");
      return `<section>${markdown(block.md)}${images}</section>`;
    })
    .join("\n");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeText(info.title)}</title>
<style>${STYLE}</style>
</head>
<body>
<main>
${body}
</main>
</body>
</html>
`;
}

export function exportJson(entries: ChatEntry[], info: ExportInfo): string {
  const conversation = {
    sandhill: "conversation",
    version: 1,
    title: info.title,
    agent: info.kind,
    model: info.model,
    documents: info.documents,
    exported: info.at.toISOString(),
    entries,
  };
  return `${JSON.stringify(conversation, null, 2)}\n`;
}

/** A file name for the export, from the conversation's title. */
export function exportName(title: string, format: ExportFormat): string {
  const slug = title
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 50)
    .replace(/-+$/, "");
  return `${slug || "conversation"}.${format}`;
}

function heading(info: ExportInfo): string {
  const about = [info.agent, info.model, ...info.documents].filter(Boolean).join(" · ");
  return `# ${oneLine(info.title)}\n\n${about}  \nExported from Sandhill on ${info.at.toLocaleString()}`;
}

function blocks(entries: ChatEntry[], info: ExportInfo): Block[] {
  const out: Block[] = [];
  let speaker: "user" | "agent" | undefined;
  const say = (who: "user" | "agent") => {
    if (speaker !== who) out.push({ md: `## ${who === "user" ? "You" : info.agent}`, images: [] });
    speaker = who;
  };
  for (const item of buildChat(entries, false).items) {
    const block = itemBlock(item, info.agent);
    if (!block) continue;
    if (item.kind === "user") say("user");
    else if (item.kind !== "note") say("agent");
    out.push(block);
  }
  return out;
}

function itemBlock(item: ChatItem, agent: string): Block | undefined {
  switch (item.kind) {
    case "user":
    case "reply":
      return item.text.trim() ? { md: item.text.trim(), images: [] } : undefined;
    case "step":
      return stepBlock(item);
    case "ask":
      return { md: `**${agent} asked:** ${oneLine(askView(item.ask, undefined, agent).title)} · ${answerText(item.ask.answer)}`, images: [] };
    case "note":
      return { md: `*${item.text}*`, images: [] };
    case "error":
      return { md: `> **Error:** ${item.text}`, images: [] };
    default:
      return undefined;
  }
}

function stepBlock(step: Step): Block {
  const view = stepView(step);
  if (view.name === "thinking") {
    const text = view.output?.trim();
    return { md: text ? quote(text) : "*Thought*", images: [] };
  }
  const outcome: Record<string, string | undefined> = { fail: "failed", deny: "you said no", cancel: "stopped" };
  const line = [view.desc, view.detail, outcome[step.status]]
    .filter((part): part is string => !!part)
    .map(oneLine)
    .join(" · ");
  const parts = [`**\`${view.name}\`**${line ? ` ${line}` : ""}`];
  if (view.code) parts.push(fence(view.code, view.language === "shell" ? "sh" : (view.language ?? "")));
  if (view.diff?.length) parts.push(fence(view.diff.join("\n"), "diff"));
  if (view.output) parts.push(fence(view.output, ""));
  if (step.error && step.status === "fail") parts.push(fence(step.error, ""));
  return { md: parts.join("\n\n"), images: view.images };
}

function answerText(answer: { behavior: string } | null | undefined): string {
  if (answer === undefined) return "no answer";
  if (answer === null) return "it stopped waiting";
  return answer.behavior === "allow" ? "you allowed it" : "you said no";
}

/** A fenced code block, with a fence longer than any run of backticks in it. */
function fence(text: string, language: string): string {
  const longest = Math.max(2, ...[...text.matchAll(/`+/g)].map((m) => m[0].length));
  const ticks = "`".repeat(longest + 1);
  return `${ticks}${language}\n${text.replace(/\n+$/, "")}\n${ticks}`;
}

function quote(text: string): string {
  return text
    .split("\n")
    .map((line) => (line ? `> ${line}` : ">"))
    .join("\n");
}

function oneLine(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

function markdown(text: string): string {
  return DOMPurify.sanitize(marked.parse(text, { async: false, gfm: true }) as string);
}

function escapeText(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(text: string): string {
  return escapeText(text).replace(/"/g, "&quot;");
}

const STYLE = `
:root { --bg: #fbfaf7; --fg: #24221f; --muted: #6f6a62; --line: #e3dfd7; --code: #f1eee8; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #1d1c1a; --fg: #e8e4dc; --muted: #a39d93; --line: #3a3733; --code: #282623; }
}
body { margin: 0; background: var(--bg); color: var(--fg); font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
main { max-width: 780px; margin: 0 auto; padding: 32px 16px 64px; }
h1 { font-size: 22px; margin: 0 0 4px; }
h1 + p { color: var(--muted); font-size: 13px; margin-top: 0; }
h2 { font-size: 15px; margin: 28px 0 0; padding-top: 12px; border-top: 1px solid var(--line); }
section > :first-child { margin-top: 12px; }
pre { background: var(--code); border-radius: 6px; padding: 10px 12px; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 12.5px; line-height: 1.45; }
code { font-family: ui-monospace, Menlo, monospace; font-size: 0.92em; }
blockquote { margin: 12px 0; padding-left: 12px; border-left: 3px solid var(--line); color: var(--muted); }
img { display: block; max-width: 100%; margin: 8px 0; border-radius: 4px; background: #fff; }
table { border-collapse: collapse; } td, th { border: 1px solid var(--line); padding: 3px 8px; }
`;
