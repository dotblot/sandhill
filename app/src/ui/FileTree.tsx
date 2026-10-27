// The project's files, in the style of VS Code: a chevron column, an icon
// column, 16px per level and a plain guide line through each open folder's
// chevron column. New files and folders are named in a field in the tree,
// from the + menu, a folder's hover buttons or the right-click menu.

import { type Accessor, createResource, createSignal, For, type JSX, onMount, Show, untrack } from "solid-js";
import { LOCAL_HOST } from "../backend";
import type { Deleted, DirEntry, FileWritten } from "../backend/types";
import {
  baseName,
  copyName,
  joinPath,
  modifiedAgo,
  NEW_LABELS,
  NEW_NAMES,
  type NewKind,
  nameProblem,
  parentOf,
  shownInTree,
  type SortBy,
  sortEntries,
  stemLength,
  within,
} from "../model/files";
import { formatBytes } from "../model/machine";
import type { Workspace } from "../model/workspace";
import { ask, notify } from "./dialogs";
import { FileIcon, Icon } from "./icons";
import { closeOnOutside } from "./popup";

/** What the tree needs from the window around it. */
export interface TreeActions {
  /** Whether a file opens in Sandhill: in the editor, or the Viewer. */
  openable(path: string): boolean;
  /** What opening it is called in the menu: "Open", unless said. */
  openLabel?(path: string): string;
  open(path: string): void;
  /** Runs a script as a background job; undefined for files that can't be. */
  runJob?(path: string): (() => void) | undefined;
  /** What a new file starts with. */
  newText(name: string): string;
  /**
   * Closes the documents open from `path` before it is renamed or deleted,
   * and returns their paths; undefined, with a reason, if one has unsaved changes.
   */
  release(path: string): Promise<string[] | string>;
  /** Opens again documents `release` closed, at their paths now. */
  reopen(paths: string[]): void;
}

type Editing =
  | { mode: "new"; dir: string; kind: NewKind; value: string; error?: string }
  | { mode: "rename"; path: string; dir: boolean; value: string; error?: string };

const SORT_KEY = "sandhill.treeSort";

/** The tree's state, shared by the tree and the buttons over it. */
export function createTree(root: Accessor<string>) {
  const [open, setOpen] = createSignal<ReadonlySet<string>>(new Set());
  const [selected, setSelected] = createSignal<{ path: string; dir: boolean }>();
  const [editing, setEditing] = createSignal<Editing>();
  const stored = (() => {
    try {
      return localStorage.getItem(SORT_KEY);
    } catch {
      return null;
    }
  })();
  const [sortBy, setSortBy] = createSignal<SortBy>(stored === "size" || stored === "modified" ? stored : "name");
  const [reload, setReload] = createSignal(0);

  /** Opens `dir` and every folder above it, up to the project's. */
  const openTo = (dir: string) =>
    setOpen((set) => {
      const next = new Set(set);
      for (let d = dir; within(d, root()) && d !== root(); d = parentOf(d)) next.add(d);
      return next;
    });
  /** Where New makes things: the selected folder, the selected file's folder, or the project's. */
  const targetDir = () => {
    const sel = selected();
    if (!sel || !within(sel.path, root())) return root();
    return sel.dir ? sel.path : parentOf(sel.path);
  };
  return {
    root,
    open,
    toggle: (path: string) =>
      setOpen((set) => {
        const next = new Set(set);
        if (!next.delete(path)) next.add(path);
        return next;
      }),
    openTo,
    /** Moves open folders along with a rename. */
    moved(from: string, to: string) {
      setOpen((set) => new Set([...set].map((p) => (within(p, from) ? to + p.slice(from.length) : p))));
    },
    selected,
    select: (path?: string, dir = false) => setSelected(path ? { path, dir } : undefined),
    editing,
    setEditing,
    targetDir,
    /** Starts naming a new item in `dir`. */
    startNew(kind: NewKind, dir = targetDir()) {
      openTo(dir);
      setEditing({ mode: "new", dir, kind, value: NEW_NAMES[kind] });
    },
    startRename(path: string, dir: boolean) {
      setSelected({ path, dir });
      setEditing({ mode: "rename", path, dir, value: baseName(path) });
    },
    sortBy,
    setSort(by: SortBy) {
      setSortBy(by);
      try {
        localStorage.setItem(SORT_KEY, by);
      } catch {
        // The order is a convenience; it just isn't remembered.
      }
    },
    reload,
    refresh: () => setReload((n) => n + 1),
  };
}

export type Tree = ReturnType<typeof createTree>;

