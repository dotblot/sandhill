import {
  createEffect,
  createResource,
  createSignal,
  ErrorBoundary,
  For,
  type JSX,
  onCleanup,
  onMount,
  Show,
  untrack,
} from "solid-js";
import { LOCAL_HOST, type Shell } from "../backend";
import {
  CONFLICT_ERROR,
  type DirEntry,
  type FileStat,
  type FileText,
  type ProjectInfo,
  type ProjectState,
  type SessionInfo,
} from "../backend/types";
import { isScript, SessionConsole } from "../console/sessionConsole";
import { type Doc, docKind } from "../docs/doc";
import { NotebookDoc } from "../docs/notebookDoc";
import { QmdDoc } from "../docs/qmdDoc";
import { ScriptDoc } from "../docs/scriptDoc";
import { newNotebook, serializeNotebook } from "../model/ipynb";
import { languageLabel, languageVersion, sessionLabel } from "../model/machine";
import { listProjectFiles } from "../model/projectFiles";
import { documentPath, modelLabel, planUsage, timeLeft } from "../model/agents";
import { TerminalStore } from "../model/terminals";
import { normalizeLanguage, pickKernel, type Workspace } from "../model/workspace";
import { reportPlace, serveAgentEdits } from "./agentEdits";
import { AgentsPane } from "./AgentsPane";
import { ConnectionItem, connState, HostKernelStates } from "./Connection";
import { ask, Dialogs, notify } from "./dialogs";
import { FileIcon, Icon, type IconName, LangIcon, ProviderIcon } from "./icons";
import { KernelBar } from "./KernelBar";
import { hostName, LeftPane, type NewFile } from "./LeftPane";
import { takeLegacyState } from "./legacy";
import { type Command, Palette } from "./Palette";
import { type Pane, PaneButtons } from "./PaneButtons";
import { type ShellApi, ShellTerminal } from "./ShellTerminal";
import { VariablesPane } from "./VariablesPane";

