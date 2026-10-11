import { createEffect, createResource, createSignal, For, type JSX, onCleanup, onMount, Show } from "solid-js";
import { LOCAL_HOST, type Shell } from "../backend";
import {
  CONFLICT_ERROR,
  type DirEntry,
  type FileText,
  type ProjectInfo,
  type ProjectState,
} from "../backend/types";
import { type Doc, docKind } from "../docs/doc";
import { NotebookDoc } from "../docs/notebookDoc";
import { QmdDoc } from "../docs/qmdDoc";
import { ScriptDoc } from "../docs/scriptDoc";
import { newNotebook, serializeNotebook } from "../model/ipynb";
import { languageLabel, sessionLabel } from "../model/machine";
import { listProjectFiles } from "../model/projectFiles";
import { pickKernel, type Workspace } from "../model/workspace";
import { ask, Dialogs, notify } from "./dialogs";
import { FileIcon, Icon, type IconName } from "./icons";
import { KernelBar } from "./KernelBar";
import { hostName, LeftPane, type NewFile } from "./LeftPane";
import { takeLegacyState } from "./legacy";
import { linkText } from "./linkText";
import { type Command, Palette } from "./Palette";

const SAVE_STATE_MS = 300;
const LEFT_KEY = "sandhill.sidebar";
const LEFT_WIDTH_KEY = "sandhill.leftWidth";
const CONSOLE_KEY = "sandhill.console";
const CONSOLE_HEIGHT_KEY = "sandhill.consoleHeight";
const LEFT_WIDTH = 260;
const LEFT_MIN = 236;
const CONSOLE_MIN = 90;
const EDITOR_MIN = 120;

type Pane = "editor" | "console";

/**
 * A project window: the toolbar over the left pane (files, kernels and the
 * machine's resources), the editor with the Console under it, and the
 * status bar. Panes float on the window background, 3px apart.
 */
