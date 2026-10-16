// The window's side of agents' work on documents. It tells the daemon where
// the user is (the document in front, its current cell and the selection),
// and makes the edits agents ask for in the documents it has open, as the
// user would: in the editor (one ⌘Z undoes it), saved, and run as with the
// cell's Run button. Documents no window has open are edited by the daemon.

import { createEffect, onCleanup } from "solid-js";
import type { AppRequest, Focus } from "../backend/types";
import type { AgentCell, Doc } from "../docs/doc";
import type { Workspace } from "../model/workspace";

/** Cursor moves are told the daemon once they settle. */
const FOCUS_DELAY_MS = 300;

/**
 * Keeps the daemon told where the user is in this project's window. Returns
 * what to call as the window closes, so agents' edits stop coming to it.
 */
export function reportPlace(
  ws: Workspace,
  project: string,
  docs: () => Doc[],
  active: () => Doc | undefined,
): () => Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let last: Focus = { open: [] };
  createEffect(() => {
    const connected = ws.link().status === "connected";
    const doc = active();
    doc?.moved();
    const open = docs().map((d) => d.path);
    clearTimeout(timer);
    if (!connected) return;
    timer = setTimeout(() => {
      last = { file: doc?.path, ...(doc?.place() ?? {}), open };
      // A daemon older than this window doesn't know the call; agents then
      // simply don't see where the user is.
      ws.call("set_focus", { project, focus: last }).catch(() => {});
    }, FOCUS_DELAY_MS);
  });
  onCleanup(() => clearTimeout(timer));
  return async () => {
    clearTimeout(timer);
    // The place stays for agents; the documents are no longer open here.
    const closed = ws.call("set_focus", { project, focus: { ...last, open: [] } }).catch(() => {});
    await Promise.race([closed, new Promise((resolve) => setTimeout(resolve, 1000))]);
  };
}

/** Makes agents' edits to this project's open documents. */
export function serveAgentEdits(ws: Workspace, project: string, open: (path: string) => Promise<Doc | undefined>) {
  ws.backend.onRequest((request) => {
    if (request.project === project) void serve(ws, request, open);
  });
}

async function serve(ws: Workspace, request: AppRequest, open: (path: string) => Promise<Doc | undefined>) {
  const { params } = request;
  let doc: Doc;
  let cell: AgentCell;
  try {
    // Brings the document to the front, so the user sees the edit.
    const found = await open(params.file);
    if (!found) throw new Error(`Sandhill could not open ${params.file}`);
    doc = found;
    // A daemon may send null for "not given".
    if (request.method === "add_cell") cell = doc.addCell(params.code, params.language ?? undefined, params.after ?? undefined);
    else if (request.method === "edit_cell") cell = doc.editCell(params.cell_id ?? "", params.code);
    else throw new Error(`this window can't ${request.method}`);
    await doc.save();
  } catch (err) {
    await ws.call("answer", { id: request.id, error: err instanceof Error ? err.message : String(err) }).catch(() => {});
    return;
  }
  await ws.call("answer", { id: request.id, result: { cell_id: cell.cellId, language: cell.language } }).catch(() => {});
  if (params.run) await doc.runAgentCell(cell, params.code);
}
