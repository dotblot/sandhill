// The left pane: the project's name, which switches to another project on
// the same machine, then its files, the kernels running there and how busy
// the machine is. It sits directly on the window background, under the
// title bar, and shows only the machine picked there.

import { createResource, createSignal, For, type JSX, onCleanup, Show } from "solid-js";
import { LOCAL_HOST, type Nav } from "../backend";
import type { ProjectInfo, SessionInfo, SystemStats } from "../backend/types";
import { formatBytes, languageLabel, languageVersion, percentOf, sessionLabel, watchMachine } from "../model/machine";
import { shortPath } from "../model/places";
import type { Workspace } from "../model/workspace";
import { notify } from "./dialogs";
import { FileTree } from "./FileTree";
import { FileIcon, Icon, type IconName, LangIcon } from "./icons";
import { SessionMenu } from "./KernelBar";
import { linkText } from "./linkText";

export type NewFile = "qmd" | "ipynb" | "R" | "py";

export function LeftPane(props: {
  ws: Workspace;
  nav: Nav;
  project: string;
  name: string;
  /** Why the project can't be opened, if it can't. */
  missing?: string;
  activePath?: string;
  refreshKey: number;
  width: number;
  onOpen: (path: string) => void;
  onRefresh: () => void;
  onCreate: (kind: NewFile) => void;
  /** Names of the open documents using a session. */
  usedBy: (session: string) => string[];
  /** State of the session an open file runs in, for a dot in the tree. */
  marker: (path: string) => "idle" | "busy" | undefined;
}) {
  const ws = props.ws;
  const stats = watchMachine(ws, props.project);
  const connected = () => ws.link().status === "connected";
  return (
    <aside class="left-pane" style={{ width: `${props.width}px` }}>
      <div class="pane-head proj-head">
        <ProjectButton ws={ws} nav={props.nav} project={props.project} name={props.name} />
        <span class="spacer" />
        <NewFileMenu onCreate={props.onCreate} />
        <button class="pane-button" title="Refresh" onClick={() => props.onRefresh()}>
          <Icon name="refresh" small />
        </button>
      </div>
      <Show
        when={!props.missing && connected()}
        fallback={<p class="pane-note grow">{props.missing ? `Cannot open this project: ${props.missing}` : linkText(ws)}</p>}
      >
        <FileTree
          ws={ws}
          root={props.project}
          onOpen={props.onOpen}
          activePath={props.activePath}
          refreshKey={props.refreshKey}
          marker={props.marker}
        />
      </Show>
      <KernelRows
        ws={ws}
        sessions={ws.projectSessions()}
        stats={stats()}
        describe={(s) => props.usedBy(s.id).join(", ") || "not used by an open file"}
        unused={(s) => !props.usedBy(s.id).length}
        usedBy={(s) => props.usedBy(s.id)}
      />
      <Resources stats={stats()} host={ws.backend.host} sessions={ws.liveSessions()} />
    </aside>
  );
}

/**
 * The project's name, with a folder icon and ⌄: it lists the machine's
 * projects, and picking one switches the window to it, like picking a
 * machine. Opening or making another goes to the Start screen.
 */
