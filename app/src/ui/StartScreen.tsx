import { createResource, createSignal, For, onMount, Show } from "solid-js";
import { LOCAL_HOST, type Shell } from "../backend";
import type { DirEntry, ProjectInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { Dialogs, notify } from "./dialogs";
import { previousFolder as legacyFolder } from "./legacy";
import { linkText } from "./linkText";

const HOST_KEY = "sandhill.startHost";

/**
 * The start window: this Mac and the servers, each with its recent projects
 * and a way to open a folder there as a new project.
 */
export function StartScreen(props: { shell: Shell; workspace: (host: string) => Promise<Workspace> }) {
  const [servers, setServers] = createSignal<string[]>([]);
  const [host, setHost] = createSignal(LOCAL_HOST);
  const [adding, setAdding] = createSignal(false);
  // One connection per host for as long as the window is open.
  const workspaces = new Map<string, Promise<Workspace>>();
  const workspaceFor = (h: string) => {
    let ws = workspaces.get(h);
    if (!ws) workspaces.set(h, (ws = props.workspace(h)));
    return ws;
  };
  const [workspace] = createResource(host, workspaceFor);

  onMount(async () => {
    const list = await props.shell.listServers().catch(() => [] as string[]);
    setServers(list);
    const last = storedHost();
    if (last && list.includes(last)) setHost(last);
  });

  function select(h: string) {
    setHost(h);
    setAdding(false);
    try {
      localStorage.setItem(HOST_KEY, h);
    } catch {
      // Private mode; the choice just isn't remembered.
    }
  }

  async function add(h: string) {
    try {
      setServers(await props.shell.addServer(h));
      select(h.trim());
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
    if (host() === h) select(LOCAL_HOST);
  }

  return (
    <div class="start">
      <header class="start-header">
        <h1>Sandhill</h1>
      </header>
      <nav class="hosts" aria-label="Where to work">
        <button class="host" classList={{ active: host() === LOCAL_HOST }} onClick={() => select(LOCAL_HOST)}>
          This Mac
        </button>
        <For each={servers()}>
          {(server) => (
            <span class="host-item">
              <button class="host" classList={{ active: host() === server }} onClick={() => select(server)}>
                {server}
              </button>
              <button class="host-remove" title="Remove from this list" onClick={() => void remove(server)}>
                ×
              </button>
            </span>
          )}
        </For>
        <button class="host add" classList={{ active: adding() }} onClick={() => setAdding(!adding())}>
          + Server…
        </button>
      </nav>
      <Show when={adding()}>
        <AddServer onAdd={(h) => void add(h)} onCancel={() => setAdding(false)} />
      </Show>
      <Show when={workspace()} keyed fallback={<p class="muted">Connecting…</p>}>
        {(ws) => <HostProjects ws={ws} shell={props.shell} />}
      </Show>
      <Dialogs />
    </div>
  );
}

function storedHost(): string | null {
  try {
    return localStorage.getItem(HOST_KEY);
  } catch {
    return null;
  }
}

function AddServer(props: { onAdd: (host: string) => void; onCancel: () => void }) {
  const [text, setText] = createSignal("");
  return (
    <form
      class="add-server"
      onSubmit={(e) => {
        e.preventDefault();
        if (text().trim()) props.onAdd(text());
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

/** One host's recent projects, and opening a folder there. */
function HostProjects(props: { ws: Workspace; shell: Shell }) {
  const ws = props.ws;
  const connected = () => ws.link().status === "connected";
  const [recent, { refetch }] = createResource(
    () => connected() || undefined,
    () => ws.call<ProjectInfo[]>("list_recent_projects"),
  );
  const [picking, setPicking] = createSignal(false);
  const [opening, setOpening] = createSignal<string>();

  async function open(path: string) {
    if (opening()) return;
    setOpening(path);
    try {
      const project = await ws.call<ProjectInfo>("open_project", { path });
      await props.shell.openProject(ws.backend.host, project.path);
      await props.shell.closeWindow();
    } catch (err) {
      notify(`Could not open ${path}: ${message(err)}`);
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

  // Before projects the app showed one folder on this Mac; offer it until a project exists.
  const previousFolder = () =>
    ws.backend.host !== LOCAL_HOST || recent.loading || recent()?.length ? undefined : legacyFolder();

  return (
    <>
      <p class="start-link muted" data-link={ws.link().status}>
        {linkText(ws)}
      </p>
      <div class="start-actions">
        <button class="primary" disabled={!connected()} onClick={() => setPicking(true)}>
          Open Folder…
        </button>
      </div>
      <Show when={picking()}>
        <FolderPicker ws={ws} onPick={(path) => void open(path)} onCancel={() => setPicking(false)} />
      </Show>
      <section class="recent">
        <h2>{ws.backend.host === LOCAL_HOST ? "On this Mac" : `On ${ws.host()?.hostname ?? ws.backend.host}`}</h2>
        <Show when={previousFolder()}>
          {(folder) => (
            <ul class="recent-list">
              <li class="recent-item">
                <button class="recent-open" onClick={() => void open(folder())}>
                  <span class="recent-name">{baseName(folder())}</span>
                  <span class="recent-path">{folder()} · the folder you used before projects</span>
                </button>
              </li>
            </ul>
          )}
        </Show>
        <Show
          when={recent()?.length || previousFolder()}
          fallback={<EmptyRecent loading={connected() && !recent()} />}
        >
          <ul class="recent-list">
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
                    <span class="recent-path">
                      {project.path} · {project.exists ? openedAgo(project.opened_ms) : "folder not found"}
                    </span>
                  </button>
                  <button class="recent-forget" title="Remove from this list" onClick={() => void forget(project)}>
                    ×
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </section>
    </>
  );
}

function EmptyRecent(props: { loading: boolean }) {
  return (
    <p class="muted">
      {props.loading ? "Loading…" : "No projects yet. Open a folder with your analysis to start one."}
    </p>
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
    <div class="folder-picker">
      <form
        class="folder-path"
        onSubmit={(e) => {
          e.preventDefault();
          go(typed());
        }}
      >
        <button type="button" title="Parent folder" onClick={() => go(path().replace(/\/[^/]+$/, "") || "/")}>
          ↑
        </button>
        <input value={typed()} onInput={(e) => setTyped(e.currentTarget.value)} spellcheck={false} aria-label="Folder" />
      </form>
      <ul class="folder-list">
        <Show when={!folders.error} fallback={<li class="muted">{message(folders.error)}</li>}>
          <For each={folders()} fallback={<li class="muted">{folders.loading ? "Loading…" : "No folders here"}</li>}>
            {(name) => (
              <li>
                <button class="folder" onClick={() => go(child(name))} onDblClick={() => props.onPick(child(name))}>
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
