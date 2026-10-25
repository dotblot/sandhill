import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import type { KernelspecInfo, ProjectSettings, ProjectSetup } from "../backend/types";
import { Workspace } from "../model/workspace";
import { ONLY_KERNEL, ProjectSettingsDialog, worthAsking } from "./ProjectSettings";

const settle = () => new Promise((r) => setTimeout(r, 0));
const notesOf = (root: Element) => [...root.querySelectorAll(".ps-note")].map((n) => n.textContent);

const SPECS: KernelspecInfo[] = [
  { name: "ark", display_name: "R (Ark)", language: "R" },
  { name: "ir", display_name: "R", language: "R" },
  { name: "python3", display_name: "Python 3", language: "python" },
  { name: "conda-env-omicverse-py", display_name: "Python (omicverse)", language: "python", env: "omicverse" },
];

/** A project whose environment.yml names omicverse, which has Python but no R kernel, and renv for R. */
const FOUND: ProjectSetup = {
  settings: { documents: "document", env_files: [".env"] },
  saved: false,
  found: [
    { language: "r", file: "renv.lock", label: "renv", kernel: "ark" },
    { language: "python", file: "environment.yml", label: "conda env omicverse", kernel: "conda-env-omicverse-py" },
    {
      language: "r",
      file: "environment.yml",
      label: "conda env omicverse",
      hint: "It has no R kernel: `conda install -n omicverse r-irkernel`.",
    },
  ],
  r: "ark",
  python: "conda-env-omicverse-py",
};

async function workspace(setup: ProjectSetup, specs = SPECS) {
  const saved: ProjectSettings[] = [];
  const backend = {
    host: "local",
    async call(method: string, params: Record<string, unknown> = {}) {
      if (method === "list_project_kernelspecs") return specs;
      if (method === "get_project_settings") return setup;
      if (method === "list_sessions") return [];
      if (method === "save_project_settings") {
        const settings = params.settings as ProjectSettings;
        saved.push(settings);
        return { ...setup, settings, saved: true };
      }
      return null;
    },
    onEvent() {},
    onLink() {},
  } as unknown as Backend;
  const ws = new Workspace(backend, "/home/huan/pbmc");
  await ws.refreshKernelspecs();
  await ws.refreshSetup();
  return { ws, saved };
}

function show(ws: Workspace, setup: ProjectSetup, first: boolean, onClose = vi.fn(), onSaved = vi.fn()) {
  const root = document.body.appendChild(document.createElement("div"));
  render(() => <ProjectSettingsDialog ws={ws} setup={setup} name="pbmc" first={first} onSaved={onSaved} onClose={onClose} />, root);
  const select = (id: string) => root.querySelector<HTMLSelectElement>(`#ps-${id}`)!;
  const options = (id: string) => [...select(id).options].map((o) => o.textContent);
  const button = (text: string) => [...root.querySelectorAll<HTMLButtonElement>(".modal-buttons button")].find((b) => b.textContent?.trim() === text)!;
  return { root, select, options, button, onClose, onSaved };
}

describe("ProjectSettingsDialog", () => {
  afterEach(() => document.body.replaceChildren());

  it("asks every project without saved settings, once a kernel is installed", () => {
    const plain: ProjectSetup = { settings: FOUND.settings, saved: false, found: [] };
    expect(worthAsking(FOUND, SPECS)).toBe(true);
    // A folder that points nowhere, with one kernel of each language, as on a
    // server whose only Python with ipykernel is omicverse.
    expect(worthAsking(plain, [SPECS[0], SPECS[3]])).toBe(true);
    expect(worthAsking(plain, [])).toBe(false);
    expect(worthAsking({ ...FOUND, saved: true }, SPECS)).toBe(false);
  });

  it("says why a language has only one kernel to pick", async () => {
    const plain: ProjectSetup = { settings: FOUND.settings, saved: false, found: [], r: "ark", python: "conda-env-omicverse-py" };
    const { ws } = await workspace(plain, [SPECS[0], SPECS[3]]);
    const { root, select, options } = show(ws, plain, true);
    expect(root.querySelector(".modal-message")!.textContent).toBe("Which R and Python should pbmc use?");
    expect(options("python")).toEqual(["Automatic: Python (omicverse)", "Python (omicverse)"]);
    expect(select("python").value).toBe("conda-env-omicverse-py");
    const notes = notesOf(root);
    expect(notes).toContain("Only environments with ipykernel are listed (for a conda env: `conda install -n <env> ipykernel`).");
    expect(notes).toContain("Only R installations with a kernel (Ark, or IRkernel) are listed.");
  });

  it("offers what the folder points to and saves the choice", async () => {
    const { ws, saved } = await workspace(FOUND);
    const { root, select, options, button, onClose, onSaved } = show(ws, FOUND, true);
    expect(root.querySelector(".modal-message")!.textContent).toBe("Which R and Python should pbmc use?");
    expect(options("r")).toEqual(["Automatic: R (Ark), from renv.lock", "R (Ark)", "R"]);
    expect(options("python")).toEqual(["Automatic: Python (omicverse), from environment.yml", "Python 3", "Python (omicverse)"]);
    // Not saved yet: what runs now is picked, so saving keeps it.
    expect(select("r").value).toBe("ark");
    expect(select("python").value).toBe("conda-env-omicverse-py");
    const notes = notesOf(root);
    expect(notes).toContain("Found renv.lock (renv).");
    expect(notes).toContain(
      "Found environment.yml (conda env omicverse). It has no R kernel: `conda install -n omicverse r-irkernel`.",
    );
    expect(notes).toContain("R starts in the project's folder, so renv loads the project's library.");

    select("python").value = "python3";
    select("python").dispatchEvent(new Event("change"));
    select("docs").value = "project";
    select("docs").dispatchEvent(new Event("change"));
    const env = root.querySelector<HTMLInputElement>("#ps-env")!;
    env.value = ".env, secrets.env ,";
    env.dispatchEvent(new InputEvent("input", { bubbles: true }));
    button("Save").click();
    await settle();
    expect(saved).toEqual([{ r: "ark", python: "python3", documents: "project", env_files: [".env", "secrets.env"] }]);
    // The file tree shows .sandhill/ then.
    expect(onSaved).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
    expect(ws.setup()?.saved).toBe(true);
    expect(ws.documentFolder("/home/huan/pbmc/reports")).toBe("/home/huan/pbmc");
  });

  it("keeps a saved choice, even of a kernel this machine doesn't have", async () => {
    const setup: ProjectSetup = {
      ...FOUND,
      saved: true,
      settings: { python: "conda-env-scvi-py", documents: "document", env_files: [] },
      python: "conda-env-scvi-py",
    };
    const { ws, saved } = await workspace(setup);
    const { root, select, options, button, onClose } = show(ws, setup, false);
    expect(root.querySelector(".modal-message")!.textContent).toBe("Project settings · pbmc");
    // R says "detect" in the file.
    expect(select("r").value).toBe("");
    expect(select("python").value).toBe("conda-env-scvi-py");
    expect(options("python").at(-1)).toBe("conda-env-scvi-py (not installed here)");
    expect(root.querySelector<HTMLInputElement>("#ps-env")!.value).toBe("");
    button("Cancel").click();
    expect(onClose).toHaveBeenCalled();
    expect(saved).toEqual([]);
    expect(notesOf(root)).not.toContain(ONLY_KERNEL.python);
  });
});