export function App(props: { ws: Workspace; shell: Shell; project: string }) {
  const ws = props.ws;
  const root = () => props.project;
  const name = props.project.slice(props.project.lastIndexOf("/") + 1) || props.project;
  const [missing, setMissing] = createSignal<string>();
  const [docs, setDocs] = createSignal<Doc[]>([]);
  const [active, setActive] = createSignal<Doc>();
  const [treeKey, setTreeKey] = createSignal(0);
  const hostLabel = ws.backend.host === LOCAL_HOST ? "This Mac" : ws.backend.host;
  const opening = new Map<string, Promise<Doc | undefined>>();
  const hosts = new Map<Doc, HTMLElement>();
  let docArea!: HTMLDivElement;
  const [restored, setRestored] = createSignal(false);
  /** Shown outputs of documents that are not open, as the project state keeps them. */
  let shownOutputs: Record<string, string[]> = {};

  // The layout: which panes show, and their sizes.
  const [left, setLeft] = createSignal(stored(LEFT_KEY) !== "hidden");
  const [leftWidth, setLeftWidth] = createSignal(clampLeft(Number(stored(LEFT_WIDTH_KEY)) || LEFT_WIDTH));
  const [consoleOn, setConsoleOn] = createSignal(stored(CONSOLE_KEY) !== "hidden");
  /** The Console's height in px; 0 until dragged, for a share of the column. */
  const [consoleHeight, setConsoleHeight] = createSignal(Number(stored(CONSOLE_HEIGHT_KEY)) || 0);
  const [minimized, setMinimized] = createSignal<Pane[]>([]);
  const [maximized, setMaximized] = createSignal<Pane>();
  const [palette, setPalette] = createSignal(false);
  const [menu, setMenu] = createSignal(false);
  const toggleLeft = () => {
    setLeft(!left());
    store(LEFT_KEY, left() ? "shown" : "hidden");
  };
  const toggleConsole = () => {
    setConsoleOn(!consoleOn());
    store(CONSOLE_KEY, consoleOn() ? "shown" : "hidden");
    if (!consoleOn() && maximized() === "console") setMaximized(undefined);
  };
  const isMin = (pane: Pane) => minimized().includes(pane) && maximized() !== pane;
  const toggleMin = (pane: Pane) => {
    setMaximized((m) => (m === pane ? undefined : m));
    setMinimized((list) => (list.includes(pane) ? list.filter((p) => p !== pane) : [...list.filter((p) => p !== pane), pane]));
  };
  const toggleMax = (pane: Pane) => {
    setMinimized((list) => list.filter((p) => p !== pane));
    setMaximized((m) => (m === pane ? undefined : pane));
  };

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

  // The Console shows the active script's runs, or the last script's while
  // another kind of document is in front.
  const [lastScript, setLastScript] = createSignal<ScriptDoc>();
  createEffect(() => {
    const doc = active();
    if (doc instanceof ScriptDoc) setLastScript(doc);
  });
  const consoleDoc = (): ScriptDoc | undefined => {
    const doc = active();
    if (doc instanceof ScriptDoc) return doc;
    const last = lastScript();
    return last && docs().includes(last) ? last : undefined;
  };
  const [consoleBody, setConsoleBody] = createSignal<HTMLElement>();
  const consoleHint = (
    <p class="console-hint">Run a line of an R or Python script with ⌘↵ and it shows here.</p>
  ) as HTMLElement;
  createEffect(() => {
    const body = consoleBody();
    if (!body) return;
    const doc = consoleDoc();
    body.replaceChildren(doc ? doc.consoleElement : consoleHint);
    doc?.consoleShown();
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
          const kind = docKind(path);
          const doc =
            kind === "ipynb"
              ? await NotebookDoc.open(ws, path, shown)
              : kind === "script"
                ? await ScriptDoc.open(ws, path, shown)
                : await QmdDoc.open(ws, path, shown);
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

  async function createFile(kind: NewFile) {
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
          : kind === "qmd"
            ? "---\ntitle: \"Untitled\"\n---\n\n```{python}\nprint(\"hello\")\n```\n"
            : "";
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

  /** What ⌘K offers besides files. */
  const commands = (): Command[] => {
    const doc = active();
    const list: Command[] = [];
    if (doc) {
      list.push({ label: "Save", keys: "⌘S", run: () => command("save") });
      list.push({ label: "Close tab", keys: "⌘W", run: () => command("close-tab") });
      list.push({ label: doc.kind === "script" ? "Run file" : "Run all", keys: "⌥⌘R", run: () => command("run-all") });
      for (const language of doc.languages()) {
        const label = languageLabel(language);
        list.push({ label: `${label}: Restart session`, run: () => void doc.kernels.restart(language).catch((e) => notify(message(e))) });
        list.push({ label: `${label}: Interrupt`, run: () => void doc.kernels.interrupt(language).catch((e) => notify(message(e))) });
      }
    }
    list.push(
      { label: "New Quarto document", run: () => void createFile("qmd") },
      { label: "New Jupyter notebook", run: () => void createFile("ipynb") },
      { label: "New R script", run: () => void createFile("R") },
      { label: "New Python script", run: () => void createFile("py") },
      { label: left() ? "Hide left pane" : "Show left pane", keys: "⌘B", run: toggleLeft },
      { label: consoleOn() ? "Hide console" : "Show console", keys: "⌘J", run: toggleConsole },
      { label: "Refresh files", run: () => setTreeKey((k) => k + 1) },
      { label: "Open project…", keys: "⌘O", run: () => void props.shell.showStart() },
      { label: "Connect to server…", run: () => void props.shell.showStart() },
    );
    return list;
  };

  onMount(() => {
    ws.backend.onMenu?.(command);
    ws.backend.onCloseRequested?.(confirmClose);
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === "Escape" && menu()) setMenu(false);
      if (mod && !e.altKey && !e.shiftKey) {
        const key = e.key.toLowerCase();
        const action = key === "b" ? toggleLeft : key === "j" ? toggleConsole : key === "k" ? () => setPalette(!palette()) : undefined;
        if (action) {
          e.preventDefault();
          action();
          return;
        }
      }
      // In the Mac app the menu bar owns these shortcuts.
      if (!mod || ws.backend.onMenu) return;
      if (e.key === "s") command("save");
      else if (e.key === "w") command("close-tab");
      else if (e.altKey && (e.key === "r" || e.code === "KeyR")) command("run-all");
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
    window.addEventListener("beforeunload", (e) => {
      if (docs().some((d) => d.dirty())) e.preventDefault();
    });
  });

  /** Dragging the gap beside the left pane sets its width. */
  const resizeLeft = (down: PointerEvent) => {
    const start = leftWidth();
    drag(down, "col-resize", (e) => setLeftWidth(clampLeft(start + e.clientX - down.clientX)), () =>
      store(LEFT_WIDTH_KEY, String(leftWidth())),
    );
  };
  let column!: HTMLElement;
  let consolePane: HTMLDivElement | undefined;
  /** Dragging the gap above the Console sets its height. */
  const resizeConsole = (down: PointerEvent) => {
    if (!consolePane) return;
    const start = consolePane.getBoundingClientRect().height;
    const room = column.getBoundingClientRect().height - EDITOR_MIN;
    drag(down, "row-resize", (e) => setConsoleHeight(Math.round(Math.max(CONSOLE_MIN, Math.min(room, start - (e.clientY - down.clientY))))), () =>
      store(CONSOLE_HEIGHT_KEY, String(consoleHeight())),
    );
  };

  const usedBy = (id: string) => docs().filter((d) => d.kernels.sessions().includes(id)).map((d) => d.name);
  const marker = (path: string) => {
    const doc = docs().find((d) => d.path === path);
    const sessions = doc?.kernels.sessions() ?? [];
    if (!sessions.length) return undefined;
    return sessions.some((s) => ws.isBusy(s)) ? "busy" : "idle";
  };

  return (
    <div class="window" classList={{ "left-hidden": !left() }}>
      <Toolbar
        left={left()}
        menu={menu()}
        onLeft={toggleLeft}
        onSearch={() => setPalette(true)}
        onMenu={() => setMenu(!menu())}
      />
      <div class="window-body">
        <Show when={left()}>
          <LeftPane
            ws={ws}
            shell={props.shell}
            project={props.project}
            name={name}
            missing={missing()}
            activePath={active()?.path}
            refreshKey={treeKey()}
            width={leftWidth()}
            onOpen={(p) => void open(p)}
            onRefresh={() => setTreeKey((k) => k + 1)}
            onCreate={(kind) => void createFile(kind)}
            usedBy={usedBy}
            marker={marker}
          />
          <div class="split v" title="Drag to resize" onPointerDown={resizeLeft} />
        </Show>
        <section class="editor-col" ref={column}>
          <div
            class="pane editor-pane"
            classList={{ min: isMin("editor"), max: maximized() === "editor", hidden: maximized() === "console" }}
          >
            <div class="tabs" role="tablist" data-tauri-drag-region="deep">
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
                      <Icon name="x" small />
                    </button>
                  </div>
                )}
              </For>
              <PaneButtons pane="editor" min={isMin("editor")} max={maximized() === "editor"} onMin={toggleMin} onMax={toggleMax} />
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
                    <p>Open a .qmd, .Rmd, .ipynb, .R or .py file from the left pane, or press ⌘K.</p>
                    <p class="muted">Code runs in sessions that keep going when you close the app.</p>
                  </div>
                </Show>
              </Show>
            </div>
          </div>
          <Show when={consoleOn()}>
            <div class="split h" classList={{ hidden: !!maximized() }} title="Drag to resize" onPointerDown={resizeConsole} />
            <div
              class="pane console-pane"
              ref={consolePane}
              classList={{
                min: isMin("console"),
                max: maximized() === "console",
                hidden: maximized() === "editor",
                fill: isMin("editor") && !isMin("console"),
              }}
              style={consoleHeight() && !isMin("console") && !isMin("editor") ? { "flex-basis": `${consoleHeight()}px` } : undefined}
            >
              <div class="tabs" role="tablist" data-tauri-drag-region="deep">
                <div class="tab active" role="tab" aria-selected="true" title={consoleDoc()?.path}>
                  <span class="tab-name">Console</span>
                  <Show when={consoleDoc() && consoleDoc() !== active()}>
                    <span class="tab-detail">{consoleDoc()!.name}</span>
                  </Show>
                </div>
                <span class="spacer" />
                <Show when={consoleDoc()}>
                  {(doc) => <ConsoleSession ws={ws} doc={doc()} />}
                </Show>
                <Show when={consoleDoc()}>
                  {(doc) => (
                    <button class="bar-button console-clear" title="Clear the console" onClick={() => doc().clearConsole()}>
                      Clear
                    </button>
                  )}
                </Show>
                <PaneButtons pane="console" min={isMin("console")} max={maximized() === "console"} onMin={toggleMin} onMax={toggleMax} />
              </div>
              <div class="console" ref={setConsoleBody} />
            </div>
          </Show>
        </section>
      </div>
      <footer class="status-bar" data-link={ws.link().status}>
        <span class="status-item" title={linkText(ws)}>
          <span class="link-dot" />
          {ws.link().status === "connected" && ws.host()
            ? `${hostLabel} · sandhilld ${ws.host()!.daemon_version}`
            : linkText(ws)}
        </span>
        <Show when={active()}>{(doc) => <KernelStates ws={ws} doc={doc()} />}</Show>
        <span class="spacer" />
        <span class="status-item">{active()?.cursor() ?? ""}</span>
      </footer>
      <Show when={menu()}>
        <MainMenu
          ws={ws}
          shell={props.shell}
          project={props.project}
          consoleOn={consoleOn()}
          onConsole={toggleConsole}
          onClose={() => setMenu(false)}
        />
      </Show>
      <Show when={palette()}>
        <Palette
          root={props.project}
          commands={commands()}
          files={() => listProjectFiles((path) => ws.call<DirEntry[]>("list_dir", { path }), props.project)}
          onOpenFile={(p) => void open(p)}
          onClose={() => setPalette(false)}
        />
      </Show>
      <Dialogs />
    </div>
  );
}

