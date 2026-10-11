import { createEffect, createSignal, For, onMount, Show } from "solid-js";
import { CONFLICT_ERROR, type DirEntry, type FileText } from "../backend/types";
import { type Doc, docKind } from "../docs/doc";
import { NotebookDoc } from "../docs/notebookDoc";
import { QmdDoc } from "../docs/qmdDoc";
import { newNotebook, serializeNotebook } from "../model/ipynb";
import { pickKernel, type Workspace } from "../model/workspace";
import { ask, Dialogs, notify } from "./dialogs";
import { FileTree } from "./FileTree";
import { KernelBar } from "./KernelBar";

const ROOT_KEY = "sandhill.root";
const TABS_KEY = "sandhill.tabs";

export function App(props: { ws: Workspace }) {
  const ws = props.ws;
  const [root, setRoot] = createSignal<string>(load(ROOT_KEY) ?? "");
  const [rootInput, setRootInput] = createSignal(root());
  const [docs, setDocs] = createSignal<Doc[]>([]);
  const [active, setActive] = createSignal<Doc>();
  const [treeKey, setTreeKey] = createSignal(0);
  const opening = new Map<string, Promise<Doc | undefined>>();
  const hosts = new Map<Doc, HTMLElement>();
  let docArea!: HTMLDivElement;
  const [restored, setRestored] = createSignal(false);

  // Once connected, default to the home folder and reopen the last tabs.
  createEffect(() => {
    const host = ws.host();
    if (!host || restored()) return;
    if (!root()) changeRoot(host.home);
    const tabs: string[] = JSON.parse(load(TABS_KEY) ?? "[]");
    void (async () => {
      for (const path of tabs) await open(path, true);
      setRestored(true);
    })();
  });

  createEffect(() => {
    const paths = docs().map((d) => d.path);
    if (restored()) save(TABS_KEY, JSON.stringify(paths));
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

  function changeRoot(path: string) {
    const clean = path.trim().replace(/(.)\/+$/, "$1");
    setRoot(clean);
    setRootInput(clean);
    save(ROOT_KEY, clean);
  }

  async function open(path: string, quiet = false): Promise<Doc | undefined> {
    const existing = docs().find((d) => d.path === path);
    if (existing) {
      setActive(existing);
      return existing;
    }
    let pending = opening.get(path);
    if (!pending) {
      pending = (async () => {
        try {
          const doc = docKind(path) === "ipynb" ? await NotebookDoc.open(ws, path) : await QmdDoc.open(ws, path);
          setDocs((d) => [...d, doc]);
          setActive(doc);
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
    return pending;
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

  async function confirmQuit(): Promise<boolean> {
    const dirty = docs().filter((d) => d.dirty());
    if (!dirty.length) return true;
    const choice = await ask(
      dirty.length === 1 ? `Save changes to ${dirty[0].name}?` : `Save changes to ${dirty.length} documents?`,
      ["Save", "Don't save", "Cancel"],
      "Running code keeps running either way.",
    );
    if (choice === "Cancel") return false;
    if (choice === "Save") {
      for (const doc of dirty) if (!(await saveDoc(doc))) return false;
    }
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
    ws.backend.onCloseRequested?.(confirmQuit);
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

  const linkText = () => {
    const link = ws.link();
    const host = ws.host();
    if (link.status === "connected" && host) return `Connected to sandhilld ${host.daemon_version} on ${host.hostname}`;
    if (link.status === "connected") return "Connected";
    if (link.status === "connecting") return "Connecting to sandhilld…";
    return `Disconnected${link.error ? `: ${link.error}` : ""}. Retrying…`;
  };

  return (
    <div class="app">
      <aside class="sidebar">
        <form
          class="root-form"
          onSubmit={(e) => {
            e.preventDefault();
            changeRoot(rootInput());
          }}
        >
          <button type="button" title="Parent folder" onClick={() => changeRoot(root().replace(/\/[^/]+$/, "") || "/")}>
            ↑
          </button>
          <input value={rootInput()} onInput={(e) => setRootInput(e.currentTarget.value)} spellcheck={false} />
        </form>
        <div class="sidebar-actions">
          <button onClick={() => createFile("qmd")}>New Quarto</button>
          <button onClick={() => createFile("ipynb")}>New notebook</button>
          <button title="Refresh" onClick={() => setTreeKey((k) => k + 1)}>
            ⟳
          </button>
        </div>
        <Show when={root() && ws.link().status === "connected"}>
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
            <div class="empty-state">
              <p>Open a .qmd, .Rmd or .ipynb file from the sidebar.</p>
              <p class="muted">Code runs in sessions that keep going when you close the app.</p>
            </div>
          </Show>
        </div>
        <footer class="status-bar" data-link={ws.link().status}>
          <span class="link-dot" />
          {linkText()}
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

function load(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function save(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage can be unavailable; remembering tabs is only a convenience.
  }
}
