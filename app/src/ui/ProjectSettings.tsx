// Project settings: the R and Python each language runs in, where notebooks
// and Quarto documents run, and the env files kernels start with. The
// daemon saves them in .sandhill/project.toml, so collaborators get them
// too. The first time a project opens with a choice to make (an
// environment its files point to, or more than one kernel of a language),
// this asks once; the ☰ menu brings it back.

import { createSignal, For, onMount, Show } from "solid-js";
import type { DocumentsRunIn, FoundEnvironment, KernelspecInfo, ProjectSettings, ProjectSetup } from "../backend/types";
import { normalizeLanguage, pickKernel, type Workspace } from "../model/workspace";
import { notify } from "./dialogs";

const LANGUAGES = [
  { id: "r", label: "R" },
  { id: "python", label: "Python" },
] as const;

/** Whether a project has something to ask about: what its files point to, or a choice of kernels. */
export function worthAsking(setup: ProjectSetup, specs: KernelspecInfo[]): boolean {
  if (setup.saved) return false;
  if (setup.found.length) return true;
  return LANGUAGES.some(({ id }) => specs.filter((k) => normalizeLanguage(k.language) === id).length > 1);
}

/** "Found renv.lock (renv)." or "Found environment.yml (conda env omicverse).", then what to do when it has no kernel. */
export function foundNote(found: FoundEnvironment): string {
  const what = found.file === found.label ? `Found ${found.file}.` : `Found ${found.file} (${found.label}).`;
  return found.hint ? `${what} ${found.hint}` : what;
}

/**
 * The kernel a language starts as picked: the saved one ("" for detect),
 * else what the project runs now, so saving keeps it.
 */
export function initialKernel(ws: Workspace, setup: ProjectSetup, language: "r" | "python"): string {
  if (setup.saved) return setup.settings[language] ?? "";
  return ws.projectKernel(language) ?? ws.pick(language)?.name ?? "";
}

export function ProjectSettingsDialog(props: {
  ws: Workspace;
  setup: ProjectSetup;
  /** The project's name, for the title. */
  name: string;
  /** Asked unprompted, on opening the project: the way out is "Not now". */
  first?: boolean;
  onClose: () => void;
}) {
  const ws = props.ws;
  const [kernels, setKernels] = createSignal<Record<string, string>>({
    r: initialKernel(ws, props.setup, "r"),
    python: initialKernel(ws, props.setup, "python"),
  });
  const [documents, setDocuments] = createSignal<DocumentsRunIn>(props.setup.settings.documents);
  const [envFiles, setEnvFiles] = createSignal(props.setup.settings.env_files.join(", "));
  const [saving, setSaving] = createSignal(false);
  const specsFor = (language: string) => ws.kernelspecs().filter((k) => normalizeLanguage(k.language) === language);
  /** What "Automatic" runs: the first environment found with a kernel, else the usual kernel. */
  const automatic = (language: string) => {
    const found = props.setup.found.find((f) => f.language === language && f.kernel);
    const spec = found ? ws.kernelspecs().find((k) => k.name === found.kernel) : pickKernel(ws.kernelspecs(), language);
    const name = spec?.display_name ?? found?.kernel;
    if (!name) return "none installed";
    return found && !name.includes(found.file) ? `${name}, from ${found.file}` : name;
  };

  async function save() {
    const files = envFiles()
      .split(",")
      .map((f) => f.trim())
      .filter(Boolean);
    const settings: ProjectSettings = {
      r: kernels().r || undefined,
      python: kernels().python || undefined,
      documents: documents(),
      env_files: files,
    };
    setSaving(true);
    try {
      await ws.saveSettings(settings);
      const running = ws.projectSessions().length;
      notify(
        running
          ? "Saved in .sandhill/project.toml. New sessions follow it; running ones keep their kernel until restarted."
          : "Saved in .sandhill/project.toml.",
        "info",
      );
      props.onClose();
    } catch (err) {
      notify(`Could not save the project's settings: ${err instanceof Error ? err.message : String(err)}`);
      setSaving(false);
    }
  }

  let first: HTMLSelectElement | undefined;
  onMount(() => first?.focus());
  return (
    <div class="modal-backdrop" onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
      <div class="modal projdlg" role="dialog" aria-modal="true" aria-label="Project settings">
        <p class="modal-message">{props.first ? `Which R and Python should ${props.name} use?` : `Project settings · ${props.name}`}</p>
        <p class="modal-detail">Saved in .sandhill/project.toml in the project's folder. Commit it to share these with collaborators.</p>
        <Show when={props.setup.problem}>
          <p class="ps-problem">Couldn't read the file: {props.setup.problem}</p>
        </Show>
        <div class="ps-grid">
          <For each={LANGUAGES}>
            {({ id, label }) => {
              const notes = () => props.setup.found.filter((f) => f.language === id);
              const chosen = () => kernels()[id];
              // A saved kernel this machine doesn't have stays listed, so it shows.
              const missing = () => !!chosen() && !specsFor(id).some((k) => k.name === chosen());
              return (
                <>
                  <label class="ps-label" for={`ps-${id}`}>
                    {label}
                  </label>
                  <div class="ps-field">
                    <select
                      id={`ps-${id}`}
                      ref={(el) => {
                        if (id === "r") first = el;
                      }}
                      value={chosen()}
                      onChange={(e) => setKernels((k) => ({ ...k, [id]: e.currentTarget.value }))}
                    >
                      <option value="">Automatic: {automatic(id)}</option>
                      <For each={specsFor(id)}>{(k) => <option value={k.name}>{k.display_name}</option>}</For>
                      <Show when={missing()}>
                        <option value={chosen()}>{chosen()} (not installed here)</option>
                      </Show>
                    </select>
                    <For each={notes()}>
                      {(found) => (
                        <p class="ps-note" classList={{ warn: !!found.hint }}>
                          {foundNote(found)}
                        </p>
                      )}
                    </For>
                    <Show when={id === "r" && notes().some((f) => f.file === "renv.lock")}>
                      <p class="ps-note">R starts in the project's folder, so renv loads the project's library.</p>
                    </Show>
                  </div>
                </>
              );
            }}
          </For>
          <label class="ps-label" for="ps-docs">
            Notebooks
          </label>
          <div class="ps-field">
            <select id="ps-docs" value={documents()} onChange={(e) => setDocuments(e.currentTarget.value as DocumentsRunIn)}>
              <option value="document">Run in their own folder, as quarto render does</option>
              <option value="project">Run in the project's folder</option>
            </select>
            <p class="ps-note">For .ipynb, .qmd and .Rmd. The Console and scripts run in the project's folder.</p>
          </div>
          <label class="ps-label" for="ps-env">
            Env files
          </label>
          <div class="ps-field">
            <input
              id="ps-env"
              value={envFiles()}
              placeholder="none"
              spellcheck={false}
              autocomplete="off"
              onInput={(e) => setEnvFiles(e.currentTarget.value)}
              onKeyDown={(e) => e.key === "Enter" && void save()}
            />
            <p class="ps-note">NAME=value lines new R and Python sessions start with, comma-separated. Missing files are skipped.</p>
          </div>
        </div>
        <div class="modal-buttons">
          <button onClick={props.onClose}>{props.first ? "Not now" : "Cancel"}</button>
          <button class="primary" disabled={saving()} onClick={() => void save()}>
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
