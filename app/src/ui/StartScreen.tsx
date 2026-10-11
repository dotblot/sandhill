import { createResource, createSignal, For, Show } from "solid-js";
import type { Shell } from "../backend";
import type { DirEntry, ProjectInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { Dialogs, notify } from "./dialogs";
import { previousFolder as legacyFolder } from "./legacy";

/** The start window: recent projects on this host, and opening a folder as a new one. */
export function StartScreen(props: { ws: Workspace; shell: Shell }) {
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

  // Before projects the app showed one folder; offer it until a project exists.
  const previousFolder = () => (recent.loading || recent()?.length ? undefined : legacyFolder());

  return (
    <div class="start">
      <header class="start-header">
        <h1>Sandhill</h1>
        <span class="muted">{linkText(ws)}</span>
      </header>
      <div class="start-actions">
        <button class="primary" disabled={!connected()} onClick={() => setPicking(true)}>
          Open Folder…
        </button>
      </div>
      <Show when={picking()}>
        <FolderPicker ws={ws} onPick={(path) => void open(path)} onCancel={() => setPicking(false)} />
      </Show>
      <section class="recent">
        <h2>{ws.backend.host === "local" ? "On this Mac" : ws.host()?.hostname}</h2>
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
        <Show when={recent()?.length || previousFolder()} fallback={<EmptyRecent loading={!recent()} />}>
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
      <Dialogs />
    </div>
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

function linkText(ws: Workspace): string {
  const link = ws.link();
  if (link.status === "connected") return ws.host() ? `sandhilld ${ws.host()!.daemon_version} on ${ws.host()!.hostname}` : "";
  if (link.status === "connecting") return "Connecting to sandhilld…";
  return `Disconnected${link.error ? `: ${link.error}` : ""}. Retrying…`;
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
