import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, Packages, SessionInfo } from "../backend/types";
import type { HelpRequest } from "../model/help";
import { Workspace } from "../model/workspace";
import { PackagesPane } from "./PackagesPane";

vi.mock("./dialogs", () => ({ ask: vi.fn(async () => "Remove"), notify: vi.fn() }));

const SESSION: SessionInfo = {
  id: "s1",
  kernel: "ir",
  language: "R",
  cwd: "/home/huan/pbmc",
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 0,
  project: "/home/huan/pbmc",
  pending_runs: 0,
  language_version: "4.4.1",
};

const LISTED: Packages = {
  session: "s1",
  libraries: ["/home/huan/R", "/usr/lib/R/library"],
  cran: true,
  bioconductor: true,
  conda: false,
  packages: [
    { name: "DESeq2", version: "1.44.0", title: "Differential gene expression analysis", library: "/home/huan/R", attached: false, loaded: false, base: false },
    { name: "ggplot2", version: "3.5.1", title: "Create Elegant Data Visualisations", library: "/home/huan/R", attached: true, loaded: true, base: false },
    { name: "Matrix", version: "1.7-0", title: "Sparse and Dense Matrix Classes", library: "/usr/lib/R/library", attached: false, loaded: true, base: false },
    { name: "stats", version: "4.4.1", title: "The R Stats Package", library: "/usr/lib/R/library", attached: true, loaded: true, base: true },
  ],
};

function fakeWorkspace() {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  let emit: (event: DaemonEvent) => void = () => {};
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "list_sessions":
          return [SESSION] as T;
        case "list_packages":
          return LISTED as T;
        default:
          return null as T;
      }
    },
    onEvent(listener) {
      emit = listener;
    },
    onTerminal() {},
    onRequest() {},
    openLink() {},
    onLink() {},
    blobUrl: () => "",
    reconnect: async () => {},
    disconnect: async () => {},
  };
  const ws = new Workspace(backend, "/home/huan/pbmc");
  const listings = () => calls.filter((c) => c.method === "list_packages").length;
  return { ws, listings, emit: (event: DaemonEvent) => emit(event) };
}

const settle = async () => {
  for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

afterEach(() => {
  document.body.replaceChildren();
});

function mount(ws: Workspace) {
  const helped: HelpRequest[] = [];
  const ran: string[] = [];
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(
    () => (
      <PackagesPane ws={ws} session={SESSION} sessions={[SESSION]} onSession={() => {}} onHelp={(r) => helped.push(r)} onRun={(_, code) => ran.push(code)} shown />
    ),
    root,
  );
  return { root, dispose, helped, ran };
}

describe("PackagesPane", () => {
  it("lists what's attached first, filters, and opens a package's help", async () => {
    const { ws, listings, emit } = fakeWorkspace();
    await ws.refreshSessions();
    const { root, dispose, helped } = mount(ws);
    await settle();
    expect(root.querySelector(".vmem")?.textContent).toBe("4 installed");
    expect([...root.querySelectorAll(".vh")].map((e) => e.textContent)).toEqual(["Attached", "Installed"]);
    expect([...root.querySelectorAll(".pkgrow .nm")].map((e) => e.textContent)).toEqual(["ggplot2", "stats", "DESeq2", "Matrix"]);
    expect([...root.querySelectorAll(".pkgrow .sz")].map((e) => e.textContent)).toEqual(["attached", "attached", "", "loaded"]);
    // R's own packages can't be removed.
    expect(root.querySelectorAll(".pkgrow .prm")).toHaveLength(3);
    const filter = root.querySelector('input[aria-label="Filter packages"]') as HTMLInputElement;
    filter.value = "gene";
    filter.dispatchEvent(new InputEvent("input", { bubbles: true }));
    expect([...root.querySelectorAll(".pkgrow .nm")].map((e) => e.textContent)).toEqual(["DESeq2"]);
    (root.querySelector(".pkgrow") as HTMLElement).click();
    expect(helped).toEqual([{ topic: "", package: "DESeq2" }]);
    // Read again after code runs (library(DESeq2) attaches it).
    emit({ seq: 1, session: "s1", run: "r1", msg_type: "sandhill.run_started", ts_ms: 1, content: { code: "library(DESeq2)" } });
    emit({ seq: 2, session: "s1", run: "r1", msg_type: "sandhill.run_finished", ts_ms: 2, content: { status: "ok" } });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(listings()).toBe(2);
    dispose();
  });

  it("installs and removes in the Console", async () => {
    const { ws } = fakeWorkspace();
    await ws.refreshSessions();
    const { root, dispose, ran } = mount(ws);
    await settle();
    (root.querySelector('button[title="Install packages"]') as HTMLButtonElement).click();
    expect(root.querySelector(".pi-note")?.textContent).toBe("Runs BiocManager::install() in the Console: CRAN and Bioconductor");
    const names = root.querySelector('input[aria-label="Packages to install"]') as HTMLInputElement;
    names.value = "harmony; rm";
    names.dispatchEvent(new InputEvent("input", { bubbles: true }));
    names.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(root.querySelector(".pi-note.err")?.textContent).toBe("Type package names, separated by spaces.");
    names.value = "harmony scran";
    names.dispatchEvent(new InputEvent("input", { bubbles: true }));
    (root.querySelector(".pi-row .primary") as HTMLButtonElement).click();
    expect(ran).toEqual(['BiocManager::install(c("harmony", "scran"), update = FALSE, ask = FALSE)']);
    expect(root.querySelector(".pinstall")).toBeNull();

    // Removing asks first, and doesn't open the package's help.
    (root.querySelectorAll(".pkgrow .prm")[2] as HTMLButtonElement).click();
    await settle();
    expect(ran[1]).toBe('remove.packages("Matrix")');
    dispose();
  });
});
