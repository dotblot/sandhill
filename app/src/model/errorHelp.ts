// Fix and Explain, on an error in an output: the message that hands the
// error, and the code that raised it, to the agent in the Agents pane.

import type { MessageRef, Output } from "../backend/types";
import { plainText } from "../render/ansi";

export type ErrorAction = "fix" | "explain";

/** An error, and the code that raised it. */
export interface ErrorAsk {
  action: ErrorAction;
  error: Output;
  /** The code that ran. */
  code: string;
  /** Its language, for the code block: "r", "python". */
  language?: string;
  /** The cell it ran from: the file's path, the cell's id, and its name ("cell 3", "markers") when known. */
  cell?: { path: string; id: string; name?: string };
  /** The script it ran from, run a line or selection at a time in the Console. */
  file?: string;
  /** The session it ran in, when not a document's. */
  session?: string;
}

/** How much of a long traceback the agent gets: its end, where the error is. */
const MAX_LINES = 80;

/** The error as plain text: its traceback, else its name and message. */
export function errorText(error: Output): string {
  const traceback: string[] = Array.isArray(error.traceback) ? error.traceback : [];
  const text = plainText(traceback.length ? traceback.join("\n") : `${error.ename ?? "Error"}: ${error.evalue ?? ""}`);
  const lines = text.trimEnd().split("\n");
  return lines.length > MAX_LINES ? ["…", ...lines.slice(-MAX_LINES)].join("\n") : lines.join("\n");
}

/** Where an error came from, for the message: "cell 3 of de.ipynb", "the R Console". */
export function errorPlace(ask: ErrorAsk, project?: string): string {
  const language = ask.language === "python" ? "Python " : ask.language === "r" ? "R " : "";
  if (ask.cell) return `${ask.cell.name ?? "a cell"} of ${inProject(ask.cell.path, project)}`;
  if (ask.file) return `${inProject(ask.file, project)}, run in the ${language}Console`;
  return `the ${language}Console`;
}

/**
 * The message for the agent, and the cell to attach to it, which tells the
 * agent which cell to change. Paths are given from the project's folder.
 */
export function errorMessage(ask: ErrorAsk, project?: string): { text: string; refs: MessageRef[] } {
  const where = errorPlace(ask, project);
  const lead =
    ask.action === "fix"
      ? `Fix this error from ${where}: change the code so it works, then run it again to check.`
      : `Explain this error from ${where}: what went wrong and how to fix it. Don't change anything yet.`;
  const code = ask.code.trimEnd();
  const error = errorText(ask.error);
  const text = [lead, "", "The code:", block(code, ask.language), "", "The error:", block(error)].join("\n");
  const refs: MessageRef[] = [];
  if (ask.cell) {
    const file = ask.cell.path.split("/").pop() ?? ask.cell.path;
    const path = inProject(ask.cell.path, project);
    refs.push({ kind: "chunk", name: `${file} › ${ask.cell.name ?? "cell"}`, path, cell: ask.cell.id });
  }
  return { text, refs };
}

function inProject(path: string, project?: string): string {
  return project && path.startsWith(`${project}/`) ? path.slice(project.length + 1) : path;
}

/** A fenced block, its fence longer than any run of backticks inside. */
function block(text: string, language = ""): string {
  const longest = Math.max(2, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  return `${fence}${language}\n${text}\n${fence}`;
}