function ProjectButton(props: { ws: Workspace; nav: Nav; project: string; name: string }) {
  const [open, setOpen] = createSignal(false);
  const [recent, { refetch }] = createResource(open, () =>
    props.ws.call<ProjectInfo[]>("list_recent_projects").catch(() => [] as ProjectInfo[]),
  );
  let button!: HTMLButtonElement;
  let pop: HTMLDivElement | undefined;
  const close = (e: MouseEvent) => {
    if (!pop?.contains(e.target as Node) && !button.contains(e.target as Node)) setOpen(false);
  };
  const escape = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
  document.addEventListener("mousedown", close);
  document.addEventListener("keydown", escape);
  onCleanup(() => {
    document.removeEventListener("mousedown", close);
    document.removeEventListener("keydown", escape);
  });
  const home = () => props.ws.host()?.home;
  /** The open project first, then the others most recent first. */
  const projects = () => {
    const list = (recent() ?? []).filter((p) => p.exists || p.path === props.project);
    const here = list.find((p) => p.path === props.project) ?? { path: props.project, name: props.name, opened_ms: Date.now(), exists: true, state: null };
    return [here, ...list.filter((p) => p.path !== props.project)];
  };
  const kernels = (path: string) => props.ws.liveSessions().filter((s) => s.project === path).length;
  const when = (p: ProjectInfo) => {
    const n = kernels(p.path);
    if (n) return n === 1 ? "1 kernel" : `${n} kernels`;
    const date = new Date(p.opened_ms);
    return date.toDateString() === new Date().toDateString()
      ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
      : date.toLocaleDateString([], { month: "short", day: "numeric" });
  };
  const go = (run: () => void) => {
    setOpen(false);
    run();
  };
  const at = () => button.getBoundingClientRect();
  return (
    <>
      <button
        class="projbtn"
        ref={button}
        title={`${shortPath(props.project, home())} · Click to switch project on ${hostName(props.ws.backend.host)}`}
        aria-expanded={open()}
        onClick={() => {
          if (!open()) void refetch();
          setOpen(!open());
        }}
      >
        <Icon name="folder" />
        <span>{props.name}</span>
        <Icon name="chevron" />
      </button>
      <Show when={open()}>
        <div class="kpop projpop" role="menu" aria-label="Projects" ref={pop} style={{ left: `${at().left}px`, top: `${at().bottom + 4}px` }}>
          <div class="mm-h">Projects on {hostName(props.ws.backend.host)}</div>
          <For each={projects()}>
            {(p) => (
              <button
                class="mm-i pj"
                classList={{ on: p.path === props.project }}
                role="menuitem"
                title={p.path}
                onClick={() => go(() => p.path !== props.project && props.nav.openProject(props.ws.backend.host, p.path))}
              >
                <Icon name="folder" small />
                <span>
                  <b>{p.name}</b>
                  <small>{shortPath(p.path, home())}</small>
                </span>
                <kbd>{p.path === props.project ? <Icon name="check" small /> : when(p)}</kbd>
              </button>
            )}
          </For>
          <div class="mm-sep" />
          <button class="mm-i" role="menuitem" onClick={() => go(props.nav.showStart)}>
            <Icon name="folder" small />
            <span>Open project…</span>
            <kbd>⌘O</kbd>
          </button>
          <button class="mm-i" role="menuitem" onClick={() => go(props.nav.showStart)}>
            <Icon name="plus" small />
            <span>New project…</span>
            <kbd>⇧⌘N</kbd>
          </button>
        </div>
      </Show>
    </>
  );
}

/**
 * Running kernels, one 11px row each: language, version, what uses it and
 * its memory. The trash button turns a row into an inline confirmation.
 */
export function KernelRows(props: {
  ws: Workspace;
  sessions: SessionInfo[];
  stats?: SystemStats;
  /** What the session is for: the files using it, or its project. */
  describe: (session: SessionInfo) => string;
  /** Sessions no open file uses, which "Shut down unused" stops together. */
  unused?: (session: SessionInfo) => boolean;
  /** The open documents using a session, for its details. */
  usedBy?: (session: SessionInfo) => string[];
}) {
  /** The session id whose row asks for confirmation, or UNUSED. */
  const [asking, setAsking] = createSignal<string>();
  /** The row whose details are open. */
  const [details, setDetails] = createSignal<{ session: SessionInfo; row: HTMLElement }>();
  const open = sectionOpen("kernels");
  const memory = (id: string) => props.stats?.sessions.find((m) => m.session === id)?.rss;
  const unusedIdle = () => (props.unused ? props.sessions.filter((s) => props.unused!(s) && !props.ws.isBusy(s.id)) : []);

  const stop = async (sessions: SessionInfo[]) => {
    setAsking(undefined);
    for (const s of sessions) {
      await props.ws.shutdown(s.id).catch((e) => notify(`Could not shut down ${sessionLabel(s)}: ${message(e)}`));
    }
  };

  return (
    <div class="kernels">
      <SectionHead name="Kernels" section="kernels" summary={props.sessions.length ? String(props.sessions.length) : undefined} />
      <Show when={open()}>
        <For each={props.sessions} fallback={<div class="krow dim">
              <span class="kn">No kernels running</span>
            </div>}>
          {(s) => (
            <Show
              when={asking() === s.id}
              fallback={
                <div
                  class="krow"
                  classList={{ unused: props.unused?.(s), busy: props.ws.isBusy(s.id), on: details()?.session.id === s.id }}
                  role="button"
                  tabindex="0"
                  aria-haspopup="menu"
                  title={`${sessionLabel(s)} · ${props.ws.isBusy(s.id) ? "busy" : "idle"}. Click for details`}
                  onClick={(e) => setDetails(details()?.session.id === s.id ? undefined : { session: s, row: e.currentTarget })}
                  onKeyDown={(e) => {
                    if (e.key !== "Enter" && e.key !== " ") return;
                    e.preventDefault();
                    e.currentTarget.click();
                  }}
                >
                  <span class="krow-icon">
                    <LangIcon language={s.language} />
                  </span>
                  {/* The icon says the language; the row names only the version. */}
                  <span class="kn">
                    {languageVersion(s) || languageLabel(s.language)}
                    <small>{props.describe(s)}</small>
                  </span>
                  <span class="km">{memory(s.id) === undefined ? "" : formatBytes(memory(s.id)!)}</span>
                  <button
                    class="kx"
                    title="Shut down this session"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDetails(undefined);
                      setAsking(s.id);
                    }}
                  >
                    <Icon name="trash" small />
                  </button>
                </div>
              }
            >
              <div class="krow ask" title={props.ws.isBusy(s.id) ? "Its running code stops and its variables are lost" : "Its variables are lost"}>
                <span class="krow-icon">
                  <LangIcon language={s.language} />
                </span>
                <span class="kn">Shut down {sessionLabel(s)}?</span>
                <button class="kbtn" onClick={() => setAsking(undefined)}>
                  Cancel
                </button>
                <button class="kbtn danger" onClick={() => void stop([s])}>
                  Shut down
                </button>
              </div>
            </Show>
          )}
        </For>
        <Show when={unusedIdle().length}>
          {(n) => (
            <Show
              when={asking() === UNUSED}
              fallback={
                <button class="krow kunused" onClick={() => setAsking(UNUSED)} title="Shut down the sessions no open file uses">
                  <span class="krow-icon">
                    <Icon name="trash" small />
                  </span>
                  <span class="kn">Shut down {n() === 1 ? "the unused session" : `${n()} unused sessions`}</span>
                </button>
              }
            >
              <div class="krow ask" title="Their variables are lost. Sessions running code are left alone.">
                <span class="krow-icon">
                  <Icon name="trash" small />
                </span>
                <span class="kn">Shut down {n() === 1 ? "1 session" : `${n()} sessions`}?</span>
                <button class="kbtn" onClick={() => setAsking(undefined)}>
                  Cancel
                </button>
                <button class="kbtn danger" onClick={() => void stop(unusedIdle())}>
                  Shut down
                </button>
              </div>
            </Show>
          )}
        </Show>
      </Show>
      {/* Keyed: another row's details are a new pop-up beside that row. */}
      <Show when={details()} keyed>
        {(d) => (
          <SessionMenu
            ws={props.ws}
            session={d.session}
            usedBy={props.usedBy?.(d.session) ?? []}
            anchor={d.row}
            onClose={() => setDetails(undefined)}
            onShutDown={() => setAsking(d.session.id)}
          />
        )}
      </Show>
    </div>
  );
}