/**
 * The toolbar over the top of the left pane, after the window buttons: the
 * left and right panes, Agents, ⌘K search and the menu.
 */
function Toolbar(props: { left: boolean; menu: boolean; onLeft: () => void; onSearch: () => void; onMenu: () => void }) {
  return (
    <div class="toolbar" data-tauri-drag-region="deep">
      <ToolButton icon="leftPane" title="Left pane (⌘B)" pressed={props.left} onClick={props.onLeft} />
      <ToolButton icon="rightPane" title="Right pane: Variables, Help, Packages, Git and Plots come in a later build" />
      <ToolButton icon="chat" title="Agents: Claude Code, Codex and open models come in a later build" />
      <ToolButton icon="search" title="Go to a file or run a command (⌘K)" onClick={props.onSearch} />
      <ToolButton icon="menu" title="Menu" pressed={props.menu} onClick={props.onMenu} menu />
    </div>
  );
}

function ToolButton(props: { icon: IconName; title: string; pressed?: boolean; onClick?: () => void; menu?: boolean }) {
  return (
    <button
      class="tbi"
      classList={{ "main-menu-button": props.menu }}
      title={props.title}
      aria-pressed={props.onClick && !props.menu ? props.pressed : undefined}
      aria-expanded={props.menu ? props.pressed : undefined}
      aria-disabled={props.onClick ? undefined : "true"}
      onClick={() => props.onClick?.()}
    >
      <Icon name={props.icon} />
    </button>
  );
}

