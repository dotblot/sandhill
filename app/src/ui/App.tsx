import { createEffect, createSignal, For, onMount, Show } from "solid-js";
import type { Shell } from "../backend";
import { CONFLICT_ERROR, type DirEntry, type FileText, type ProjectInfo, type ProjectState } from "../backend/types";
import { type Doc, docKind } from "../docs/doc";
import { NotebookDoc } from "../docs/notebookDoc";
import { QmdDoc } from "../docs/qmdDoc";
import { newNotebook, serializeNotebook } from "../model/ipynb";
import { pickKernel, type Workspace } from "../model/workspace";
import { ask, Dialogs, notify } from "./dialogs";
import { FileTree } from "./FileTree";
import { KernelBar } from "./KernelBar";
import { takeLegacyState } from "./legacy";
import { linkText } from "./linkText";

const SAVE_STATE_MS = 300;

/** A project window: the project's files, its open documents and their kernels. */
export function App(props: { ws: Workspace; shell: Shell; project: string }) {
  const ws = props.ws;
  const root = () => props.project;
  const name = props.project.slice(props.project.lastIndexOf("/") + 1) || props.project;
  const [missing, setMissing] = createSignal<string>();
  const [docs, setDocs] = createSignal<Doc[]>([]);
  const [active, setActive] = createSignal<Doc>();
  const [treeKey, setTreeKey] = createSignal(0);
  const opening = new Map<string, Promise<Doc | undefined>>();
  const hosts = new Map<Doc, HTMLElement>();
  let docArea!: HTMLDivElement;
  const [restored, setRestored] = createSignal(false);
  /** Shown outputs of documents that are not open, as the project state keeps them. */
  let shownOutputs: Record<string, string[]> = {};

  // Once connected, reopen the documents that were open in this project.
  let restoring = false;
  /** Set when the person opens a document before the tabs are back; their choice stays in front. */
  let openedWhileRestoring = false;
  createEffect(() => {
    if (!ws.host() || restoring) return;
    restoring = true;
    void (async () => {
      let project: ProjectInfo;
      try {
        project = await ws.call<ProjectInfo>("open_project", { path: props.project });
      } catch (err) {
        setMissing(message(err));
        // Try again after a reconnect, in case the daemon was what failed.
        restoring = false;
        return;
      }
      setMissing(undefined);
      const state = project.state ?? takeLegacyState(props.project);
      shownOutputs = state.outputs ?? {};
      for (const path of state.tabs ?? []) await open(path, true);
      const last = docs().find((d) => d.path === state.active);
      if (last && !openedWhileRestoring) setActive(last);
      setRestored(true);
    })();
  });

  // The daemon keeps the open documents for next time, not the folder.
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  const projectState = (): ProjectState => {
    const outputs = { ...shownOutputs };
    for (const doc of docs()) outputs[doc.path] = doc.shownOutputs();
    for (const [path, ids] of Object.entries(outputs)) if (!ids.length) delete outputs[path];
    return { tabs: docs().map((d) => d.path), active: active()?.path, outputs };
  };
  const saveState = () => {
    clearTimeout(saveTimer);
    saveTimer = undefined;
    return ws
      .call("save_project_state", { path: props.project, state: projectState() })
      .catch((err) => console.warn("could not save the project's tabs", err));
  };
  createEffect(() => {
    projectState();
    if (!restored()) return;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => void saveState(), SAVE_STATE_MS);
  });

  // Mount each document once, keep it alive while hidden so editors keep their state.
  createEffect(() => {
    const doc = active();
    for (const [d, el] of hosts) el.hidden = d !== doc;
    if (!doc) return;
    let el = hosts.get(doc);
    if (!el) {
      el = document.createElement("div");
      el.className = "doc-host";
      docArea.append(el);
      hosts.set(doc, el);
      doc.mount(el);
    }
    el.hidden = false;
    queueMicrotask(() => doc.focus());
  });

  async function open(path: string, quiet = false): Promise<Doc | undefined> {
    if (!quiet && !restored()) openedWhileRestoring = true;
    const existing = docs().find((d) => d.path === path);
    if (existing) {
      setActive(existing);
      return existing;
    }
    let pending = opening.get(path);
    if (!pending) {
      pending = (async () => {
        try {
          const shown = shownOutputs[path] ?? [];
          const doc =
            docKind(path) === "ipynb" ? await NotebookDoc.open(ws, path, shown) : await QmdDoc.open(ws, path, shown);
          setDocs((d) => [...d, doc]);
          return doc;
        } catch (err) {
          if (!quiet) notify(`Could not open ${path}: ${message(err)}`);
          return undefined;
        } finally {
          opening.delete(path);
        }
      })();
      opening.set(path, pending);
    }
    const doc = await pending;
    if (doc && (!quiet || !openedWhileRestoring)) setActive(doc);
    return doc;
  }

  async function saveDoc(doc: Doc): Promise<boolean> {
    try {
      await doc.save();
      return true;
    } catch (err) {
      if (!message(err).includes(CONFLICT_ERROR)) {
        notify(`Could not save ${doc.name}: ${message(err)}`);
        return false;
      }
      const choice = await ask(
        `${doc.name} changed on disk since you opened it.`,
        ["Overwrite", "Cancel"],
        "Overwriting replaces the version on disk with yours.",
      );
      if (choice !== "Overwrite") return false;
      await doc.save(true);
      return true;
    }
  }

  async function close(doc: Doc): Promise<boolean> {
    if (doc.dirty()) {
      const choice = await ask(`Save changes to ${doc.name}?`, ["Save", "Don't save", "Cancel"]);
      if (choice === "Cancel") return false;
      if (choice === "Save" && !(await saveDoc(doc))) return false;
    }
    shownOutputs[doc.path] = doc.shownOutputs();
    const list = docs();
    const index = list.indexOf(doc);
    const rest = list.filter((d) => d !== doc);
    setDocs(rest);
    if (active() === doc) setActive(rest[Math.min(index, rest.length - 1)]);
    hosts.get(doc)?.remove();
    hosts.delete(doc);
    doc.dispose();
    return true;
  }

  async function confirmClose(): Promise<boolean> {
    const dirty = docs().filter((d) => d.dirty());
    if (dirty.length) {
      const choice = await ask(
        dirty.length === 1 ? `Save changes to ${dirty[0].name}?` : `Save changes to ${dirty.length} documents?`,
        ["Save", "Don't save", "Cancel"],
        "Running code keeps running either way.",
      );
      if (choice === "Cancel") return false;
      if (choice === "Save") {
        for (const doc of dirty) if (!(await saveDoc(doc))) return false;
      }
    }
    if (saveTimer !== undefined) await saveState();
    return true;
  }

  async function createFile(kind: "qmd" | "ipynb") {
    const dir = root();
    try {
      const names = new Set((await ws.call<DirEntry[]>("list_dir", { path: dir })).map((e) => e.name));
      let n = 1;
      const name = () => `Untitled${n > 1 ? `-${n}` : ""}.${kind}`;
      while (names.has(name())) n++;
      const python = pickKernel(ws.kernelspecs(), "python");
      const text =
        kind === "ipynb"
          ? serializeNotebook(newNotebook(python))
          : "---\ntitle: \"Untitled\"\n---\n\n```{python}\nprint(\"hello\")\n```\n";
      const path = `${dir}/${name()}`;
      await ws.call<FileText>("write_file", { path, text });
      setTreeKey((k) => k + 1);
      await open(path);
    } catch (err) {
      notify(`Could not create a file in ${dir}: ${message(err)}`);
    }
  }

  function command(name: string) {
    const doc = active();
    if (name === "save" && doc) void saveDoc(doc);
    if (name === "close-tab" && doc) void close(doc);
    if (name === "run-all" && doc) doc.runAll().catch((e) => notify(message(e)));
  }

  onMount(() => {
    ws.backend.onMenu?.(command);
    ws.backend.onCloseRequested?.(confirmClose);
    window.addEventListener("keydown", (e) => {
      // In the Mac app the menu bar owns these shortcuts.
      const mod = e.metaKey || e.ctrlKey;
      if (!mod || ws.backend.onMenu) return;
      if (e.key === "s") command("save");
      else if (e.key === "w") command("close-tab");
      else if (e.altKey && (e.key === "r" || e.code === "KeyR")) command("run-all");
      else return;
      e.preventDefault();
    });
    window.addEventListener("beforeunload", (e) => {
      if (docs().some((d) => d.dirty())) e.preventDefault();
    });
  });

  return (
    <div class="app">
      <aside class="sidebar">
        <div class="project-header">
          <span class="project-name" title={props.project}>
            {name}
          </span>
          <button title="Open another project (⌘O)" onClick={() => void props.shell.showStart()}>
            Projects
          </button>
        </div>
        <div class="sidebar-actions">
          <button onClick={() => createFile("qmd")}>New Quarto</button>
          <button onClick={() => createFile("ipynb")}>New notebook</button>
          <button title="Refresh" onClick={() => setTreeKey((k) => k + 1)}>
            ⟳
          </button>
        </div>
        <Show when={!missing() && ws.link().status === "connected"}>
          <FileTree ws={ws} root={root()} onOpen={(p) => void open(p)} activePath={active()?.path} refreshKey={treeKey()} />
        </Show>
      </aside>
      <main class="main">
        <div class="tabs" role="tablist">
          <For each={docs()}>
            {(doc) => (
              <div
                class="tab"
                role="tab"
                classList={{ active: active() === doc }}
                title={doc.path}
                onClick={() => setActive(doc)}
              >
                <span>{doc.name}</span>
                <button
                  class="tab-close"
                  classList={{ dirty: doc.dirty() }}
                  title={doc.dirty() ? "Unsaved changes" : "Close"}
                  onClick={(e) => {
                    e.stopPropagation();
                    void close(doc);
                  }}
                >
                  {doc.dirty() ? "●" : "×"}
                </button>
              </div>
            )}
          </For>
        </div>
        <Show when={active()}>{(doc) => <KernelBar ws={ws} doc={doc()} onError={notify} />}</Show>
        <div class="doc-area" ref={docArea}>
          <Show when={!active()}>
            <Show
              when={!missing()}
              fallback={
                <div class="empty-state">
                  <p>Cannot open this project: {missing()}</p>
                  <p>
                    <button onClick={() => void props.shell.showStart()}>Show projects</button>
                  </p>
                </div>
              }
            >
              <div class="empty-state">
                <p>Open a .qmd, .Rmd or .ipynb file from the sidebar.</p>
                <p class="muted">Code runs in sessions that keep going when you close the app.</p>
              </div>
            </Show>
          </Show>
        </div>
        <footer class="status-bar" data-link={ws.link().status}>
          <span class="link-dot" />
          {linkText(ws)}
          <span class="spacer" />
          {active()?.path ?? ""}
        </footer>
      </main>
      <Dialogs />
    </div>
  );
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