const UNUSED = "unused";

/** Which of the left pane's sections are collapsed, remembered across windows. */
const SECTIONS_KEY = "sandhill.collapsedSections";
const [collapsed, setCollapsed] = createSignal<string[]>(readCollapsed());

function readCollapsed(): string[] {
  try {
    const list = JSON.parse(localStorage.getItem(SECTIONS_KEY) ?? "[]");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

function sectionOpen(section: string) {
  return () => !collapsed().includes(section);
}

function toggleSection(section: string) {
  const list = collapsed().includes(section) ? collapsed().filter((s) => s !== section) : [...collapsed(), section];
  setCollapsed(list);
  try {
    localStorage.setItem(SECTIONS_KEY, JSON.stringify(list));
  } catch {
    // Private mode; the choice lasts while the window is open.
  }
}

/** A section's header row: click to collapse or expand it. Collapsed, it shows `summary`. */
function SectionHead(props: { name: string; section: string; summary?: JSX.Element }) {
  const open = sectionOpen(props.section);
  return (
    <button class="sec-h" aria-expanded={open()} onClick={() => toggleSection(props.section)}>
      <span class="chev" classList={{ open: open() }}>
        <Icon name="chevronRight" small />
      </span>
      {props.name}
      <Show when={!open() && props.summary}>
        <span class="sec-n">{props.summary}</span>
      </Show>
    </button>
  );
}

interface Resource {
  key: string;
  icon: IconName;
  label: string;
  name: string;
  percent: number;
  main: string;
  detail: string;
}

/** What the machine's CPU, memory, GPUs and disk are doing, as rows with bars. */
export function resourceRows(stats: SystemStats, sessions: SessionInfo[]): Resource[] {
  const rows: Resource[] = [];
  const { cpu, memory, gpus, disk } = stats;
  rows.push({
    key: "cpu",
    icon: "cpu",
    label: "CPU",
    name: "CPU",
    percent: Math.round(cpu.percent),
    main: `${cpu.model ? `${cpu.model} · ` : ""}${cpu.cores} cores`,
    detail: `Load ${cpu.load.toFixed(1)}`,
  });
  const kernels = stats.sessions
    .map((m) => {
      const s = sessions.find((x) => x.id === m.session);
      // Each kernel stays on one line when the list wraps.
      return s ? `${sessionLabel(s)} ${formatBytes(m.rss)}`.replace(/ /g, "\u00a0") : "";
    })
    .filter(Boolean);
  rows.push({
    key: "mem",
    icon: "memory",
    label: "MEM",
    name: "Memory",
    percent: percentOf(memory.used, memory.total),
    main: `${formatBytes(memory.used)} of ${formatBytes(memory.total)} used`,
    detail: kernels.length ? `Kernels: ${kernels.join(" · ")}` : "No kernels running",
  });
  if (gpus.length) {
    const busiest = gpus.reduce((a, b) => (b.percent > a.percent ? b : a));
    rows.push({
      key: "gpu",
      icon: "gpu",
      label: "GPU",
      name: gpus.length > 1 ? `GPU (${gpus.length})` : "GPU",
      percent: Math.round(busiest.percent),
      main: gpus.length > 1 ? `${gpus.length} × ${busiest.name}` : busiest.name,
      detail: gpus
        .map((g) => `${Math.round(g.percent)}% busy · ${formatBytes(g.memory_used)} of ${formatBytes(g.memory_total)}`)
        .join("\n"),
    });
  }
  if (disk) {
    rows.push({
      key: "disk",
      icon: "disk",
      label: "Disk",
      name: "Disk",
      percent: percentOf(disk.used, disk.used + disk.available),
      main: `${formatBytes(disk.used)} of ${formatBytes(disk.total)} used`,
      detail: `${formatBytes(disk.available)} free for ${disk.path}`,
    });
  }
  return rows;
}

/** 11px items with a bar each, two to a row; clicking one shows its percent and details beside the pane. */
function Resources(props: { stats?: SystemStats; host: string; sessions: SessionInfo[] }) {
  const [open, setOpen] = createSignal<{ key: string; left: number; bottom: number }>();
  const rows = () => (props.stats ? resourceRows(props.stats, props.sessions) : []);
  const shown = () => rows().find((r) => r.key === open()?.key);
  const expanded = sectionOpen("resources");
  const hot = () => rows().filter((r) => r.percent >= 80).map((r) => r.label);
  const close = (e: Event) => {
    if (!(e.target as Element).closest?.(".res-pop, .rrow")) setOpen(undefined);
  };
  const escape = (e: KeyboardEvent) => e.key === "Escape" && setOpen(undefined);
  document.addEventListener("mousedown", close);
  document.addEventListener("keydown", escape);
  onCleanup(() => {
    document.removeEventListener("mousedown", close);
    document.removeEventListener("keydown", escape);
  });
  return (
    <Show when={rows().length}>
      <div class="resources">
        <SectionHead
          name="Resources"
          section="resources"
          summary={
            hot().length ? (
              <>
                <span class="hotdot" />
                {hot().join(", ")} high
              </>
            ) : undefined
          }
        />
        <For each={expanded() ? rows() : []}>
          {(r) => (
            <button
              class="rrow"
              classList={{ on: open()?.key === r.key }}
              title={`${r.name}: ${r.percent}%`}
              onClick={(e) => {
                if (open()?.key === r.key) return setOpen(undefined);
                const box = e.currentTarget.getBoundingClientRect();
                setOpen({ key: r.key, left: box.right + 8, bottom: Math.max(8, window.innerHeight - box.bottom - 4) });
              }}
            >
              <Icon name={r.icon} />
              <span class="rl">{r.label}</span>
              <Bar percent={r.percent} />
            </button>
          )}
        </For>
        <Show when={expanded() && shown()}>
          {(r) => (
            <div class="res-pop" role="dialog" style={{ left: `${open()!.left}px`, bottom: `${open()!.bottom}px` }}>
              <div class="rp-head">
                <b>{r().name}</b>
                <span>{hostName(props.host)}</span>
              </div>
              <div class="rp-big">
                <b>{r().percent}%</b>
                <Bar percent={r().percent} />
              </div>
              <div class="rp-main">{r().main}</div>
              <div class="rp-detail">{r().detail}</div>
            </div>
          )}
        </Show>
      </div>
    </Show>
  );
}

function Bar(props: { percent: number }): JSX.Element {
  return (
    <span class="rb">
      <i classList={{ hot: props.percent >= 80 }} style={{ width: `${Math.min(100, Math.max(0, props.percent))}%` }} />
    </span>
  );
}

/** The + button: a new Quarto document, notebook or script. */
function NewFileMenu(props: { onCreate: (kind: NewFile) => void }) {
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
  const pick = (kind: NewFile) => {
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
          <button role="menuitem" onClick={() => pick("R")}>
            <FileIcon name="Untitled.R" />
            R script
          </button>
          <button role="menuitem" onClick={() => pick("py")}>
            <FileIcon name="Untitled.py" />
            Python script
          </button>
        </div>
      </Show>
    </span>
  );
}

export function hostName(host: string): string {
  return host === LOCAL_HOST ? "this Mac" : host;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