const SAVE_STATE_MS = 300;
const LEFT_KEY = "sandhill.sidebar";
const LEFT_WIDTH_KEY = "sandhill.leftWidth";
const CONSOLE_KEY = "sandhill.console";
const CONSOLE_HEIGHT_KEY = "sandhill.consoleHeight";
const RIGHT_KEY = "sandhill.right";
const RIGHT_WIDTH_KEY = "sandhill.rightWidth";
const RIGHT_WIDTH = 320;
const RIGHT_MIN = 240;
const AGENTS_KEY = "sandhill.agents";
const AGENTS_WIDTH_KEY = "sandhill.agentsWidth";
const AGENTS_WIDTH = 400;
const AGENTS_MIN = 300;
/** How often open documents are checked for changes on disk, as when an agent edits one. */
const DISK_CHECK_MS = 1500;
const LEFT_WIDTH = 260;
const LEFT_MIN = 236;
const CONSOLE_MIN = 90;
const EDITOR_MIN = 120;


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
  /** The machine the status bar shows: the one picked in the left pane. */
  const [statusHost, setStatusHost] = createSignal(ws);
  // Another machine's sessions aren't followed live, so ask for them now and then.
  createEffect(() => {
    const other = statusHost();
    if (other === ws) return;
    const timer = setInterval(() => {
      if (other.link().status === "connected") other.refreshSessions().catch(() => {});
    }, 5000);
    onCleanup(() => clearInterval(timer));
  });
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
  const [rightOn, setRightOn] = createSignal(stored(RIGHT_KEY) === "shown");
  const [rightWidth, setRightWidth] = createSignal(clampRight(Number(stored(RIGHT_WIDTH_KEY)) || RIGHT_WIDTH));
  /** Bumped by the Variables pane's refresh button. */
  const [variablesKey, setVariablesKey] = createSignal(0);
  const [agentsOn, setAgentsOn] = createSignal(stored(AGENTS_KEY) === "shown");
  const [agentsWidth, setAgentsWidth] = createSignal(clampAgents(Number(stored(AGENTS_WIDTH_KEY)) || AGENTS_WIDTH));
  /** The agent session in front in the Agents pane. */
  const [agentTab, setAgentTab] = createSignal<string>();
  const [consoleTab, setConsoleTab] = createSignal<"console" | "terminal">("console");
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
  const toggleRight = () => {
    setRightOn(!rightOn());
    store(RIGHT_KEY, rightOn() ? "shown" : "hidden");
    if (!rightOn() && maximized() === "right") setMaximized(undefined);
  };
  const toggleAgents = () => {
    setAgentsOn(!agentsOn());
    store(AGENTS_KEY, agentsOn() ? "shown" : "hidden");
    if (!agentsOn() && maximized() === "agents") setMaximized(undefined);
  };
  /** Shows an agent session, and brings the first document it is about to the front. */
  const selectAgent = (id: string | undefined) => {
    setAgentTab(id);
    const doc = terminals.list().find((t) => t.id === id)?.options?.documents?.[0];
    if (doc) void open(documentPath(props.project, doc));
  };
  /** Brings an agent session to the front, opening the pane if need be. */
  const showAgent = (id?: string) => {
    if (id) selectAgent(id);
    if (!agentsOn()) toggleAgents();
    setMinimized((list) => list.filter((p) => p !== "agents"));
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

  // Agents and shells the daemon keeps for this project, followed from each connection.
  const terminals = new TerminalStore(ws.backend, props.project);
  let shellApi: ShellApi | undefined;
  createEffect(() => {
    if (ws.link().status !== "connected" || !ws.host()) return;
    terminals.refresh().catch((err) => console.warn("could not list the terminals", err));
  });
  /** Runs a command in the Console's Terminal tab, as installing an agent does. */
  const runInTerminal = (command: string) => {
    if (!consoleOn()) toggleConsole();
    setMinimized((list) => list.filter((p) => p !== "console"));
    setConsoleTab("terminal");
    void shellApi?.run(command);
  };

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

  // The Console shows one session at a time, the one in front, which the
  // Variables pane shows too. It follows the document in front, and the
  // script whose line just ran.
  const consoleSessions = () =>
    ws
      .liveSessions()
      .filter((s) => s.project === props.project)
      .sort((a, b) => a.created_ms - b.created_ms);
  const [foreground, setForeground] = createSignal<string>();
  const foregroundInfo = () => consoleSessions().find((s) => s.id === foreground());
  const consoles = new Map<string, SessionConsole>();
  const consoleFor = (id: string): SessionConsole | undefined => {
    let found = consoles.get(id);
    const info = ws.sessions().find((s) => s.id === id);
    if (!found && info) {
      found = new SessionConsole(ws, id, normalizeLanguage(info.language), {
        openFile: (path) => void open(path),
        shortName: relative,
        onError: (text) => notify(text),
      });
      consoles.set(id, found);
    }
    return found;
  };
  const docSession = (doc: Doc | undefined): string | undefined => {
    for (const language of doc?.languages() ?? []) {
      const id = doc!.kernels.session(language);
      if (id) return id;
    }
    return undefined;
  };
  createEffect(() => {
    const id = docSession(active());
    if (id) setForeground(id);
  });
  // Something live stays in front.
  createEffect(() => {
    const live = consoleSessions();
    const id = foreground();
    if (id && live.some((s) => s.id === id)) return;
    setForeground(docSession(untrack(active)) ?? live[live.length - 1]?.id);
  });
  // The Consoles of sessions that ended go.
  createEffect(() => {
    const live = new Set(consoleSessions().map((s) => s.id));
    for (const [id, gone] of consoles) {
      if (live.has(id)) continue;
      gone.dispose();
      consoles.delete(id);
    }
  });
  // A script's line shows in the Console of its session.
  const seenRuns = new Set<string>();
  onCleanup(
    ws.runs.watchAll((run) => {
      if (seenRuns.has(run.id)) return;
      seenRuns.add(run.id);
      const file = run.cell?.file;
      if (run.phase === "done" || !file || !isScript(file) || !docs().some((d) => d.path === file)) return;
      setForeground(run.session);
      setConsoleTab("console");
    }),
  );
  onCleanup(() => {
    for (const shown of consoles.values()) shown.dispose();
  });
  /** Starts R or Python for the Console, in the project's folder. */
  const startConsole = async (language: string) => {
    const spec = pickKernel(ws.kernelspecs(), language) ?? pickKernel(await ws.refreshKernelspecs(), language);
    if (!spec) return notify(`No ${languageLabel(language)} kernel was found where sandhilld runs.`);
    const info = await ws.startShared(spec.name, props.project);
    setForeground(info.id);
  };
  const [consoleBody, setConsoleBody] = createSignal<HTMLElement>();
  const consoleHint = (
    <div class="console-hint">
      <p>No R or Python session yet. Run a line of a script with ⌘↵, or start one here:</p>
      <p>
        <button class="bar-button" onClick={() => void startConsole("r").catch((e) => notify(message(e)))}>
          <LangIcon language="r" />R
        </button>
        <button class="bar-button" onClick={() => void startConsole("python").catch((e) => notify(message(e)))}>
          <LangIcon language="python" />
          Python
        </button>
      </p>
    </div>
  ) as HTMLElement;
  createEffect(() => {
    const body = consoleBody();
    if (!body) return;
    const id = foreground();
    const shown = id ? untrack(() => consoleFor(id)) : undefined;
    body.replaceChildren(shown ? shown.element : consoleHint);
    shown?.shown();
  });
  const foregroundConsole = () => {
    const id = foreground();
    return id ? consoles.get(id) : undefined;
  };
  /** A path as shown: inside the project, from its folder. */
  function relative(path: string): string {
    return path.startsWith(`${props.project}/`) ? path.slice(props.project.length + 1) : path;
  }

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

  // Agents see where the user is, and their edits to open documents happen here.
  const leaveDocuments = reportPlace(ws, props.project, docs, active);
  serveAgentEdits(ws, props.project, open);

  // Documents follow their files: an agent's edits show up in the editor.
  /** Per document, the file version the person chose to keep their own changes over. */
  const keptOver = new Map<Doc, number>();
  let checking = false;
  async function checkDisk() {
    const list = docs();
    if (checking || !list.length || ws.link().status !== "connected") return;
    checking = true;
    try {
      const stats = await ws.call<FileStat[]>("stat_files", { paths: list.map((d) => d.path) });
      for (const stat of stats) {
        const doc = list.find((d) => d.path === stat.path);
        if (!doc || stat.mtime_ms === undefined || stat.mtime_ms === doc.diskMtime() || !docs().includes(doc)) continue;
        if (doc.dirty()) {
          if (keptOver.get(doc) === stat.mtime_ms) continue;
          const choice = await ask(
            `${doc.name} changed on disk, and you have unsaved changes.`,
            ["Keep mine", "Reload"],
            "Reloading replaces your changes with the version on disk. Keeping yours asks again when you save.",
          );
          if (choice !== "Reload") {
            keptOver.set(doc, stat.mtime_ms);
            continue;
          }
        }
        await reload(doc);
      }
    } catch (err) {
      console.warn("could not check the open documents on disk", err);
    } finally {
      checking = false;
    }
  }
  onMount(() => {
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") void checkDisk();
    }, DISK_CHECK_MS);
    const now = () => void checkDisk();
    window.addEventListener("focus", now);
    onCleanup(() => {
      clearInterval(timer);
      window.removeEventListener("focus", now);
    });
  });

  /** Takes a document's file from disk again, in place if it can. */
  async function reload(doc: Doc) {
    keptOver.delete(doc);
    if (doc.kind !== "ipynb") {
      const file = await ws.call<FileText>("read_file", { path: doc.path });
      if (doc.reload(file)) return;
    }
    const shown = doc.shownOutputs();
    const fresh = await NotebookDoc.open(ws, doc.path, shown);
    const list = docs();
    if (!list.includes(doc)) {
      fresh.dispose();
      return;
    }
    setDocs(list.map((d) => (d === doc ? fresh : d)));
    if (active() === doc) setActive(fresh);
    hosts.get(doc)?.remove();
    hosts.delete(doc);
    doc.dispose();
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
    await leaveDocuments();
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
      { label: agentsOn() ? "Hide agents" : "Show agents", keys: "⌘I", run: toggleAgents },
      { label: rightOn() ? "Hide right pane" : "Show right pane", keys: "⌥⌘B", run: toggleRight },
      {
        label: "Open terminal",
        run: () => {
          if (!consoleOn()) toggleConsole();
          setConsoleTab("terminal");
        },
      },
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
      if (mod && e.altKey && !e.shiftKey && e.code === "KeyB") {
        e.preventDefault();
        toggleRight();
        return;
      }
      if (mod && !e.altKey && !e.shiftKey) {
        const key = e.key.toLowerCase();
        const action =
          key === "b"
            ? toggleLeft
            : key === "j"
              ? toggleConsole
              : key === "i"
                ? toggleAgents
                : key === "k"
                  ? () => setPalette(!palette())
                  : undefined;
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
  /** Dragging the gap beside the Agents pane sets its width. */
  const resizeAgents = (down: PointerEvent) => {
    const start = agentsWidth();
    drag(down, "col-resize", (e) => setAgentsWidth(clampAgents(start + e.clientX - down.clientX)), () =>
      store(AGENTS_WIDTH_KEY, String(agentsWidth())),
    );
  };
  let column!: HTMLElement;
  let consolePane: HTMLDivElement | undefined;
  /** Dragging the gap beside the right pane sets its width. */
  const resizeRight = (down: PointerEvent) => {
    const start = rightWidth();
    drag(down, "col-resize", (e) => setRightWidth(clampRight(start - (e.clientX - down.clientX))), () =>
      store(RIGHT_WIDTH_KEY, String(rightWidth())),
    );
  };
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
    <div class="window" classList={{ "left-hidden": !left(), "agents-shown": agentsOn() }}>
      <Toolbar
        left={left()}
        menu={menu()}
        onLeft={toggleLeft}
        agents={agentsOn()}
        waiting={terminals.waiting().length > 0}
        onAgents={toggleAgents}
        right={rightOn()}
        onRight={toggleRight}
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
            onHost={setStatusHost}
          />
          <div class="split v" title="Drag to resize" onPointerDown={resizeLeft} />
        </Show>
        <Show when={agentsOn()}>
          <ErrorBoundary
            fallback={(err, reset) => (
              <div class="pane agents-pane" style={{ width: `${agentsWidth()}px` }} aria-label="Agents">
                <div class="agent-empty">
                  <b>The Agents pane stopped with an error</b>
                  <p>{err instanceof Error ? err.message : String(err)}</p>
                  <button class="primary" onClick={reset}>
                    Try again
                  </button>
                </div>
              </div>
            )}
          >
            <AgentsPane
              ws={ws}
              store={terminals}
              project={props.project}
              hostLabel={hostLabel}
              docs={docs()}
              active={active()}
              selected={agentTab()}
              onSelect={selectAgent}
              onOpen={open}
              width={agentsWidth()}
              min={isMin("agents")}
              max={maximized() === "agents"}
              onMin={toggleMin}
              onMax={toggleMax}
              onInstall={runInTerminal}
            />
          </ErrorBoundary>
          <div class="split v" title="Drag to resize" onPointerDown={resizeAgents} />
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
            <Show when={active()}>{(doc) => <KernelBar ws={ws} doc={doc()} usedBy={usedBy} onError={notify} />}</Show>
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
                <div
                  class="tab"
                  role="tab"
                  aria-selected={consoleTab() === "console"}
                  classList={{ active: consoleTab() === "console" }}
                  title="The R and Python sessions of this project: what runs in them, and a prompt"
                  onClick={() => setConsoleTab("console")}
                >
                  <span class="tab-name">Console</span>
                </div>
                <div
                  class="tab"
                  role="tab"
                  aria-selected={consoleTab() === "terminal"}
                  classList={{ active: consoleTab() === "terminal" }}
                  title={`A shell on ${hostLabel}, in this project's folder`}
                  onClick={() => setConsoleTab("terminal")}
                >
                  <span class="tab-name">Terminal</span>
                </div>
                <span class="spacer" />
                <Show when={consoleTab() === "console"}>
                  <ConsoleSessions
                    ws={ws}
                    sessions={consoleSessions()}
                    foreground={foreground()}
                    usedBy={usedBy}
                    onPick={setForeground}
                    onClear={() => foregroundConsole()?.clear()}
                  />
                </Show>
                <PaneButtons pane="console" min={isMin("console")} max={maximized() === "console"} onMin={toggleMin} onMax={toggleMax} />
              </div>
              <div class="console" ref={setConsoleBody} hidden={consoleTab() !== "console"} />
              <ShellTerminal
                store={terminals}
                shown={consoleTab() === "terminal" && !isMin("console")}
                openLink={(url) => ws.backend.openLink(url)}
                api={(api) => (shellApi = api)}
              />
            </div>
          </Show>
        </section>
        <Show when={rightOn()}>
          <div class="split v" title="Drag to resize" onPointerDown={resizeRight} />
          <aside
            class="pane right-pane"
            classList={{ min: isMin("right"), max: maximized() === "right" }}
            style={{ width: `${rightWidth()}px` }}
            aria-label="Right pane"
          >
            <div class="tabs" role="tablist" data-tauri-drag-region="deep">
              <div class="tab active" role="tab" aria-selected="true">
                <span class="tab-name">Variables</span>
              </div>
              <span class="spacer" />
              <button class="tab-btn" title="Read the variables again" onClick={() => setVariablesKey((k) => k + 1)}>
                <Icon name="refresh" small />
              </button>
              <PaneButtons pane="right" min={isMin("right")} max={maximized() === "right"} onMin={toggleMin} onMax={toggleMax} />
            </div>
            <VariablesPane
              ws={ws}
              session={foregroundInfo()}
              sessions={consoleSessions()}
              usedBy={usedBy}
              onSession={setForeground}
              shown={!isMin("right")}
              refreshKey={variablesKey()}
            />
          </aside>
        </Show>
      </div>
      <footer class="status-bar" data-link={statusHost().link().status}>
        <ConnectionItem ws={statusHost()} />
        {/* Kernel states, hidden while the machine is disconnected. */}
        <Show when={connState(statusHost()) !== "off"}>
          <Show when={statusHost() === ws} fallback={<HostKernelStates ws={statusHost()} />}>
            <Show when={active()}>{(doc) => <KernelStates ws={ws} doc={doc()} />}</Show>
          </Show>
        </Show>
        <span class="spacer" />
        <AgentStatus store={terminals} onShow={showAgent} />
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
 * left pane, Agents, the right pane, ⌘K search and the menu.
 */
function Toolbar(props: {
  left: boolean;
  agents: boolean;
  waiting: boolean;
  right: boolean;
  menu: boolean;
  onLeft: () => void;
  onAgents: () => void;
  onRight: () => void;
  onSearch: () => void;
  onMenu: () => void;
}) {
  return (
    <div class="toolbar" data-tauri-drag-region="deep">
      <ToolButton icon="leftPane" title="Left pane (⌘B)" pressed={props.left} onClick={props.onLeft} />
      <ToolButton
        icon="chat"
        title={props.waiting ? "Agents (⌘I): an agent is waiting for you" : "Agents: Claude Code and Codex (⌘I)"}
        pressed={props.agents}
        onClick={props.onAgents}
        dot={props.waiting}
      />
      <ToolButton icon="rightPane" title="Right pane: Variables (⌥⌘B)" pressed={props.right} onClick={props.onRight} />
      <ToolButton icon="search" title="Go to a file or run a command (⌘K)" onClick={props.onSearch} />
      <ToolButton icon="menu" title="Menu" pressed={props.menu} onClick={props.onMenu} menu />
    </div>
  );
}

function ToolButton(props: {
  icon: IconName;
  title: string;
  pressed?: boolean;
  onClick?: () => void;
  menu?: boolean;
  dot?: boolean;
}) {
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
      <Show when={props.dot}>
        <span class="dot" />
      </Show>
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

/**
 * The Console's sessions in its tab strip, one pill each: the one in front
 * is pressed. Then stopping what runs in it, and clearing its Console.
 */
function ConsoleSessions(props: {
  ws: Workspace;
  sessions: SessionInfo[];
  foreground?: string;
  usedBy: (session: string) => string[];
  onPick: (session: string) => void;
  onClear: () => void;
}) {
  const busy = () => !!props.foreground && props.ws.isBusy(props.foreground);
  const tip = (s: SessionInfo) => {
    const users = props.usedBy(s.id);
    const state = s.status === "starting" ? "starting" : props.ws.isBusy(s.id) ? "running" : "idle";
    return [`${sessionLabel(s)} · ${state}`, s.cwd, users.length ? `Used by ${users.join(", ")}` : ""].filter(Boolean).join("\n");
  };
  return (
    <>
      <span class="console-sessions">
        <For each={props.sessions}>
          {(s) => (
            <button
              class="pill"
              aria-pressed={props.foreground === s.id}
              data-state={props.ws.isBusy(s.id) ? "running" : "idle"}
              title={tip(s)}
              onClick={() => props.onPick(s.id)}
            >
              <LangIcon language={normalizeLanguage(s.language)} />
              {languageVersion(s) || languageLabel(s.language)}
            </button>
          )}
        </For>
      </span>
      <Show when={busy()}>
        <button
          class="bar-button"
          title="Stop what is running (Ctrl-C in the Console)"
          onClick={() => void props.ws.call("interrupt", { session: props.foreground }).catch((e) => notify(message(e)))}
        >
          <Icon name="stop" small />
          Stop
        </button>
      </Show>
      <Show when={props.foreground}>
        <button class="bar-button console-clear" title="Clear the Console (Ctrl-L)" onClick={() => props.onClear()}>
          Clear
        </button>
      </Show>
    </>
  );
}

/**
 * In the status bar: which agent waits for you, then how much of the Claude
 * plan's 5-hour and weekly limits is used, as Claude Code last reported.
 */
function AgentStatus(props: { store: TerminalStore; onShow: (id: string) => void }) {
  const waiting = () => props.store.waiting()[0];
  const usage = () => {
    const reports = props.store
      .agents()
      .filter((t) => t.agent?.status?.rate_limits)
      .sort((a, b) => (b.agent?.ts_ms ?? 0) - (a.agent?.ts_ms ?? 0));
    return reports.length ? { kind: reports[0].kind, limits: planUsage(reports[0].agent?.status) } : undefined;
  };
  return (
    <>
      <Show when={waiting()}>
        {(terminal) => (
          <button class="status-item waiting" title={terminal().agent?.message ?? "Show the agent"} onClick={() => props.onShow(terminal().id)}>
            <span class="dot wait" />
            {modelLabel(terminal())} is waiting for you
          </button>
        )}
      </Show>
      <Show when={usage()?.limits.length}>
        <span class="status-item usage" title="Plan usage, as the agent last reported it">
          <ProviderIcon kind={usage()!.kind} />
          <For each={usage()!.limits}>
            {(limit, i) => (
              <>
                <Show when={i() > 0}>
                  <span class="sep">·</span>
                </Show>
                <span title={limit.resetsAt ? `Resets in ${timeLeft(limit.resetsAt)}` : undefined}>{limit.label}</span>
                <UsageRing percent={limit.percent} />
                <span classList={{ hotx: limit.percent >= 80 }}>{limit.percent}%</span>
                <Show when={limit.resetsAt}>
                  <span class="rs">{timeLeft(limit.resetsAt!)}</span>
                </Show>
              </>
            )}
          </For>
        </span>
      </Show>
    </>
  );
}

/** A 14px ring that fills clockwise from the top, red from 80%. */
function UsageRing(props: { percent: number }) {
  const r = 5.25;
  const circumference = 2 * Math.PI * r;
  const percent = () => Math.min(100, Math.max(0, props.percent));
  return (
    <svg class="uring" classList={{ hot: percent() >= 80 }} viewBox="0 0 14 14" aria-hidden="true">
      <circle class="trk" cx="7" cy="7" r={r} />
      <Show when={percent() > 0}>
        <circle
          class="val"
          cx="7"
          cy="7"
          r={r}
          stroke-dasharray={`${((circumference * percent()) / 100).toFixed(2)} ${circumference.toFixed(2)}`}
          transform="rotate(-90 7 7)"
        />
      </Show>
    </svg>
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

function clampAgents(width: number): number {
  return Math.round(Math.max(AGENTS_MIN, Math.min(width, Math.max(AGENTS_MIN, window.innerWidth * 0.6))));
}

function clampRight(width: number): number {
  return Math.round(Math.max(RIGHT_MIN, Math.min(width, Math.max(RIGHT_MIN, window.innerWidth * 0.5))));
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
