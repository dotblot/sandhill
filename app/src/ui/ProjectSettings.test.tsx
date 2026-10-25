import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import type { KernelspecInfo, ProjectSettings, ProjectSetup } from "../backend/types";
import { Workspace } from "../model/workspace";
import { ProjectSettingsDialog, worthAsking } from "./ProjectSettings";

const settle = () => new Promise((r) => setTimeout(r, 0));

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

async function workspace(setup: ProjectSetup) {
  const saved: ProjectSettings[] = [];
  const backend = {
    host: "local",
    async call(method: string, params: Record<string, unknown> = {}) {
      if (method === "list_project_kernelspecs") return SPECS;
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

function show(ws: Workspace, setup: ProjectSetup, first: boolean, onClose = vi.fn()) {
  const root = document.body.appendChild(document.createElement("div"));
  render(() => <ProjectSettingsDialog ws={ws} setup={setup} name="pbmc" first={first} onClose={onClose} />, root);
  const select = (id: string) => root.querySelector<HTMLSelectElement>(`#ps-${id}`)!;
  const options = (id: string) => [...select(id).options].map((o) => o.textContent);
  const button = (text: string) => [...root.querySelectorAll<HTMLButtonElement>(".modal-buttons button")].find((b) => b.textContent?.trim() === text)!;
  return { root, select, options, button, onClose };
}

describe("ProjectSettingsDialog", () => {
  afterEach(() => document.body.replaceChildren());

  it("asks only when there is something to choose", () => {
    const plain: ProjectSetup = { settings: FOUND.settings, saved: false, found: [] };
    expect(worthAsking(FOUND, SPECS)).toBe(true);
    // Two R kernels are a choice even when the folder points nowhere.
    expect(worthAsking(plain, SPECS)).toBe(true);
    expect(worthAsking(plain, [SPECS[0], SPECS[2]])).toBe(false);
    expect(worthAsking({ ...FOUND, saved: true }, SPECS)).toBe(false);
  });

  it("offers what the folder points to and saves the choice", async () => {
    const { ws, saved } = await workspace(FOUND);
    const { root, select, options, button, onClose } = show(ws, FOUND, true);
    expect(root.querySelector(".modal-message")!.textContent).toBe("Which R and Python should pbmc use?");
    expect(options("r")).toEqual(["Automatic: R (Ark), from renv.lock", "R (Ark)", "R"]);
    expect(options("python")).toEqual(["Automatic: Python (omicverse), from environment.yml", "Python 3", "Python (omicverse)"]);
    // Not saved yet: what runs now is picked, so saving keeps it.
    expect(select("r").value).toBe("ark");
    expect(select("python").value).toBe("conda-env-omicverse-py");
    const notes = [...root.querySelectorAll(".ps-note")].map((n) => n.textContent);
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
  });
});