interface TreeProps {
  ws: Workspace;
  tree: Tree;
  actions: TreeActions;
  activePath?: string;
  refreshKey: number;
  /** A dot after open files whose session is idle or busy. */
  marker?: (path: string) => "idle" | "busy" | undefined;
}

/** What the right-click menu is on, and where it opens. */
interface MenuAt {
  path: string;
  dir: boolean;
  x: number;
  y: number;
}

export function FileTree(props: TreeProps) {
  const ops = treeOps(props);
  const [menu, setMenu] = createSignal<MenuAt>();
  let el!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    const sel = props.tree.selected();
    if (!sel || props.tree.editing() || e.target !== el) return;
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === "Enter" && !mod && sel.path !== props.tree.root()) props.tree.startRename(sel.path, sel.dir);
    else if (mod && e.key === "Backspace" && sel.path !== props.tree.root()) void ops.remove(sel.path, sel.dir);
    else if (mod && e.altKey && e.code === "KeyC") ops.copyPath(sel.path);
    else if (mod && e.key === "ArrowDown" && !sel.dir && props.actions.openable(sel.path)) props.actions.open(sel.path);
    else return;
    e.preventDefault();
  };
  return (
    <div
      class="tree"
      role="tree"
      tabIndex={0}
      ref={el}
      onKeyDown={onKey}
      onClick={(e) => e.target === el && props.tree.select(undefined)}
      onContextMenu={(e) => {
        e.preventDefault();
        const row = (e.target as HTMLElement).closest<HTMLElement>(".tree-item[data-path]");
        const path = row?.dataset.path ?? props.tree.root();
        const dir = !row || row.dataset.dir === "true";
        props.tree.select(row ? path : undefined, dir);
        setMenu({ path, dir, x: e.clientX, y: e.clientY });
      }}
    >
      <Dir {...props} ops={ops} path={props.tree.root()} depth={0} />
      <Show when={menu()}>{(at) => <TreeMenu {...props} ops={ops} at={at()} onClose={() => setMenu(undefined)} />}</Show>
    </div>
  );
}

type Ops = ReturnType<typeof treeOps>;

function Dir(props: TreeProps & { ops: Ops; path: string; depth: number }) {
  const sizes = () => props.tree.sortBy() === "size";
  const [entries] = createResource(
    () => [props.path, props.refreshKey, props.tree.reload(), sizes()] as const,
    async ([path, , , sizes]) =>
      (await props.ws.call<DirEntry[]>("list_dir", sizes ? { path, sizes } : { path })).filter((e) => shownInTree(e.name)),
  );
  const sorted = () => sortEntries(entries() ?? [], props.tree.sortBy());
  const newHere = () => {
    const e = props.tree.editing();
    return e?.mode === "new" && e.dir === props.path ? e : undefined;
  };
  return (
    <>
      <Show when={newHere()}>
        {(e) => <NameRow {...props} depth={props.depth} dir={e().kind === "folder"} editing={e()} label={NEW_LABELS[e().kind]} />}
      </Show>
      <Show when={!entries.error} fallback={<div class="tree-note" style={indent(props.depth)}>{String(entries.error)}</div>}>
        <For each={sorted()}>{(entry) => <Row {...props} entry={entry} path={joinPath(props.path, entry.name)} />}</For>
      </Show>
    </>
  );
}

