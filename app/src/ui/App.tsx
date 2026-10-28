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
import { EditorView } from "@codemirror/view";
import { LOCAL_HOST, type Nav, type Shell } from "../backend";
import {
  CONFLICT_ERROR,
  type DirEntry,
  type FileStat,
  type FileText,
  type ProjectInfo,
  type ProjectState,
  type SavedExtra,
  type SessionInfo,
  type SessionRuns,
} from "../backend/types";
import { SessionConsole } from "../console/sessionConsole";
import { type Doc, docKind } from "../docs/doc";
import { missingKernel } from "../docs/kernels";
import { NotebookDoc } from "../docs/notebookDoc";
import { QmdDoc } from "../docs/qmdDoc";
import { ScriptDoc } from "../docs/scriptDoc";
import { canEdit, type EditAction, lastEditor, runEdit } from "../editor/editing";
import { type HelpRequest, nameAt } from "../model/help";
import { type NewKind, template, within } from "../model/files";
import { isJobScript, jobNote, jobState } from "../model/jobs";
import { newNotebook, serializeNotebook } from "../model/ipynb";
import { languageLabel, languageVersion, sessionLabel } from "../model/machine";
import { browserOpener, fromRun as pagesOf, isHtmlFile, isPreviewable, localPort } from "../model/pages";
import { listProjectFiles } from "../model/projectFiles";
import { viewPath } from "../model/table";
import { fromNotebook, isScript } from "../model/runs";
import { documentPath, modelLabel, planUsage, timeLeft } from "../model/agents";
import { TerminalStore } from "../model/terminals";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { reportPlace, serveAgentEdits } from "./agentEdits";
import { AgentsPane } from "./AgentsPane";
import { ConnectionItem, connState } from "./Connection";
import { DataExplorer, type TableTab, tableId, tableTab } from "./DataExplorer";
import { type DiffTab, DiffView, diffTab, diffTitle } from "./DiffView";
import { ask, notify } from "./dialogs";
import { type DiffRequest, GitPane } from "./GitPane";
import { HelpPane } from "./HelpPane";
import { FileIcon, Icon, type IconName, LangIcon, ProviderIcon } from "./icons";
import { KernelBar } from "./KernelBar";
import { hostName, LeftPane } from "./LeftPane";
import { createTree, type TreeActions } from "./FileTree";
import { takeLegacyState } from "./legacy";
import { type Command, Palette } from "./Palette";
import { type Pane, PaneButtons } from "./PaneButtons";
import { PackagesPane } from "./PackagesPane";
import { PlotsPane } from "./PlotsPane";
import { type ViewerRequest, ViewerPane } from "./ViewerPane";
import { ProjectSettingsDialog, worthAsking } from "./ProjectSettings";
import { type JobsApi, JobsPane } from "./JobsPane";
import { type ShellApi, ShellTerminal } from "./ShellTerminal";
import { type EditButtons, EditCapsule, TitleBar, ToolbarCapsule } from "./TitleBar";
import { VariablesPane } from "./VariablesPane";
import type { PlaceApi } from "./Workbench";

const SAVE_STATE_MS = 300;
const LEFT_KEY = "sandhill.sidebar";
const LEFT_WIDTH_KEY = "sandhill.leftWidth";
const CONSOLE_KEY = "sandhill.console";
const CONSOLE_HEIGHT_KEY = "sandhill.consoleHeight";
/** Whether notebook and Quarto cells' runs show in the Console, and their plots in Plots: "on", or not. */
const NOTEBOOK_CONSOLE_KEY = "sandhill.notebookConsole";
const NOTEBOOK_PLOTS_KEY = "sandhill.notebookPlots";
/** Runs read to see whether a session that was running already was used from the Console. */
const USED_RUNS = 50;
const RIGHT_KEY = "sandhill.right";
const RIGHT_WIDTH_KEY = "sandhill.rightWidth";
const RIGHT_WIDTH = 320;
const RIGHT_MIN = 240;
const VARIABLES_HEIGHT_KEY = "sandhill.variablesHeight";
const VARIABLES_MIN = 90;
const PLOTS_MIN = 120;
/** The right pane's bottom group of tabs, and the one in front. */
const BOTTOM_TABS = [
  ["plots", "Plots"],
  ["viewer", "Viewer"],
] as const;
type BottomTab = (typeof BOTTOM_TABS)[number][0];
const BOTTOM_TAB_KEY = "sandhill.bottomTab";
/** The right pane's top group of tabs. */
const TOP_TABS = [
  ["variables", "Variables"],
  ["help", "Help"],
  ["packages", "Packages"],
  ["git", "Git"],
] as const;
type TopTab = (typeof TOP_TABS)[number][0];
/** What the refresh button over each top tab says and does. */
const TOP_REFRESH: Record<TopTab, string> = {
  variables: "Read the variables again",
  help: "Search the help again",
  packages: "Read the packages again",
  git: "Read the repository again",
};
/** Beside the documents in the editor: a table in the Data Explorer, or a diff. */
type ExtraTab = TableTab | DiffTab;
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
/** The ☰ menu's width, as styles.css has it. */
const MENU_WIDTH = 250;


/**
 * A project, as its machine's tab shows it: the title bar, the left pane
 * (files, kernels and the machine's resources), Agents, the editor with the
 * Console under it, the right pane and the status bar. Panes float on the
 * window background, 3px apart.
 */
