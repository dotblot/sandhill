import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { LOCAL_HOST, type Shell } from "../backend";
import {
  CONFLICT_ERROR,
  type DirEntry,
  type FileText,
  type ProjectInfo,
  type ProjectState,
  type SessionInfo,
} from "../backend/types";
import { type Doc, docKind } from "../docs/doc";
import { NotebookDoc } from "../docs/notebookDoc";
import { QmdDoc } from "../docs/qmdDoc";
import { newNotebook, serializeNotebook } from "../model/ipynb";
import { pickKernel, type Workspace } from "../model/workspace";
import smallIcon from "../../src-tauri/icons/32x32.png";
import { ask, Dialogs, notify } from "./dialogs";
import { FileTree } from "./FileTree";
import { FileIcon, FilesRailIcon, Icon } from "./icons";
import { KernelBar } from "./KernelBar";
import { takeLegacyState } from "./legacy";
import { linkText } from "./linkText";

const SAVE_STATE_MS = 300;
const SIDEBAR_KEY = "sandhill.sidebar";

/** A project window: the project's files, its open documents and their kernels. */
export function App(props: { ws: Workspace; shell: Shell; project: string }) {
  const ws = props.ws;
  const root = () => props.project;
  const name = props.project.slice(props.project.lastIndexOf("/") + 1) || props.project;
  const [missing, setMissing] = createSignal<string>();
  const [docs, setDocs] = createSignal<Doc[]>([]);
  const [active, setActive] = createSignal<Doc>();
  const [treeKey, setTreeKey] = createSignal(0);
  const [sidebar, setSidebar] = createSignal(stored(SIDEBAR_KEY) !== "hidden");
  const toggleSidebar = () => {
    setSidebar(!sidebar());
    store(SIDEBAR_KEY, sidebar() ? "shown" : "hidden");
  };
  const hostName = ws.backend.host === LOCAL_HOST ? "This Mac" : ws.backend.host;
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
      // A new notebook names the kernel of the Python session it will join.
      const joining = ws.sessionToJoin("python", dir)?.kernel;
      const python = ws.kernelspecs().find((k) => k.name === joining) ?? pickKernel(ws.kernelspecs(), "python");
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
      const mod = e.metaKey || e.ctrlKey;
      if (mod && !e.altKey && !e.shiftKey && e.key === "b") {
        e.preventDefault();
        toggleSidebar();
        return;
      }
      // In the Mac app the menu bar owns these shortcuts.
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
    <div class="window">
      <header class="titlebar" data-tauri-drag-region="deep">
        <button class="proj" title="Open another project (⌘O)" onClick={() => void props.shell.showStart()}>
          <img class="proj-icon" src={smallIcon} alt="" />
          <span class="proj-name" title={props.project}>
            {name}
          </span>
          <span class="proj-host" data-link={ws.link().status}>
            <span class="dot" />
            {hostName}
          </span>
          <Icon name="chevron" small />
        </button>
        <span class="spacer" />
        <button class="tbbtn" aria-pressed={sidebar()} title="Sidebar (⌘B)" onClick={toggleSidebar}>
          <Icon name="sidebar" />
        </button>
      </header>
      <div class="window-body">
        <nav class="rail" aria-label="Views">
          <button aria-pressed={sidebar()} title="Files (⌘B)" onClick={toggleSidebar}>
            <FilesRailIcon />
          </button>
        </nav>
        <Show when={sidebar()}>
          <aside class="sidebar pane">
            <div class="pane-head">
              <span class="pane-title" title={props.project}>
                {name}
              </span>
              <span class="spacer" />
              <NewFileMenu onCreate={(kind) => void createFile(kind)} />
              <button class="pane-button" title="Refresh" onClick={() => setTreeKey((k) => k + 1)}>
                <Icon name="refresh" small />
              </button>
            </div>
            <Show when={!missing() && ws.link().status === "connected"}>
              <FileTree ws={ws} root={root()} onOpen={(p) => void open(p)} activePath={active()?.path} refreshKey={treeKey()} />
            </Show>
            <Sessions
              ws={ws}
              hostName={hostName}
              usedBy={(id) => docs().filter((d) => d.kernels.sessions().includes(id)).map((d) => d.name)}
            />
          </aside>
        </Show>
        <main class="editor pane">
          <div class="tabs" role="tablist">
            <For each={docs()}>
              {(doc) => (
                <div
                  class="tab"
                  role="tab"
                  aria-selected={active() === doc}
                  classList={{ active: active() === doc }}
                  title={doc.path}
                  onClick={() => setActive(doc)}
                >
                  <FileIcon name={doc.name} />
                  <span class="tab-name">{doc.name}</span>
                  <button
                    class="tab-close"
                    classList={{ dirty: doc.dirty() }}
                    title={doc.dirty() ? "Unsaved changes" : "Close"}
                    onClick={(e) => {
                      e.stopPropagation();
                      void close(doc);
                    }}
                  >
                    ×
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
        </main>
      </div>
      <footer class="status-bar" data-link={ws.link().status}>
        <span class="status-item" title={linkText(ws)}>
          <span class="link-dot" />
          {ws.link().status === "connected" && ws.host()
            ? `${hostName} · sandhilld ${ws.host()!.daemon_version}`
            : linkText(ws)}
        </span>
        <Show when={active()}>{(doc) => <KernelStates ws={ws} doc={doc()} />}</Show>
        <span class="spacer" />
        <span class="status-item status-path">{active()?.path ?? ""}</span>
      </footer>
      <Dialogs />
    </div>
  );
}

/** The + menu in the sidebar: new Quarto document or notebook. */
function NewFileMenu(props: { onCreate: (kind: "qmd" | "ipynb") => void }) {
  const [open, setOpen] = createSignal(false);
  let wrap!: HTMLSpanElement;
  const close = (e: MouseEvent) => {
    if (!wrap.contains(e.target as Node)) setOpen(false);
  };
  const escape = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
  document.addEventListener("mousedown", close);
  document.addEventListener("keydown", escape);
  onCleanup(() => {
    document.removeEventListener("mousedown", close);
    document.removeEventListener("keydown", escape);
  });
  const pick = (kind: "qmd" | "ipynb") => {
    setOpen(false);
    props.onCreate(kind);
  };
  return (
    <span class="menu-wrap" ref={wrap}>
      <button class="pane-button" title="New file" aria-expanded={open()} onClick={() => setOpen(!open())}>
        <Icon name="plus" small />
      </button>
      <Show when={open()}>
        <div class="menu" role="menu">
          <button role="menuitem" onClick={() => pick("qmd")}>
            <FileIcon name="Untitled.qmd" />
            Quarto document
          </button>
          <button role="menuitem" onClick={() => pick("ipynb")}>
            <FileIcon name="Untitled.ipynb" />
            Jupyter notebook
          </button>
        </div>
      </Show>
    </span>
  );
}

/**
 * Kernels running for this project, under the file tree. They keep running
 * when files and the app close, so each has a stop button, and the ones no
 * open file uses can be stopped together.
 */
export function Sessions(props: { ws: Workspace; hostName: string; usedBy: (session: string) => string[] }) {
  const [now, setNow] = createSignal(Date.now());
  // Also catches sessions started or stopped elsewhere, and long runs finishing.
  const timer = setInterval(() => {
    setNow(Date.now());
    props.ws.refreshSessions().catch(() => {});
  }, 60_000);
  onCleanup(() => clearInterval(timer));
  const sessions = () => props.ws.projectSessions();
  const kernelName = (kernel: string) =>
    props.ws.kernelspecs().find((k) => k.name === kernel)?.display_name ?? kernel;
  const unused = () => sessions().filter((s) => !props.usedBy(s.id).length && !props.ws.isBusy(s.id));

  const stop = async (s: SessionInfo) => {
    const users = props.usedBy(s.id);
    const detail = [
      props.ws.isBusy(s.id) ? "The code it is running stops too." : "",
      users.length ? `${users.join(", ")} will start a new session when you run code.` : "",
      "Its variables are lost.",
    ];
    const choice = await ask(`Stop ${kernelName(s.kernel)}?`, ["Stop", "Cancel"], detail.filter(Boolean).join(" "));
    if (choice === "Stop") await props.ws.shutdown(s.id).catch((e) => notify(`Could not stop the session: ${message(e)}`));
  };

  const stopUnused = async () => {
    await props.ws.refreshSessions().catch(() => {});
    const list = unused();
    if (!list.length) return;
    const choice = await ask(
      list.length === 1 ? "Stop the session no open file uses?" : `Stop the ${list.length} sessions no open file uses?`,
      ["Stop", "Cancel"],
      "Their variables are lost. Sessions running code are left alone.",
    );
    if (choice !== "Stop") return;
    for (const s of list) {
      await props.ws.shutdown(s.id).catch((e) => notify(`Could not stop a session: ${message(e)}`));
    }
  };

  return (
    <Show when={sessions().length}>
      <div class="sessions">
        <div class="pane-head sub">
          <span class="pane-title">Running on {props.hostName}</span>
          <span class="spacer" />
          <Show when={unused().length}>
            <button class="link stop-unused" onClick={() => void stopUnused()} title="Stop the sessions no open file uses">
              Stop unused
            </button>
          </Show>
        </div>
        <For each={sessions()}>
          {(s) => {
            const users = () => props.usedBy(s.id);
            return (
              <div
                class="session-row"
                classList={{ unused: !users().length }}
                title={`${s.kernel} · ${s.id.slice(0, 6)} · ${s.cwd}\n${users().length ? `Used by ${users().join(", ")}` : "No open file uses it"}`}
              >
                <span class="dot" classList={{ busy: props.ws.isBusy(s.id) }} />
                <span class="session-name">{kernelName(s.kernel)}</span>
                <span class="session-age">{users().length ? "" : "unused · "}{uptime(now() - s.created_ms)}</span>
                <button class="session-stop" title="Stop this session" onClick={() => void stop(s)}>
                  <Icon name="stop" small />
                </button>
              </div>
            );
          }}
        </For>
      </div>
    </Show>
  );
}

/** "R idle", "Python running" for the open document, in the status bar. */
function KernelStates(props: { ws: Workspace; doc: Doc }) {
  const label = (language: string) => (language === "r" ? "R" : language === "python" ? "Python" : language);
  const state = (language: string) => {
    if (props.doc.kernels.isStarting(language)) return "starting";
    const id = props.doc.kernels.session(language);
    if (!id) return undefined;
    return props.ws.isBusy(id) ? "running" : "idle";
  };
  return (
    <For each={props.doc.languages().filter((l) => state(l))}>
      {(language) => (
        <span class="status-item" data-state={state(language)}>
          <span class="kernel-dot" />
          {label(language)} {state(language)}
        </span>
      )}
    </For>
  );
}

/** How long a session has run: "4 min", "5 h", "2 d". */
export function uptime(ms: number): string {
  const min = Math.max(0, Math.floor(ms / 60_000));
  if (min < 60) return `${min} min`;
  const hours = Math.floor(min / 60);
  if (hours < 48) return `${hours} h`;
  return `${Math.floor(hours / 24)} d`;
}

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private mode; the choice just isn't remembered.
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