function Row(props: TreeProps & { ops: Ops; entry: DirEntry; path: string; depth: number }) {
  const dir = props.entry.kind === "dir";
  const isOpen = () => dir && props.tree.open().has(props.path);
  const renaming = () => {
    const e = props.tree.editing();
    return e?.mode === "rename" && e.path === props.path ? e : undefined;
  };
  const openable = () => !dir && props.actions.openable(props.path);
  const tip = () => {
    const sized = !dir || props.tree.sortBy() === "size";
    return [props.entry.name, sized && formatBytes(props.entry.size), `modified ${modifiedAgo(props.entry.mtime_ms)}`]
      .filter(Boolean)
      .join(" · ");
  };
  return (
    <>
      <Show
        when={renaming()}
        fallback={
          <div
            class="tree-item"
            classList={{
              dir,
              file: !dir,
              openable: openable(),
              dim: !dir && !openable(),
              open: isOpen(),
              sel: props.tree.selected()?.path === props.path,
              active: props.activePath === props.path,
            }}
            data-path={props.path}
            data-dir={String(dir)}
            role="treeitem"
            aria-expanded={dir ? isOpen() : undefined}
            style={indent(props.depth)}
            title={tip()}
            onClick={(e) => {
              if ((e.target as HTMLElement).closest(".tacts")) return;
              props.tree.select(props.path, dir);
              if (dir) props.tree.toggle(props.path);
              else if (openable()) props.actions.open(props.path);
            }}
          >
            <Guides depth={props.depth} />
            <span class="chev">
              <Show when={dir}>
                <Icon name="chevronRight" small />
              </Show>
            </span>
            <FileIcon name={props.entry.name} dir={dir} />
            <span class="tree-name">{props.entry.name}</span>
            <Meta {...props} dir={dir} openable={openable()} />
            <Show when={dir}>
              <span class="tacts">
                <button title={`New file in ${props.entry.name}`} onClick={() => props.tree.startNew("file", props.path)}>
                  <Icon name="file.new" />
                </button>
                <button title={`New folder in ${props.entry.name}`} onClick={() => props.tree.startNew("folder", props.path)}>
                  <Icon name="folder.new" />
                </button>
              </span>
            </Show>
          </div>
        }
      >
        {(e) => <NameRow {...props} depth={props.depth} dir={dir} editing={e()} label={`Rename ${props.entry.name}`} />}
      </Show>
      <Show when={isOpen()}>
        <Dir {...props} path={props.path} depth={props.depth + 1} />
      </Show>
    </>
  );
}

/** At a row's right: a dot for an open file's session, or the size of a file Sandhill doesn't open (and of folders when sorted by size). */
function Meta(props: TreeProps & { entry: DirEntry; path: string; dir: boolean; openable: boolean }) {
  const state = () => (props.openable ? props.marker?.(props.path) : undefined);
  const size = () => (props.dir ? props.tree.sortBy() === "size" : !props.openable);
  return (
    <Show when={state()} fallback={<Show when={size()}><span class="tree-meta">{formatBytes(props.entry.size)}</span></Show>}>
      {(s) => (
        <span class="tree-meta">
          <span class="dot" classList={{ busy: s() === "busy", idle: s() === "idle" }} />
        </span>
      )}
    </Show>
  );
}

/**
 * The name field, at the top of a folder's children for a new item or in
 * place for a rename. The icon follows what is typed. Enter makes it, Esc
 * cancels, and clicking away makes it if a name was typed.
 */
function NameRow(props: TreeProps & { ops: Ops; depth: number; dir: boolean; editing: Editing; label: string }) {
  let input!: HTMLInputElement;
  let done = false;
  const original = props.editing.mode === "rename" ? props.editing.value : "";
  onMount(() => {
    input.focus();
    input.setSelectionRange(0, stemLength(input.value, props.dir));
  });
  const commit = async () => {
    if (done) return;
    done = true;
    const ok = await props.ops.commit(untrack(props.tree.editing)!, input.value.trim());
    if (!ok) {
      done = false;
      input.focus();
    }
  };
  const cancel = () => {
    done = true;
    props.tree.setEditing(undefined);
  };
  const icon = () => props.editing.value || (props.dir ? "folder" : "");
  return (
    <>
      <div class="tree-item tnew" style={indent(props.depth)}>
        <Guides depth={props.depth} />
        <span class="chev" />
        <FileIcon name={icon()} dir={props.dir} />
        <input
          class="tname"
          ref={input}
          value={props.editing.value}
          spellcheck={false}
          autocomplete="off"
          aria-label={props.label}
          aria-invalid={!!props.editing.error}
          onInput={() => props.tree.setEditing({ ...props.editing, value: input.value, error: undefined })}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              cancel();
            }
          }}
          onBlur={() =>
            setTimeout(() => {
              if (done || untrack(props.tree.editing) === undefined) return;
              const value = input.value.trim();
              if (value && value !== original) void commit();
              else cancel();
            }, 120)
          }
        />
      </div>
      <Show when={props.editing.error}>
        <div class="tnew-err" style={{ "padding-left": `${44 + 16 * props.depth}px` }}>
          {props.editing.error}
        </div>
      </Show>
    </>
  );
}

