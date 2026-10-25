import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DirEntry, SessionInfo } from "../backend/types";
import { importsFor } from "../model/imports";
import type { Workspace } from "../model/workspace";
import { ImportDialog } from "./ImportDialog";

const settle = () => new Promise((r) => setTimeout(r, 0));
const entry = (name: string, kind: "file" | "dir" = "file"): DirEntry => ({ name, kind, size: 1, mtime_ms: 0 });

describe("ImportDialog", () => {
  afterEach(() => document.body.replaceChildren());

  it("shows the code for the picked file and runs it in the session", async () => {
    const execute = vi.fn(async () => "run-1");
    const ws = {
      call: async (_m: string, params: { path: string }) =>
        params.path === "/w/p" ? [entry("data", "dir"), entry("donors.csv"), entry("notes.md")] : [entry("pbmc counts.tsv")],
      execute,
    } as unknown as Workspace;
    const session = { id: "s1", language: "R", language_version: "4.4.1", cwd: "/w/p" } as SessionInfo;
    const kind = importsFor("R").find((k) => k.id === "readr")!;
    const onClose = vi.fn();
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <ImportDialog ws={ws} session={session} kind={kind} folder="/w/p" onClose={onClose} />, root);
    await settle();

    const items = () => [...root.querySelectorAll<HTMLButtonElement>(".imp-list button")];
    // Only folders and files readr reads.
    expect(items().map((b) => b.textContent)).toEqual(["data", "donors.csv"]);
    expect(root.querySelector(".modal-message")!.textContent).toBe("From text (readr) into R 4.4.1");
    items()[0].click();
    await settle();
    items()[0].click();
    expect(root.querySelector(".imp-code")!.textContent).toBe('pbmc_counts <- readr::read_tsv("data/pbmc counts.tsv")');

    const name = root.querySelector<HTMLInputElement>(".imp-name input")!;
    name.value = "2bad";
    name.dispatchEvent(new InputEvent("input", { bubbles: true }));
    expect(root.querySelector<HTMLButtonElement>(".importdlg .primary")!.disabled).toBe(true);
    name.value = "counts";
    name.dispatchEvent(new InputEvent("input", { bubbles: true }));
    root.querySelector<HTMLButtonElement>(".importdlg .primary")!.click();
    await settle();
    expect(execute).toHaveBeenCalledWith("s1", 'counts <- readr::read_tsv("data/pbmc counts.tsv")');
    expect(onClose).toHaveBeenCalled();
    dispose();
  });
});
