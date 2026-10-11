import { createResource, createSignal, For, onMount, Show } from "solid-js";
import { LOCAL_HOST, type Shell } from "../backend";
import type { DirEntry, ProjectInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import appIcon from "../../src-tauri/icons/icon.png";
import { Dialogs, notify } from "./dialogs";
import { FileIcon, Icon } from "./icons";
import { previousFolder as legacyFolder } from "./legacy";
import { linkText } from "./linkText";

const HOST_KEY = "sandhill.startHost";

/**
 * The start window: a card for each server and this Mac, with its recent
 * projects, and a way to open a folder on one of them as a new project.
 */
export function StartScreen(props: { shell: Shell; workspace: (host: string) => Promise<Workspace> }) {
  const [servers, setServers] = createSignal<string[]>([]);
  const [adding, setAdding] = createSignal(false);
  /** The host the folder picker browses, while it is open. */
  const [picking, setPicking] = createSignal<string>();
  // One connection per host for as long as the window is open.
  const workspaces = new Map<string, Promise<Workspace>>();
  const workspaceFor = (h: string) => {
    let ws = workspaces.get(h);
    if (!ws) workspaces.set(h, (ws = props.workspace(h)));
    return ws;
  };
  const hosts = () => [...servers(), LOCAL_HOST];

  onMount(async () => {
    setServers(await props.shell.listServers().catch(() => [] as string[]));
  });

  /** Where Open Folder… starts: the host used last, else the first server. */
  const defaultHost = () => {
    const last = stored(HOST_KEY);
    return last && hosts().includes(last) ? last : hosts()[0];
  };

  async function add(h: string) {
    try {
      setServers(await props.shell.addServer(h));
      setAdding(false);
    } catch (err) {
      notify(message(err));
    }
  }

  async function remove(h: string) {
    try {
      setServers(await props.shell.removeServer(h));
    } catch (err) {
      notify(message(err));
    }
    if (picking() === h) setPicking(undefined);
  }

  async function open(ws: Workspace, path: string) {
    try {
      const project = await ws.call<ProjectInfo>("open_project", { path });
      store(HOST_KEY, ws.backend.host);
      await props.shell.openProject(ws.backend.host, project.path);
      await props.shell.closeWindow();
    } catch (err) {
      notify(`Could not open ${path}: ${message(err)}`);
      throw err;
    }
  }

  return (
    <div class="start-window">
      <div class="drag-strip" data-tauri-drag-region />
      <div class="start">
        <div class="start-in">
          <header class="brand">
            <img src={appIcon} alt="" />
            <div>
              <h1>Sandhill</h1>
              <p>R and Python notebooks that keep running when you close the lid.</p>
            </div>
          </header>
          <div class="start-actions">
            <button
              class="primary"
              onClick={() => {
                setAdding(false);
                setPicking(picking() ? undefined : defaultHost());
              }}
            >
              <Icon name="folder" small />
              Open Folder…
            </button>
            <button
              class="connect"
              classList={{ pressed: adding() }}
              onClick={() => {
                setPicking(undefined);
                setAdding(!adding());
              }}
            >
              <Icon name="server" small />
              Connect to Server…
            </button>
          </div>
          <Show when={adding()}>
            <AddServer onAdd={(h) => void add(h)} onCancel={() => setAdding(false)} />
          </Show>
          <Show when={picking()} keyed>
            {(host) => (
              <FolderPickerCard
                host={host}
                hosts={hosts()}
                workspace={workspaceFor}
                onHost={setPicking}
                onPick={(ws, path) => open(ws, path).catch(() => {})}
                onCancel={() => setPicking(undefined)}
              />
            )}
          </Show>
          <div class="host-cards">
            <For each={hosts()}>
              {(host) => (
                <HostCard
                  host={host}
                  workspace={workspaceFor}
                  onOpen={open}
                  onPick={() => {
                    setAdding(false);
                    setPicking(host);
                  }}
                  onRemove={host === LOCAL_HOST ? undefined : () => void remove(host)}
                />
              )}
            </For>
          </div>
          <p class="start-foot">
            Kernels keep running after you close a project. Reopening it reattaches to them and restores your tabs.
          </p>
        </div>
      </div>
      <Dialogs />
    </div>
  );
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

function hostLabel(host: string): string {
  return host === LOCAL_HOST ? "This Mac" : host;
}

function AddServer(props: { onAdd: (host: string) => void; onCancel: () => void }) {
  const [text, setText] = createSignal("");
  return (
    <form
      class="add-server card"
      onSubmit={(e) => {
        e.preventDefault();
        if (text().trim()) props.onAdd(text().trim());
      }}
    >
      <div class="add-server-row">
        <input
          value={text()}
          onInput={(e) => setText(e.currentTarget.value)}
          placeholder="lab, or you@server.example.org"
          spellcheck={false}
          autocapitalize="off"
          aria-label="Server"
          ref={(el) => queueMicrotask(() => el.focus())}
        />
        <button type="button" onClick={props.onCancel}>
          Cancel
        </button>
        <button type="submit" class="primary" disabled={!text().trim()}>
          Add
        </button>
      </div>
      <p class="muted">
        Sandhill connects with ssh, using your ~/.ssh/config and keys, so a name from there works. The first time,
        it installs sandhilld in ~/.local/bin on the server.
      </p>
    </form>
  );
}

/** One host: whether it's reachable, and its recent projects. */
function HostCard(props: {
  host: string;
  workspace: (host: string) => Promise<Workspace>;
  onOpen: (ws: Workspace, path: string) => Promise<void>;
  onPick: () => void;
  onRemove?: () => void;
}) {
  const [ws] = createResource(() => props.host, props.workspace);
  return (
    <section class="host-card card" data-host={props.host}>
      <header>
        <Icon name={props.host === LOCAL_HOST ? "laptop" : "server"} small />
        <span class="host-name">{hostLabel(props.host)}</span>
        <Show when={ws()}>
          {(w) => (
            <small class="host-link" data-link={w().link().status} title={linkText(w())}>
              <span class="dot" />
              {w().link().status === "connected" ? "connected" : w().link().status === "connecting" ? "connecting" : "not connected"}
            </small>
          )}
        </Show>
        <Show when={props.onRemove}>
          <button class="host-remove" title="Remove from this list" onClick={() => props.onRemove?.()}>
            ×
          </button>
        </Show>
      </header>
      <Show when={ws()} keyed fallback={<p class="host-note">Connecting…</p>}>
        {(w) => <HostProjects ws={w} onOpen={props.onOpen} onPick={props.onPick} />}
      </Show>
    </section>
  );
}

/** One host's recent projects. */
function HostProjects(props: {
  ws: Workspace;
  onOpen: (ws: Workspace, path: string) => Promise<void>;
  onPick: () => void;
}) {
  const ws = props.ws;
  const connected = () => ws.link().status === "connected";
  const [recent, { refetch }] = createResource(
    () => connected() || undefined,
    () => ws.call<ProjectInfo[]>("list_recent_projects"),
  );
  const [opening, setOpening] = createSignal<string>();

  async function open(path: string) {
    if (opening()) return;
    setOpening(path);
    try {
      await props.onOpen(ws, path);
    } catch {
      void refetch();
    } finally {
      setOpening(undefined);
    }
  }

  async function forget(project: ProjectInfo) {
    try {
      await ws.call("forget_project", { path: project.path });
    } catch (err) {
      notify(message(err));
    }
    void refetch();
  }

  /** Kernels still running for a project, as the daemon lists them. */
  const kernels = (path: string) =>
    ws.sessions().filter((s) => s.project === path && s.status !== "ended").length;

  // Before projects the app showed one folder on this Mac; offer it until a project exists.
  const previousFolder = () =>
    ws.backend.host !== LOCAL_HOST || recent.loading || recent()?.length ? undefined : legacyFolder();

  return (
    <Show when={connected()} fallback={<p class="host-note">{linkText(ws)}</p>}>
      <ul class="recent-list">
        <Show when={previousFolder()}>
          {(folder) => (
            <li class="recent-item">
              <button class="recent-open" onClick={() => void open(folder())}>
                <span class="recent-name">{baseName(folder())}</span>
                <span class="recent-path">{folder()}</span>
                <span class="recent-when">the folder you used before projects</span>
              </button>
            </li>
          )}
        </Show>
        <For each={recent()}>
          {(project) => (
            <li class="recent-item" classList={{ missing: !project.exists }}>
              <button
                class="recent-open"
                disabled={!project.exists || !!opening()}
                title={project.exists ? project.path : "This folder was moved or deleted"}
                onClick={() => void open(project.path)}
              >
                <span class="recent-name">{project.name}</span>
                <span class="recent-path">{project.path}</span>
                <Show
                  when={project.exists && kernels(project.path)}
                  fallback={
                    <span class="recent-when">
                      {project.exists ? openedAgo(project.opened_ms) : "folder not found"}
                    </span>
                  }
                >
                  {(n) => (
                    <span class="recent-when live">
                      <span class="dot" />
                      {n() === 1 ? "1 kernel" : `${n()} kernels`}
                    </span>
                  )}
                </Show>
              </button>
              <button class="recent-forget" title="Remove from this list" onClick={() => void forget(project)}>
                ×
              </button>
            </li>
          )}
        </For>
      </ul>
      <Show when={!recent()?.length && !previousFolder()}>
        <p class="host-note">
          {recent.loading ? (
            "Loading…"
          ) : (
            <>
              No projects yet.{" "}
              <button class="link" onClick={() => props.onPick()}>
                Open a folder
              </button>{" "}
              with your analysis to start one.
            </>
          )}
        </p>
      </Show>
    </Show>
  );
}

/** The folder picker, on one of the hosts. */
function FolderPickerCard(props: {
  host: string;
  hosts: string[];
  workspace: (host: string) => Promise<Workspace>;
  onHost: (host: string) => void;
  onPick: (ws: Workspace, path: string) => void;
  onCancel: () => void;
}) {
  const [ws] = createResource(() => props.host, props.workspace);
  return (
    <div class="folder-picker card">
      <div class="picker-head">
        <span>Open a folder on</span>
        <select onChange={(e) => props.onHost(e.currentTarget.value)} aria-label="Where">
          <For each={props.hosts}>
            {(h) => (
              <option value={h} selected={h === props.host}>
                {hostLabel(h)}
              </option>
            )}
          </For>
        </select>
      </div>
      <Show when={ws()} keyed fallback={<p class="host-note">Connecting…</p>}>
        {(w) => (
          <Show when={w.link().status === "connected"} fallback={<p class="host-note">{linkText(w)}</p>}>
            <FolderPicker ws={w} onPick={(path) => props.onPick(w, path)} onCancel={props.onCancel} />
          </Show>
        )}
      </Show>
    </div>
  );
}

/** Browses folders on the daemon's machine, so it works the same for a server. */
function FolderPicker(props: { ws: Workspace; onPick: (path: string) => void; onCancel: () => void }) {
  const [path, setPath] = createSignal(props.ws.host()?.home ?? "/");
  const [typed, setTyped] = createSignal(path());
  const [folders] = createResource(path, async (dir) =>
    (await props.ws.call<DirEntry[]>("list_dir", { path: dir }))
      .filter((e) => e.kind === "dir" && !e.name.startsWith("."))
      .map((e) => e.name),
  );
  const go = (dir: string) => {
    const clean = dir.trim().replace(/(.)\/+$/, "$1") || "/";
    setPath(clean);
    setTyped(clean);
  };
  const child = (name: string) => `${path().replace(/\/$/, "")}/${name}`;
  return (
    <div class="folder-browser">
      <form
        class="folder-path"
        onSubmit={(e) => {
          e.preventDefault();
          go(typed());
        }}
      >
        <button type="button" title="Parent folder" onClick={() => go(path().replace(/\/[^/]+$/, "") || "/")}>
          <Icon name="up" small />
        </button>
        <input value={typed()} onInput={(e) => setTyped(e.currentTarget.value)} spellcheck={false} aria-label="Folder" />
      </form>
      <ul class="folder-list">
        <Show when={!folders.error} fallback={<li class="muted">{message(folders.error)}</li>}>
          <For each={folders()} fallback={<li class="muted">{folders.loading ? "Loading…" : "No folders here"}</li>}>
            {(name) => (
              <li>
                <button class="folder" onClick={() => go(child(name))} onDblClick={() => props.onPick(child(name))}>
                  <FileIcon name={name} dir />
                  {name}
                </button>
              </li>
            )}
          </For>
        </Show>
      </ul>
      <div class="folder-buttons">
        <button onClick={props.onCancel}>Cancel</button>
        <button class="primary" disabled={!!folders.error} onClick={() => props.onPick(path())}>
          Open {baseName(path())}
        </button>
      </div>
    </div>
  );
}

/** "today", "yesterday", "3 days ago", or the date. */
export function openedAgo(ms: number, now = Date.now()): string {
  const day = 24 * 60 * 60 * 1000;
  const startOfToday = new Date(now).setHours(0, 0, 0, 0);
  if (ms >= startOfToday) return "opened today";
  if (ms >= startOfToday - day) return "opened yesterday";
  const days = Math.ceil((startOfToday - ms) / day);
  if (days < 7) return `opened ${days} days ago`;
  return `opened ${new Date(ms).toLocaleDateString()}`;
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1) || path;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