/** The right-click menu: New items on folders, then rename, duplicate, reveal, copy path and delete. */
function TreeMenu(props: TreeProps & { ops: Ops; at: MenuAt; onClose: () => void }) {
  let pop!: HTMLDivElement;
  // The menu's items run after it has closed.
  const at = untrack(() => props.at);
  closeOnOutside(() => pop, () => undefined, props.onClose);
  const top = () => at.path === props.tree.root();
  const remote = props.ws.backend.host !== LOCAL_HOST;
  const name = () => baseName(at.path);
  const dirOf = () => (at.dir ? at.path : parentOf(at.path));
  const run = (fn: () => void) => () => {
    props.onClose();
    fn();
  };
  const [pos, setPos] = createSignal({ left: at.x, top: at.y });
  onMount(() => {
    const r = pop.getBoundingClientRect();
    setPos({ left: Math.max(8, Math.min(at.x, innerWidth - r.width - 8)), top: Math.max(8, Math.min(at.y, innerHeight - r.height - 8)) });
  });
  return (
    <div class="kpop treemenu" role="menu" aria-label={`Actions for ${name()}`} ref={pop} style={{ left: `${pos().left}px`, top: `${pos().top}px` }}>
      <Show
        when={at.dir}
        fallback={
          <>
            <Show when={props.actions.openable(at.path)}>
              <Item label={props.actions.openLabel?.(at.path) ?? "Open"} keys="⌘↓" onClick={run(() => props.actions.open(at.path))} />
            </Show>
            <Show when={props.actions.runJob?.(at.path)}>
              {(runJob) => <Item label="Run as job" onClick={run(runJob())} />}
            </Show>
            <Show when={props.actions.openable(at.path) || props.actions.runJob?.(at.path)}>
              <div class="mm-sep" />
            </Show>
          </>
        }
      >
        <NewItems dir={dirOf()} name={top() ? projectName(props.tree.root()) : name()} onPick={(kind) => run(() => props.tree.startNew(kind, dirOf()))()} />
        <div class="mm-sep" />
      </Show>
      <Show when={!top()}>
        <Item label="Rename…" keys="↩" onClick={run(() => props.tree.startRename(at.path, at.dir))} />
        <Item label="Duplicate" onClick={run(() => void props.ops.duplicate(at.path, at.dir))} />
      </Show>
      <Show when={!remote && props.ws.backend.reveal}>
        <Item label="Reveal in Finder" onClick={run(() => props.ops.reveal(at.path))} />
      </Show>
      <Show when={remote && !at.dir && props.ws.backend.download}>
        <Item label="Download to this Mac…" onClick={run(() => props.ops.download(at.path))} />
      </Show>
      <Item label="Copy path" keys="⌥⌘C" onClick={run(() => props.ops.copyPath(at.path))} />
      <Show when={!top()}>
        <div class="mm-sep" />
        <Item label={remote ? "Delete…" : "Move to Trash"} keys="⌘⌫" onClick={run(() => void props.ops.remove(at.path, at.dir))} />
      </Show>
    </div>
  );
}

function projectName(path: string): string {
  return baseName(path) || path;
}

/** "New in …": new file and folder, then a Quarto document, an R script and a notebook. */
export function NewItems(props: { dir: string; name: string; onPick: (kind: NewKind) => void }) {
  return (
    <>
      <div class="mm-h">New in {props.name}</div>
      <Item label="New file…" keys="⌘N" icon={<Icon name="file.new" small />} onClick={() => props.onPick("file")} />
      <Item label="New folder…" keys="⌥⌘N" icon={<Icon name="folder.new" small />} onClick={() => props.onPick("folder")} />
      <Item label="New Quarto document…" icon={<FileIcon name="x.qmd" />} onClick={() => props.onPick("qmd")} />
      <Item label="New R script…" icon={<FileIcon name="x.R" />} onClick={() => props.onPick("R")} />
      <Item label="New notebook…" icon={<FileIcon name="x.ipynb" />} onClick={() => props.onPick("ipynb")} />
    </>
  );
}

function Item(props: { label: string; keys?: string; icon?: JSX.Element; onClick: () => void }) {
  return (
    <button class="mm-i" role="menuitem" onClick={() => props.onClick()}>
      {props.icon ?? <span />}
      <span>{props.label}</span>
      <kbd>{props.keys ?? ""}</kbd>
    </button>
  );
}