export function App(props: {
  ws: Workspace;
  shell: Shell;
  nav: Nav;
  project: string;
  /** Whether the window shows this project now; a hidden one ignores the keyboard. */
  current?: boolean;
  /** The title bar's machine tabs, which the Workbench draws. */
  machineTabs?: () => JSX.Element;
  /** Hands the Workbench what it needs from this project. */
  report?: (api: PlaceApi) => void;
}) {
  const ws = props.ws;
  const current = () => props.current !== false;
  const root = () => props.project;
  const name = props.project.slice(props.project.lastIndexOf("/") + 1) || props.project;
  const [missing, setMissing] = createSignal<string>();
  const [docs, setDocs] = createSignal<Doc[]>([]);
  const [active, setActive] = createSignal<Doc>();
  /** Tables open in the Data Explorer and diffs, and the one in front of the documents, if any. */
  const [extras, setExtras] = createSignal<ExtraTab[]>([]);
  const [activeExtra, setActiveExtra] = createSignal<ExtraTab>();
  const [treeKey, setTreeKey] = createSignal(0);
  const hostLabel = ws.backend.host === LOCAL_HOST ? "This Mac" : ws.backend.host;
  const opening = new Map<string, Promise<Doc | undefined>>();
  const hosts = new Map<Doc, HTMLElement>();
  let docArea!: HTMLDivElement;
  const [restored, setRestored] = createSignal(false);
  /** Project settings were offered once already, so they aren't offered unasked again. */
  const [setupAsked, setSetupAsked] = createSignal(false);
  /** The Project settings dialog, asked unprompted ("first") or from a menu. */
  const [settingsDialog, setSettingsDialog] = createSignal<"first" | "menu">();
  /** Shown outputs of documents that are not open, as the project state keeps them. */
  let shownOutputs: Record<string, string[]> = {};

  // The layout: which panes show, and their sizes.
  const [left, setLeft] = createSignal(stored(LEFT_KEY) !== "hidden");
  const [leftWidth, setLeftWidth] = createSignal(clampLeft(Number(stored(LEFT_WIDTH_KEY)) || LEFT_WIDTH));
  const [consoleOn, setConsoleOn] = createSignal(stored(CONSOLE_KEY) !== "hidden");
  // Notebooks keep their output under their cells; the user may also want it in the Console and Plots.
  const [notebookConsole, setNotebookConsole] = createSignal(stored(NOTEBOOK_CONSOLE_KEY) === "on");
  const [notebookPlots, setNotebookPlots] = createSignal(stored(NOTEBOOK_PLOTS_KEY) === "on");
  /** The Console's height in px; 0 until dragged, for a share of the column. */
  const [consoleHeight, setConsoleHeight] = createSignal(Number(stored(CONSOLE_HEIGHT_KEY)) || 0);
  // The Console and Plots start minimized. The Console opens when a script's
  // line runs in it, and Plots when code outside notebooks draws a plot.
  const [minimized, setMinimized] = createSignal<Pane[]>(["console", "plots"]);
  const [maximized, setMaximized] = createSignal<Pane>();
  const [rightOn, setRightOn] = createSignal(stored(RIGHT_KEY) === "shown");
  const [rightWidth, setRightWidth] = createSignal(clampRight(Number(stored(RIGHT_WIDTH_KEY)) || RIGHT_WIDTH));
  /** The Variables pane's height in px, over Plots; 0 until dragged, for half the column. */
  const [variablesHeight, setVariablesHeight] = createSignal(Number(stored(VARIABLES_HEIGHT_KEY)) || 0);
  /** Bumped by the Variables pane's refresh button. */
  const [variablesKey, setVariablesKey] = createSignal(0);
  /** The tab in front in the right pane's top group, and its refresh key. */
  const [topTab, setTopTab] = createSignal<TopTab>("variables");
  const [helpKey, setHelpKey] = createSignal(0);
  const [packagesKey, setPackagesKey] = createSignal(0);
  const [gitKey, setGitKey] = createSignal(0);
  const [bottomTab, setBottomTab] = createSignal<BottomTab>(stored(BOTTOM_TAB_KEY) === "viewer" ? "viewer" : "plots");
  /** A page for the Viewer to show; `n` changes each time. */
  const [viewerAsk, setViewerAsk] = createSignal<ViewerRequest & { n: number }>();
  /** A topic for the Help pane to show; `n` changes each time. */
  const [helpAsk, setHelpAsk] = createSignal<HelpRequest & { n: number }>();
  const [agentsOn, setAgentsOn] = createSignal(stored(AGENTS_KEY) === "shown");
  const [agentsWidth, setAgentsWidth] = createSignal(clampAgents(Number(stored(AGENTS_WIDTH_KEY)) || AGENTS_WIDTH));
  /** The agent session in front in the Agents pane. */
  const [agentTab, setAgentTab] = createSignal<string>();
  const [consoleTab, setConsoleTab] = createSignal<"console" | "terminal" | "jobs">("console");
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
  const toggleNotebookConsole = () => {
    setNotebookConsole(!notebookConsole());
    store(NOTEBOOK_CONSOLE_KEY, notebookConsole() ? "on" : "off");
  };
  const toggleNotebookPlots = () => {
    setNotebookPlots(!notebookPlots());
    store(NOTEBOOK_PLOTS_KEY, notebookPlots() ? "on" : "off");
  };
  const toggleRight = () => {
    setRightOn(!rightOn());
    store(RIGHT_KEY, rightOn() ? "shown" : "hidden");
    const max = maximized();
    if (!rightOn() && (max === "variables" || max === "plots")) setMaximized(undefined);
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
  /** Shows help in the right pane's Help tab, opening the pane if need be. */
  const openHelp = (request: HelpRequest) => {
    if (!rightOn()) toggleRight();
    setMinimized((list) => list.filter((p) => p !== "variables"));
    setTopTab("help");
    setHelpAsk({ ...request, n: (helpAsk()?.n ?? 0) + 1 });
  };
  /** Brings Plots or the Viewer to the front, opening the right pane if need be. */
  const showBottom = (tab: BottomTab) => {
    if (!rightOn()) toggleRight();
    setMinimized((list) => list.filter((p) => p !== "plots"));
    setBottomTab(tab);
    store(BOTTOM_TAB_KEY, tab);
  };
  /** Shows a page in the Viewer: an HTML file, an app, or one the Console points to. */
  const showPage = (request: ViewerRequest) => {
    showBottom("viewer");
    setViewerAsk({ ...request, n: (viewerAsk()?.n ?? 0) + 1 });
  };
  /**
   * A link in the Terminal: an app on the server (Streamlit, Dash, Shiny
   * for Python, `quarto preview`) opens in the Viewer, any other page in
   * the browser.
   */
  const openTerminalLink = (url: string) => (localPort(url) !== undefined ? showPage({ url }) : ws.backend.openLink(url));
  const isMin = (pane: Pane) => minimized().includes(pane) && maximized() !== pane;
  /** Whether a tab of the right pane's top group shows. A function: an expression in a prop would be a memo read outside it. */
  const topShown = (tab: TopTab) => current() && !isMin("variables") && topTab() === tab;
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
      setSetupAsked(!!state.setupAsked);
      shownOutputs = state.outputs ?? {};
      for (const path of state.tabs ?? []) await open(path, true);
      const last = docs().find((d) => d.path === state.active);
      if (last && !openedWhileRestoring) setActive(last);
      await restoreExtras(state);
      setRestored(true);
    })();
  });

  /** Brings back the tables and diffs that were open; a table only while its session runs. */
  async function restoreExtras(state: ProjectState) {
    if (!state.extras?.length) return;
    await ws.refreshSessions().catch(() => {});
    const live = new Set(ws.liveSessions().map((s) => s.id));
    const back = state.extras.flatMap((saved): ExtraTab[] => {
      if (saved.kind === "diff") return [diffTab(saved)];
      return live.has(saved.session) ? [tableTab({ id: saved.session, language: saved.language }, saved.path)] : [];
    });
    setExtras((open) => [...back.filter((t) => !open.some((o) => o.id === t.id)), ...open]);
    const front = extras().find((t) => t.id === state.activeExtra);
    if (front && !openedWhileRestoring) setActiveExtra(front);
  }

  // The first time the project opens without saved settings, ask once which R and Python it uses.
  createEffect(() => {
    const setup = ws.setup();
    if (!restored() || !setup || setupAsked() || untrack(settingsDialog)) return;
    if (worthAsking(setup, ws.kernelspecs())) setSettingsDialog("first");
  });
  const showSettings = () => {
    ws.refreshSetup()
      .then(() => setSettingsDialog("menu"))
      .catch((err) => notify(`Could not read the project's settings: ${message(err)}`));
  };
  const closeSettings = () => {
    setSettingsDialog(undefined);
    setSetupAsked(true);
  };

  // Agents, shells and jobs the daemon keeps for this project, followed from each connection.
  const seenKey = `sandhill.jobsSeen:${ws.backend.host}:${props.project}`;
  const terminals = new TerminalStore(ws.backend, props.project, {
    get: () => {
      const ms = Number(stored(seenKey) ?? NaN);
      return Number.isFinite(ms) ? ms : undefined;
    },
    set: (ms) => store(seenKey, String(ms)),
  });
  let shellApi: ShellApi | undefined;
  let jobsApi: JobsApi | undefined;
  /** Jobs that ended since the Jobs tab was last looked at. */
  const [jobNews, setJobNews] = createSignal(0);
  terminals.onJobEnded = (job) => {
    notify(jobNote(job), jobState(job) === "failed" ? "error" : "info");
    if (consoleTab() !== "jobs" || isMin("console") || !consoleOn()) setJobNews((n) => n + 1);
  };
  createEffect(() => consoleTab() === "jobs" && setJobNews(0));
  /** Shows the Jobs tab, at `id`'s log if given. */
  const showJobs = (id?: string) => {
    if (!consoleOn()) toggleConsole();
    setMinimized((list) => list.filter((p) => p !== "console"));
    setConsoleTab("jobs");
    if (id) jobsApi?.show(id);
  };
  const runJob = async (path: string) => {
    try {
      showJobs((await terminals.startJob({ path })).id);
    } catch (err) {
      notify(`Could not run ${path.split("/").pop()} as a job: ${message(err)}`);
    }
  };
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
    const saved = extras().map(
      (tab): SavedExtra =>
        tab.kind === "table"
          ? { kind: "table", session: tab.session, language: tab.language, path: tab.path }
          : { kind: "diff", repo: tab.repo, file: tab.file, staged: tab.staged, sha: tab.sha },
    );
    return {
      tabs: docs().map((d) => d.path),
      active: active()?.path,
      outputs,
      extras: saved,
      activeExtra: activeExtra()?.id,
      setupAsked: setupAsked() || undefined,
    };
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
    const table = activeExtra();
    for (const [d, el] of hosts) el.hidden = d !== doc || !!table;
    if (!doc || table) return;
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

  // The session in front, which Variables, Help and Packages show, follows the
  // document in front and the script whose line just ran. The Console shows
  // it too, unless only notebooks have used it and their runs stay out of the
  // Console; then it keeps the session it showed. Its pills offer every live
  // session all the same, and picking one shows it.
  const consoleSessions = () =>
    ws
      .liveSessions()
      .filter((s) => s.project === props.project)
      .sort((a, b) => a.created_ms - b.created_ms);
  const [foreground, setForeground] = createSignal<string>();
  const foregroundInfo = () => consoleSessions().find((s) => s.id === foreground());
  /** Sessions used from the Console, a script or an agent's run_code, not only notebooks. */
  const [consoleUsed, setConsoleUsed] = createSignal<ReadonlySet<string>>(new Set());
  const markUsed = (id: string) => {
    if (!untrack(consoleUsed).has(id)) setConsoleUsed((used) => new Set(used).add(id));
  };
  /** The sessions the Console follows to without being asked. */
  const consoleList = () => {
    const all = consoleSessions();
    if (notebookConsole()) return all;
    const used = consoleUsed();
    const scripts = new Set(docs().flatMap((d) => (d.kind === "script" ? d.kernels.sessions() : [])));
    return all.filter((s) => used.has(s.id) || scripts.has(s.id));
  };
  const [consolePick, setConsolePick] = createSignal<string>();
  createEffect(() => {
    const id = foreground();
    if (id && consoleList().some((s) => s.id === id)) setConsolePick(id);
  });
  /** The session the Console shows. */
  const consoleSession = () => {
    const list = consoleList();
    const id = consolePick();
    return id && list.some((s) => s.id === id) ? id : list[list.length - 1]?.id;
  };
  const consoles = new Map<string, SessionConsole>();
  const consoleFor = (id: string): SessionConsole | undefined => {
    let found = consoles.get(id);
    const info = ws.sessions().find((s) => s.id === id);
    if (!found && info) {
      found = new SessionConsole(ws, id, normalizeLanguage(info.language), {
        openFile: (path) => void open(path),
        shortName: relative,
        onError: (text) => notify(text),
        openHelp,
        showPage: (key) => showPage({ key }),
      });
      found.setCells(untrack(notebookConsole));
      consoles.set(id, found);
    }
    return found;
  };
  createEffect(() => {
    const cells = notebookConsole();
    for (const shown of consoles.values()) shown.setCells(cells);
  });
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
      if (!run.quiet && !fromNotebook(run.cell)) markUsed(run.session);
      const file = run.cell?.file;
      if (run.phase === "done" || !file || !isScript(file) || !docs().some((d) => d.path === file)) return;
      setForeground(run.session);
      setConsoleTab("console");
      setMinimized((list) => list.filter((p) => p !== "console"));
    }),
  );
  // Sessions that were running before this window opened: used from the Console if their last runs say so.
  const checkedUsed = new Set<string>();
  createEffect(() => {
    if (notebookConsole()) return;
    for (const s of consoleSessions()) {
      if (checkedUsed.has(s.id) || untrack(consoleUsed).has(s.id)) continue;
      checkedUsed.add(s.id);
      ws.call<SessionRuns>("session_runs", { session: s.id, limit: USED_RUNS })
        .then((read) => read.runs.some((r) => !fromNotebook(r.cell)) && markUsed(s.id))
        .catch(() => checkedUsed.delete(s.id));
    }
  });
  onCleanup(() => {
    for (const shown of consoles.values()) shown.dispose();
  });
  /** Shows a session in the Console, notebooks' own included. */
  const pickConsole = (id: string) => {
    markUsed(id);
    setForeground(id);
    setConsolePick(id);
  };
  /** Sessions only notebooks use while their runs stay out of the Console. */
  const notebookOnly = () => {
    const offered = new Set(consoleList().map((s) => s.id));
    return consoleSessions().filter((s) => !offered.has(s.id));
  };
  /**
   * R or Python for the Console: the session notebooks already run in that
   * language, if their runs stay out of it, or a new one in the project's folder.
   */
  const startConsole = async (language: string) => {
    const running = notebookOnly().find((s) => normalizeLanguage(s.language) === language);
    if (running) {
      markUsed(running.id);
      setForeground(running.id);
      setConsolePick(running.id);
      return;
    }
    const spec = ws.pick(language) ?? ws.pick(language, undefined, await ws.refreshKernelspecs());
    if (!spec) return notify(missingKernel(ws, language));
    const info = await ws.startShared(spec.name, props.project);
    markUsed(info.id);
    setForeground(info.id);
  };
  const [consoleBody, setConsoleBody] = createSignal<HTMLElement>();
  const consoleHint = (
    <div class="console-hint">
      <p>
        {notebookOnly().length
          ? "Notebooks' runs stay under their cells, not here. Run a line of a script with ⌘↵, or use R or Python here:"
          : "No R or Python session yet. Run a line of a script with ⌘↵, or start one here:"}
      </p>
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
    const id = consoleSession();
    const shown = id ? untrack(() => consoleFor(id)) : undefined;
    body.replaceChildren(shown ? shown.element : consoleHint);
    shown?.shown();
  });
  const foregroundConsole = () => {
    const id = consoleSession();
    return id ? consoles.get(id) : undefined;
  };
  /** Runs code in a session's Console, where its output shows: examples, installing packages. */
  const runInConsole = (session: string, code: string) => {
    if (!consoleOn()) toggleConsole();
    setMinimized((list) => list.filter((p) => p !== "console"));
    setConsoleTab("console");
    markUsed(session);
    setForeground(session);
    void consoleFor(session)?.run(code);
  };
  /** A path as shown: inside the project, from its folder. */
  function relative(path: string): string {
    return path.startsWith(`${props.project}/`) ? path.slice(props.project.length + 1) : path;
  }

  async function open(path: string, quiet = false): Promise<Doc | undefined> {
    if (!quiet && !restored()) openedWhileRestoring = true;
    if (!quiet) setActiveExtra(undefined);
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

  /** Previews a Quarto document in the Viewer, saved first, as Quarto renders what is on disk. */
  async function previewDoc(doc: Doc) {
    if (doc.dirty() && !(await saveDoc(doc))) return;
    showPage({ preview: doc.path });
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

  /** Opens a session's table in the Data Explorer, or brings it to the front. */
  function openTable(session: string, path: string[]) {
    const info = ws.sessions().find((s) => s.id === session);
    if (!info) return;
    const id = tableId(session, path);
    let table = extras().find((t) => t.id === id);
    if (!table) {
      table = tableTab(info, path);
      setExtras((list) => [...list, table!]);
    }
    setActiveExtra(table);
  }

  // View(df) in one of this window's sessions opens it in the Data Explorer.
  onCleanup(
    ws.onView((session, title) => {
      const info = ws.projectSessions().find((s) => s.id === session);
      if (!info) return;
      const path = viewPath(title, normalizeLanguage(info.language));
      if (!path) {
        notify(`View() can open a variable or a part of one, like df or seu@meta.data, not ${title}. Assign it to a variable first.`, "info");
        return;
      }
      openTable(session, path);
      if (isMin("editor")) toggleMin("editor");
    }),
  );

  /** Opens a file's diff as a tab, or brings it to the front. */
  function openDiff(request: DiffRequest) {
    const tab = diffTab(request);
    const open = extras().find((t) => t.id === tab.id);
    if (!open) setExtras((list) => [...list, tab]);
    setActiveExtra(open ?? tab);
    if (isMin("editor")) toggleMin("editor");
  }

  /** Whether a table or diff can be seen; a plain call, so reading it in a click doesn't make a memo. */
  const extraShown = (tab: ExtraTab) => activeExtra() === tab && !isMin("editor");
  /** What the status bar says about the table or diff in front. */
  const extraStatus = () => {
    const tab = activeExtra();
    return tab?.kind === "table" ? tab.count() : tab ? "" : undefined;
  };

  function closeExtra(tab: ExtraTab) {
    const list = extras();
    const rest = list.filter((t) => t !== tab);
    setExtras(rest);
    if (activeExtra() !== tab) return;
    // The tab to its right comes to the front, or else the document.
    setActiveExtra(rest[Math.min(list.indexOf(tab), rest.length - 1)] ?? undefined);
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

  /** Plots and the Viewer, which share the right pane's bottom place. */
  const bottomTabs = () => (
    <div class="cap tabcap">
      <For each={BOTTOM_TABS}>
        {([tab, label]) => (
          <div
            class="tab"
            role="tab"
            aria-selected={bottomTab() === tab}
            classList={{ active: bottomTab() === tab }}
            onClick={() => showBottom(tab)}
          >
            <span class="tab-name">{label}</span>
          </div>
        )}
      </For>
    </div>
  );
  // A page shown from the Console or a script opens the right pane, which
  // the Viewer is in; the Console only says it is there.
  const pagesSeen = new Map<string, number>();
  onCleanup(
    ws.runs.watchAll((run) => {
      if (run.phase === "done") {
        pagesSeen.delete(run.id);
        return;
      }
      if (rightOn() || run.quiet || !untrack(consoleSessions).some((s) => s.id === run.session)) return;
      const count = pagesOf(run).length;
      if (count > (pagesSeen.get(run.id) ?? 0)) showBottom("viewer");
      pagesSeen.set(run.id, count);
    }),
  );
  // A web page R opens with browseURL() goes to the browser; the Console says so.
  onCleanup(ws.runs.watchAll(browserOpener((url) => ws.backend.openLink(url))));

  // The file tree, kept while the left pane is hidden.
  const tree = createTree(root);
  const treeActions: TreeActions = {
    openable: (path) => !!docKind(path) || isHtmlFile(path),
    openLabel: (path) => (isHtmlFile(path) ? "Open in Viewer" : "Open"),
    open: (path) => (isHtmlFile(path) ? showPage({ path }) : void open(path)),
    runJob: (path) => (isJobScript(path) ? () => void runJob(path) : undefined),
    newText: (name) => (docKind(name) === "ipynb" ? notebookText() : template(name)),
    async release(path) {
      const inside = docs().filter((d) => within(d.path, path));
      const dirty = inside.filter((d) => d.dirty());
      if (dirty.length) return `Save ${dirty.map((d) => d.name).join(", ")} first`;
      const wasActive = active();
      for (const doc of inside) await close(doc);
      // The active document comes back last, so it is active again.
      return inside.sort((a, b) => Number(a === wasActive) - Number(b === wasActive)).map((d) => d.path);
    },
    reopen(paths) {
      void (async () => {
        for (const path of paths) await open(path);
      })();
    },
  };
  /** A new notebook is in the language the project's kernels run, or Python. */
  function notebookText(): string {
    const live = ws.liveSessions().filter((s) => s.project === props.project);
    const language = live.at(-1)?.language ?? (ws.pick("python") ? "python" : "r");
    const joining = ws.sessionToJoin(language, root())?.kernel;
    const kernel = ws.kernelspecs().find((k) => k.name === joining) ?? ws.pick(language);
    return serializeNotebook(newNotebook(kernel));
  }
  /** Starts naming a new file or folder in the tree, showing the left pane if need be. */
  function startNew(kind: NewKind) {
    if (!left()) toggleLeft();
    tree.startNew(kind);
  }

  async function saveAll() {
    for (const doc of docs().filter((d) => d.dirty())) if (!(await saveDoc(doc))) return;
  }

  function command(name: string) {
    if (name === "new-file") return startNew("file");
    if (name === "new-folder") return startNew("folder");
    const table = activeExtra();
    if (table) {
      if (name === "close-tab") closeExtra(table);
      if (name === "save-all") void saveAll();
      return;
    }
    const doc = active();
    if (name === "save-all") return void saveAll();
    if (name === "save" && doc) void saveDoc(doc);
    if (name === "close-tab" && doc) void close(doc);
    if (name === "run-all" && doc) doc.runAll().catch((e) => notify(message(e)));
  }

  /** What ⌘K offers besides files. */
  const commands = (): Command[] => {
    const doc = activeExtra() ? undefined : active();
    const list: Command[] = [];
    if (activeExtra()) list.push({ label: "Close tab", keys: "⌘W", run: () => command("close-tab") });
    if (doc) {
      list.push({ label: "Save", keys: "⌘S", run: () => command("save") });
      list.push({ label: "Save all", keys: "⌥⌘S", run: () => command("save-all") });
      list.push({ label: "Close tab", keys: "⌘W", run: () => command("close-tab") });
      list.push({ label: doc.kind === "script" ? "Run file" : "Run all", keys: "⌥⌘R", run: () => command("run-all") });
      if (isPreviewable(doc.path)) list.push({ label: "Quarto Preview", keys: "⇧⌘K", run: () => void previewDoc(doc) });
      if (isJobScript(doc.path)) list.push({ label: "Run as job", run: () => void runJob(doc.path) });
      for (const language of doc.languages()) {
        const label = languageLabel(language);
        list.push({ label: `${label}: Restart session`, run: () => void doc.kernels.restart(language).catch((e) => notify(message(e))) });
        list.push({ label: `${label}: Interrupt`, run: () => void doc.kernels.interrupt(language).catch((e) => notify(message(e))) });
      }
    }
    list.push(
      { label: "New file…", keys: "⌘N", run: () => startNew("file") },
      { label: "New folder…", keys: "⌥⌘N", run: () => startNew("folder") },
      { label: "New Quarto document…", run: () => startNew("qmd") },
      { label: "New R script…", run: () => startNew("R") },
      { label: "New notebook…", run: () => startNew("ipynb") },
      { label: left() ? "Hide left pane" : "Show left pane", keys: "⌘B", run: toggleLeft },
      { label: consoleOn() ? "Hide console" : "Show console", keys: "⌘J", run: toggleConsole },
      {
        label: notebookConsole() ? "Keep notebook runs out of the Console" : "Show notebook runs in the Console",
        run: toggleNotebookConsole,
      },
      { label: notebookPlots() ? "Keep notebook plots out of Plots" : "Show notebook plots in Plots", run: toggleNotebookPlots },
      { label: agentsOn() ? "Hide agents" : "Show agents", keys: "⌘I", run: toggleAgents },
      { label: rightOn() ? "Hide right pane" : "Show right pane", keys: "⌥⌘B", run: toggleRight },
      {
        label: "Open terminal",
        run: () => {
          if (!consoleOn()) toggleConsole();
          setConsoleTab("terminal");
        },
      },
      { label: "Show jobs", run: () => showJobs() },
      { label: "Refresh files", run: () => setTreeKey((k) => k + 1) },
      { label: "Open project…", keys: "⌘O", run: props.nav.showStart },
      { label: "Connect to server…", keys: "⇧⌘O", run: props.nav.showStart },
    );
    return list;
  };

  props.report?.({ confirmClose, waiting: () => terminals.waiting().length > 0, command });
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!current()) return;
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === "Escape" && menu()) setMenu(false);
      // F1 shows help for the name under the cursor, in the language of the session in front.
      if (e.key === "F1" && !mod) {
        const editor = (document.activeElement as HTMLElement | null)?.closest(".cm-editor");
        const view = editor instanceof HTMLElement ? EditorView.findFromDOM(editor) : null;
        if (!view) return;
        e.preventDefault();
        const { head, from, to } = view.state.selection.main;
        const line = view.state.doc.lineAt(head);
        const language = foregroundInfo()?.language ?? "r";
        const picked = from !== to ? nameAt(view.state.sliceDoc(from, to), 0, language) : nameAt(line.text, head - line.from, language);
        if (picked) openHelp(picked);
        return;
      }
      if (mod && e.altKey && !e.shiftKey && e.code === "KeyB") {
        e.preventDefault();
        toggleRight();
        return;
      }
      // Quarto Preview, as in RStudio and Positron.
      if (mod && e.shiftKey && !e.altKey && e.code === "KeyK") {
        const doc = activeExtra() ? undefined : active();
        if (doc && isPreviewable(doc.path)) {
          e.preventDefault();
          void previewDoc(doc);
          return;
        }
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
      if (!mod || props.shell.onMenu) return;
      if (e.altKey && e.code === "KeyS") command("save-all");
      else if (e.key === "s") command("save");
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
  let rightColumn: HTMLElement | undefined;
  let variablesPane: HTMLDivElement | undefined;
  /** Dragging the gap between Variables and Plots sets their heights. */
  const resizeVariables = (down: PointerEvent) => {
    if (!variablesPane || !rightColumn) return;
    const start = variablesPane.getBoundingClientRect().height;
    const room = rightColumn.getBoundingClientRect().height - PLOTS_MIN;
    drag(down, "row-resize", (e) => setVariablesHeight(Math.round(Math.max(VARIABLES_MIN, Math.min(room, start + e.clientY - down.clientY)))), () =>
      store(VARIABLES_HEIGHT_KEY, String(variablesHeight())),
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

  // The title bar's edit buttons act on the editor last in focus, and save the documents.
  let windowEl!: HTMLDivElement;
  let menuButton: HTMLButtonElement | undefined;
  const editButtons = (): EditButtons => {
    const view = current() ? lastEditor(windowEl) : undefined;
    const can = canEdit(view);
    const act = (action: EditAction) =>
      view && can[action]
        ? () =>
            void runEdit(view, action).catch(() =>
              notify(action === "paste" ? "Sandhill can't read the clipboard here; press ⌘V to paste." : `Could not ${action}.`),
            )
        : undefined;
    const doc = activeExtra() ? undefined : active();
    const unsaved = docs().filter((d) => d.dirty());
    return {
      undo: act("undo"),
      redo: act("redo"),
      cut: act("cut"),
      copy: act("copy"),
      paste: act("paste"),
      save: doc?.dirty() ? () => command("save") : undefined,
      saveAll: unsaved.length ? () => command("save-all") : undefined,
      saveName: doc?.name,
      unsaved: unsaved.map((d) => d.name),
    };
  };

  return (
    <div class="window" ref={windowEl} classList={{ "left-hidden": !left(), "agents-shown": agentsOn() }}>
      <TitleBar>
        <ToolbarCapsule
          left={left()}
          onLeft={toggleLeft}
          agents={agentsOn()}
          waiting={terminals.waiting().length > 0}
          onAgents={toggleAgents}
          right={rightOn()}
          onRight={toggleRight}
          onSearch={() => setPalette(true)}
          menu={menu()}
          onMenu={() => setMenu(!menu())}
          menuRef={(el) => (menuButton = el)}
        />
        {props.machineTabs?.()}
        <EditCapsule buttons={editButtons()} />
      </TitleBar>
      <div class="window-body">
        <Show when={left()}>
          <LeftPane
            ws={ws}
            project={props.project}
            name={name}
            onSettings={showSettings}
            missing={missing()}
            activePath={active()?.path}
            refreshKey={treeKey()}
            width={leftWidth()}
            tree={tree}
            actions={treeActions}
            onRefresh={() => setTreeKey((k) => k + 1)}
            usedBy={usedBy}
            marker={marker}
            nav={props.nav}
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
            <div class="tabs" role="tablist">
              <div class="cap tabcap" hidden={!docs().length && !extras().length}>
              <For each={docs()}>
                {(doc) => (
                  <div
                    class="tab"
                    role="tab"
                    aria-selected={active() === doc && !activeExtra()}
                    classList={{ active: active() === doc && !activeExtra() }}
                    title={doc.path}
                    onClick={() => {
                      setActive(doc);
                      setActiveExtra(undefined);
                    }}
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
              <For each={extras()}>
                {(tab) => (
                  <div
                    class="tab table-tab"
                    role="tab"
                    aria-selected={activeExtra() === tab}
                    classList={{ active: activeExtra() === tab }}
                    title={tab.kind === "table" ? `${tab.code()} in the Data Explorer` : `${tab.file}: ${tab.sha ? `changes in commit ${tab.sha.slice(0, 7)}` : tab.staged ? "staged changes" : "changes not staged"}`}
                    onClick={() => setActiveExtra(tab)}
                  >
                    <Icon name={tab.kind === "table" ? "table" : "branch"} small />
                    <span class="tab-name">{tab.kind === "table" ? tab.code() : diffTitle(tab)}</span>
                    <button
                      class="tab-close"
                      title="Close"
                      onClick={(e) => {
                        e.stopPropagation();
                        closeExtra(tab);
                      }}
                    >
                      <Icon name="x" small />
                    </button>
                  </div>
                )}
              </For>
              </div>
              <span class="spacer" />
              <PaneButtons pane="editor" min={isMin("editor")} max={maximized() === "editor"} onMin={toggleMin} onMax={toggleMax} />
            </div>
            <Show when={!activeExtra() && active()}>{(doc) => (
                <KernelBar
                  ws={ws}
                  doc={doc()}
                  usedBy={usedBy}
                  onError={notify}
                  onPreview={isPreviewable(doc().path) ? () => void previewDoc(doc()) : undefined}
                />
              )}</Show>
            <div class="doc-area" ref={docArea}>
              <div class="table-hosts">
                <For each={extras()}>
                  {(tab) => (
                    <div class="doc-host table-host" hidden={activeExtra() !== tab}>
                      <Show
                        when={tab.kind === "table" ? tab : undefined}
                        fallback={<DiffView ws={ws} tab={tab as DiffTab} shown={extraShown(tab)} onOpen={(path) => void open(path)} onStaged={() => setGitKey((k) => k + 1)} />}
                      >
                        {(table) => (
                          <DataExplorer
                            ws={ws}
                            tab={table()}
                            project={props.project}
                            hostLabel={hostLabel}
                            shortName={relative}
                            shown={extraShown(tab)}
                          />
                        )}
                      </Show>
                    </div>
                  )}
                </For>
              </div>
              <Show when={!active() && !activeExtra()}>
                <Show
                  when={!missing()}
                  fallback={
                    <div class="empty-state">
                      <p>Cannot open this project: {missing()}</p>
                      <p>
                        <button onClick={props.nav.showStart}>Show projects</button>
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
              <div class="tabs" role="tablist">
                <div class="cap tabcap">
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
                <div
                  class="tab"
                  role="tab"
                  aria-selected={consoleTab() === "jobs"}
                  classList={{ active: consoleTab() === "jobs" }}
                  title={`Scripts and pipelines running in the background on ${hostLabel}`}
                  onClick={() => setConsoleTab("jobs")}
                >
                  <span class="tab-name">Jobs</span>
                  <Show when={jobNews() || terminals.jobs().some((j) => j.running)}>
                    <span class="tab-count" classList={{ news: jobNews() > 0 }}>
                      {jobNews() || terminals.jobs().filter((j) => j.running).length}
                    </span>
                  </Show>
                </div>
                </div>
                <span class="spacer" />
                <Show when={consoleTab() === "console"}>
                  <ConsoleSessions
                    ws={ws}
                    sessions={consoleSessions()}
                    foreground={consoleSession()}
                    usedBy={usedBy}
                    onPick={pickConsole}
                    onClear={() => foregroundConsole()?.clear()}
                  />
                </Show>
                <PaneButtons pane="console" min={isMin("console")} max={maximized() === "console"} onMin={toggleMin} onMax={toggleMax} />
              </div>
              <div class="console" ref={setConsoleBody} hidden={consoleTab() !== "console"} />
              <ShellTerminal
                store={terminals}
                shown={consoleTab() === "terminal" && !isMin("console")}
                openLink={openTerminalLink}
                api={(api) => (shellApi = api)}
              />
              <JobsPane
                ws={ws}
                store={terminals}
                shown={consoleTab() === "jobs" && !isMin("console")}
                openLink={openTerminalLink}
                api={(api) => (jobsApi = api)}
              />
            </div>
          </Show>
        </section>
        <Show when={rightOn()}>
          <div class="split v" title="Drag to resize" onPointerDown={resizeRight} />
          <aside class="right-col" ref={rightColumn} style={{ width: `${rightWidth()}px` }} aria-label="Right pane">
            <div
              class="pane variables-pane"
              ref={variablesPane}
              classList={{ min: isMin("variables"), max: maximized() === "variables", fill: isMin("plots") && !isMin("variables"), git: topTab() === "git" }}
              style={variablesHeight() && !isMin("variables") && !isMin("plots") ? { "flex-basis": `${variablesHeight()}px` } : undefined}
            >
              <div class="tabs" role="tablist">
                <div class="cap tabcap">
                <For each={TOP_TABS}>
                  {([tab, label]) => (
                    <div
                      class="tab"
                      role="tab"
                      aria-selected={topTab() === tab}
                      classList={{ active: topTab() === tab }}
                      onClick={() => {
                        setTopTab(tab);
                        if (isMin("variables")) toggleMin("variables");
                      }}
                    >
                      <span class="tab-name">{label}</span>
                    </div>
                  )}
                </For>
                </div>
                <span class="spacer" />
                <button
                  class="tab-btn"
                  title={TOP_REFRESH[topTab()]}
                  onClick={() => ({ variables: setVariablesKey, help: setHelpKey, packages: setPackagesKey, git: setGitKey })[topTab()]((k) => k + 1)}
                >
                  <Icon name="refresh" small />
                </button>
                <PaneButtons pane="variables" min={isMin("variables")} max={maximized() === "variables"} onMin={toggleMin} onMax={toggleMax} />
              </div>
              <div class="top-body" hidden={topTab() !== "variables"}>
                <VariablesPane
                  ws={ws}
                  session={foregroundInfo()}
                  sessions={consoleSessions()}
                  usedBy={usedBy}
                  onSession={setForeground}
                  onView={(path) => foreground() && openTable(foreground()!, path)}
                  shown={topShown("variables")}
                  refreshKey={variablesKey()}
                  folder={props.project}
                />
              </div>
              <div class="top-body" hidden={topTab() !== "help"}>
                <HelpPane
                  ws={ws}
                  session={foregroundInfo()}
                  sessions={consoleSessions()}
                  usedBy={usedBy}
                  onSession={setForeground}
                  request={helpAsk()}
                  onRun={runInConsole}
                  openLink={(url) => ws.backend.openLink(url)}
                  shown={topShown("help")}
                  refreshKey={helpKey()}
                />
              </div>
              <div class="top-body" hidden={topTab() !== "packages"}>
                <PackagesPane
                  ws={ws}
                  session={foregroundInfo()}
                  sessions={consoleSessions()}
                  usedBy={usedBy}
                  onSession={setForeground}
                  onHelp={openHelp}
                  onRun={runInConsole}
                  shown={topShown("packages")}
                  refreshKey={packagesKey()}
                />
              </div>
              <div class="top-body" hidden={topTab() !== "git"}>
                <GitPane
                  ws={ws}
                  project={props.project}
                  shown={topShown("git")}
                  refreshKey={gitKey()}
                  onDiff={openDiff}
                  onOpen={(path) => void open(path)}
                />
              </div>
            </div>
            <div class="split h" classList={{ hidden: !!maximized() }} title="Drag to resize" onPointerDown={resizeVariables} />
            <PlotsPane
              ws={ws}
              project={props.project}
              sessions={consoleSessions()}
              notebooks={notebookPlots()}
              shortName={relative}
              hostLabel={hostLabel}
              classList={{ min: isMin("plots"), max: maximized() === "plots", hidden: bottomTab() !== "plots" }}
              tabs={bottomTabs()}
              buttons={<PaneButtons pane="plots" min={isMin("plots")} max={maximized() === "plots"} onMin={toggleMin} onMax={toggleMax} />}
              onNew={() => showBottom("plots")}
            />
            <ViewerPane
              ws={ws}
              project={props.project}
              sessions={consoleSessions()}
              shortName={relative}
              classList={{ min: isMin("plots"), max: maximized() === "plots", hidden: bottomTab() !== "viewer" }}
              tabs={bottomTabs()}
              buttons={<PaneButtons pane="plots" min={isMin("plots")} max={maximized() === "plots"} onMin={toggleMin} onMax={toggleMax} />}
              request={viewerAsk()}
              onNew={() => showBottom("viewer")}
            />
          </aside>
        </Show>
      </div>
      <footer class="status-bar" data-link={ws.link().status}>
        <ConnectionItem ws={ws} />
        {/* Kernel states, hidden while the machine is disconnected. */}
        <Show when={connState(ws) !== "off"}>
          <Show when={active()}>{(doc) => <KernelStates ws={ws} doc={doc()} />}</Show>
        </Show>
        <span class="spacer" />
        <JobStatus store={terminals} onShow={() => showJobs()} />
        <AgentStatus store={terminals} onShow={showAgent} />
        <span class="status-item">{extraStatus() ?? active()?.cursor() ?? ""}</span>
      </footer>
      <Show when={menu()}>
        <MainMenu
          ws={ws}
          nav={props.nav}
          anchor={menuButton}
          project={props.project}
          consoleOn={consoleOn()}
          onConsole={toggleConsole}
          notebookConsole={notebookConsole()}
          onNotebookConsole={toggleNotebookConsole}
          notebookPlots={notebookPlots()}
          onNotebookPlots={toggleNotebookPlots}
          onSettings={showSettings}
          onClose={() => setMenu(false)}
        />
      </Show>
      <Show when={settingsDialog() && ws.setup()}>
        {(setup) => (
          <ProjectSettingsDialog
            ws={ws}
            setup={setup()}
            name={name}
            first={settingsDialog() === "first"}
            onSaved={() => setTreeKey((k) => k + 1)}
            onClose={closeSettings}
          />
        )}
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
    </div>
  );
}

/** The ☰ menu: other projects and servers, showing the console, and where notebooks' output goes. */
function MainMenu(props: {
  ws: Workspace;
  nav: Nav;
  /** The menu button, which the menu opens under. */
  anchor?: HTMLElement;
  project: string;
  consoleOn: boolean;
  onConsole: () => void;
  notebookConsole: boolean;
  onNotebookConsole: () => void;
  notebookPlots: boolean;
  onNotebookPlots: () => void;
  onSettings: () => void;
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
    if (!menu.contains(target) && !props.anchor?.contains(target)) props.onClose();
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
  // Under its button, inside the window.
  const at = props.anchor?.getBoundingClientRect();
  const left = at ? Math.max(6, Math.min(at.left, window.innerWidth - MENU_WIDTH - 6)) : 6;
  return (
    <div class="main-menu" role="menu" ref={menu} style={{ left: `${left}px`, top: `${at ? at.bottom + 5 : 32}px` }}>
      {item("cloud", "Connect to server…", "⇧⌘O", props.nav.showStart)}
      {item("folder", "Open project…", "⌘O", props.nav.showStart)}
      <Show when={recent()?.length}>
        <div class="mm-sep" />
        <div class="mm-h">Recent on {hostName(props.ws.backend.host)}</div>
        <For each={recent()}>{(p) => item("folder", p.name, "", () => props.nav.openProject(props.ws.backend.host, p.path))}</For>
      </Show>
      <div class="mm-sep" />
      {item("gear", "Project settings…", "", props.onSettings)}
      <div class="mm-sep" />
      {item(props.consoleOn ? "check" : undefined, "Show console", "⌘J", props.onConsole)}
      <div class="mm-sep" />
      <div class="mm-h">Notebooks and Quarto</div>
      {item(props.notebookConsole ? "check" : undefined, "Show their runs in the Console", "", props.onNotebookConsole)}
      {item(props.notebookPlots ? "check" : undefined, "Show their plots in Plots", "", props.onNotebookPlots)}
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
      <span class="console-sessions cap tabcap" hidden={!props.sessions.length}>
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
/** How many jobs run, in the status bar; clicking shows them. */
function JobStatus(props: { store: TerminalStore; onShow: () => void }) {
  const running = () => props.store.jobs().filter((j) => j.running).length;
  return (
    <Show when={running()}>
      <button class="status-item jobs-status" title="Show the jobs running in the background" onClick={() => props.onShow()}>
        <span class="spin" />
        {running() === 1 ? "1 job" : `${running()} jobs`}
      </button>
    </Show>
  );
}

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