/** The ☰ menu: other projects and servers, and showing the console. */
function MainMenu(props: {
  ws: Workspace;
  shell: Shell;
  project: string;
  consoleOn: boolean;
  onConsole: () => void;
  onClose: () => void;
}) {
  const [recent] = createResource(() =>
    props.ws
      .call<ProjectInfo[]>("list_recent_projects")
      .then((list) => list.filter((p) => p.exists && p.path !== props.project).slice(0, 5))
      .catch(() => [] as ProjectInfo[]),
  );
  let menu!: HTMLDivElement;
  const outside = (e: MouseEvent) => {
    const target = e.target as Element;
    if (!menu.contains(target) && !target.closest(".main-menu-button")) props.onClose();
  };
  document.addEventListener("mousedown", outside);
  onCleanup(() => document.removeEventListener("mousedown", outside));
  const item = (icon: IconName | undefined, label: JSX.Element, keys: string, run: () => void) => (
    <button
      class="mm-i"
      role="menuitem"
      onClick={() => {
        props.onClose();
        run();
      }}
    >
      {icon ? <Icon name={icon} small /> : <span class="ico sm" />}
      <span>{label}</span>
      <kbd>{keys}</kbd>
    </button>
  );
  return (
    <div class="main-menu" role="menu" ref={menu}>
      {item("cloud", "Connect to server…", "", () => void props.shell.showStart())}
      {item("folder", "Open project…", "⌘O", () => void props.shell.showStart())}
      <Show when={recent()?.length}>
        <div class="mm-sep" />
        <div class="mm-h">Recent on {hostName(props.ws.backend.host)}</div>
        <For each={recent()}>
          {(p) => item("folder", p.name, "", () => void props.shell.openProject(props.ws.backend.host, p.path))}
        </For>
      </Show>
      <div class="mm-sep" />
      {item(props.consoleOn ? "check" : undefined, "Show console", "⌘J", props.onConsole)}
    </div>
  );
}