/** What the tree's items do, through the daemon. */
function treeOps(props: TreeProps) {
  const ws = props.ws;
  const tree = props.tree;
  const dirLabel = (dir: string) => (dir === tree.root() ? projectName(dir) : baseName(dir));
  const problem = (err: unknown, name: string, dir: string) => {
    const text = err instanceof Error ? err.message : String(err);
    return text.startsWith("exists:") ? `“${name}” already exists in ${dirLabel(dir)}` : text;
  };
  const setError = (error: string) => {
    const e = tree.editing();
    if (e) tree.setEditing({ ...e, error });
  };
  /** Closes documents open from `path`; undefined, after saying why, if one has unsaved changes. */
  const release = async (path: string, field = false) => {
    const released = await props.actions.release(path);
    if (typeof released !== "string") return released;
    if (field) setError(released);
    else notify(released);
    return undefined;
  };

  async function commitNew(e: Extract<Editing, { mode: "new" }>, value: string): Promise<boolean> {
    const bad = nameProblem(value, true);
    if (bad) return setError(bad), false;
    const path = joinPath(e.dir, value);
    try {
      if (e.kind === "folder") await ws.call<FileWritten>("make_dir", { path });
      else await ws.call<FileWritten>("write_file", { path, text: props.actions.newText(baseName(path)), create: true });
    } catch (err) {
      return setError(problem(err, value, e.dir)), false;
    }
    tree.setEditing(undefined);
    tree.openTo(e.kind === "folder" ? path : parentOf(path));
    tree.select(path, e.kind === "folder");
    tree.refresh();
    if (e.kind !== "folder" && props.actions.openable(path)) props.actions.open(path);
    return true;
  }

  async function commitRename(e: Extract<Editing, { mode: "rename" }>, value: string): Promise<boolean> {
    if (!value || value === baseName(e.path)) return tree.setEditing(undefined), true;
    const bad = nameProblem(value, false);
    if (bad) return setError(bad), false;
    const to = joinPath(parentOf(e.path), value);
    const closed = await release(e.path, true);
    if (!closed) return false;
    try {
      await ws.call<FileWritten>("move_path", { from: e.path, to });
    } catch (err) {
      props.actions.reopen(closed);
      return setError(problem(err, value, parentOf(e.path))), false;
    }
    tree.setEditing(undefined);
    tree.moved(e.path, to);
    tree.select(to, e.dir);
    tree.refresh();
    props.actions.reopen(closed.map((p) => to + p.slice(e.path.length)));
    return true;
  }

  return {
    /** Makes or renames what the name field names; false keeps the field open. */
    commit: (e: Editing, value: string) => (e.mode === "new" ? commitNew(e, value) : commitRename(e, value)),
    async duplicate(path: string, dir: boolean) {
      const parent = parentOf(path);
      try {
        const taken = new Set((await ws.call<DirEntry[]>("list_dir", { path: parent })).map((x) => x.name));
        const to = joinPath(parent, copyName(baseName(path), dir, taken));
        await ws.call<FileWritten>("copy_path", { from: path, to });
        tree.select(to, dir);
        tree.refresh();
      } catch (err) {
        notify(`Could not duplicate ${baseName(path)}: ${problem(err, baseName(path), parent)}`);
      }
    },
    /** Moves it to the Trash on this Mac; on a server, deletes it once you say so. */
    async remove(path: string, dir: boolean) {
      const remote = ws.backend.host !== LOCAL_HOST;
      if (remote) {
        const what = dir ? "the folder and everything in it" : "it";
        const choice = await ask(`Delete “${baseName(path)}”?`, ["Delete", "Cancel"], `This deletes ${what} on ${ws.backend.host}. It can't be put back.`);
        if (choice !== "Delete") return;
      }
      const closed = await release(path);
      if (!closed) return;
      try {
        await ws.call<Deleted>("delete_path", { path, trash: !remote });
      } catch (err) {
        props.actions.reopen(closed);
        notify(`Could not ${remote ? "delete" : "move to the Trash"} ${baseName(path)}: ${problem(err, baseName(path), parentOf(path))}`);
        return;
      }
      if (tree.selected()?.path === path) tree.select(undefined);
      tree.refresh();
    },
    copyPath(path: string) {
      void navigator.clipboard.writeText(path).catch((err) => notify(`Could not copy the path: ${err}`));
    },
    reveal(path: string) {
      void ws.backend.reveal?.(path).catch((err) => notify(String(err)));
    },
    download(path: string) {
      notify(`Downloading ${baseName(path)}…`, "info");
      void ws.backend
        .download?.(path)
        .then((where) => notify(`Downloaded ${baseName(path)} to ${where}`, "info"))
        .catch((err) => notify(`Could not download ${baseName(path)}: ${err}`));
    },
  };
}

/** One guide line per level above the row, through that open folder's chevron column. */
function Guides(props: { depth: number }) {
  return <For each={Array.from({ length: props.depth }, (_, level) => level)}>{(level) => <i class="tg" style={{ left: `${15 + 16 * level}px` }} />}</For>;
}

/**
 * Top-level chevrons start 14px from the window's edge, like the rest of
 * the left pane, and icons at 32px; each level indents 16px.
 */
function indent(depth: number) {
  return { "padding-left": `${8 + depth * 16}px` };
}