/** Minimize and maximize, at the right end of a pane's tab strip. */
function PaneButtons(props: { pane: Pane; min: boolean; max: boolean; onMin: (p: Pane) => void; onMax: (p: Pane) => void }) {
  return (
    <span class="pwin">
      <button title={props.min ? "Restore" : "Minimize"} onClick={() => props.onMin(props.pane)}>
        <Icon name={props.min ? "restore" : "minimize"} small />
      </button>
      <button title={props.max ? "Restore" : "Maximize"} onClick={() => props.onMax(props.pane)}>
        <Icon name={props.max ? "restore" : "maximize"} small />
      </button>
    </span>
  );
}

/** The session a script's console runs in, as a pill in the Console's tab strip. */
function ConsoleSession(props: { ws: Workspace; doc: ScriptDoc }) {
  const id = () => props.doc.kernels.session(props.doc.language);
  const info = () => props.ws.sessions().find((s) => s.id === id());
  const state = () =>
    props.doc.kernels.isStarting(props.doc.language) ? "starting" : !id() ? "none" : props.ws.isBusy(id()!) ? "busy" : "idle";
  return (
    <span class="pill" data-state={state()} title={info() ? `${info()!.kernel} · ${info()!.id.slice(0, 6)}` : "Starts when you run code"}>
      <span class="kernel-dot" />
      {info() ? sessionLabel(info()!) : languageLabel(props.doc.language)}
    </span>
  );
}

/** "R idle", "Python running" for the open document, in the status bar. */
function KernelStates(props: { ws: Workspace; doc: Doc }) {
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
          <Show when={state(language) === "idle"} fallback={<span class="spin" />}>
            <Icon name="sessions" small />
          </Show>
          {languageLabel(language)} {state(language)}
        </span>
      )}
    </For>
  );
}

/** Follows the pointer from `down` until it is released, with `cursor` over the whole window. */
function drag(down: PointerEvent, cursor: string, move: (e: PointerEvent) => void, done: () => void) {
  down.preventDefault();
  const target = down.currentTarget as HTMLElement;
  target.setPointerCapture?.(down.pointerId);
  target.classList.add("dragging");
  document.documentElement.style.cursor = cursor;
  document.documentElement.classList.add("resizing");
  const up = () => {
    target.removeEventListener("pointermove", move);
    target.removeEventListener("pointerup", up);
    target.removeEventListener("pointercancel", up);
    target.classList.remove("dragging");
    document.documentElement.style.cursor = "";
    document.documentElement.classList.remove("resizing");
    done();
  };
  target.addEventListener("pointermove", move);
  target.addEventListener("pointerup", up);
  target.addEventListener("pointercancel", up);
}

function clampLeft(width: number): number {
  return Math.round(Math.max(LEFT_MIN, Math.min(width, Math.max(LEFT_MIN, window.innerWidth * 0.5))));
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
