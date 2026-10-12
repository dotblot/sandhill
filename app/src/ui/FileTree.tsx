import { createResource, createSignal, For, Show } from "solid-js";
import type { DirEntry } from "../backend/types";
import { docKind } from "../docs/doc";
import { formatBytes } from "../model/machine";
import type { Workspace } from "../model/workspace";
import { FileIcon, Icon } from "./icons";

interface TreeProps {
  ws: Workspace;
  onOpen: (path: string) => void;
  activePath?: string;
  refreshKey: number;
  /** A dot after open files whose session is idle or busy. */
  marker?: (path: string) => "idle" | "busy" | undefined;
}

export function FileTree(props: TreeProps & { root: string }) {
  return (
    <div class="tree" role="tree">
      <Dir {...props} path={props.root} depth={0} open />
    </div>
  );
}

function Dir(props: TreeProps & { path: string; depth: number; open?: boolean }) {
  const [entries] = createResource(
    () => [props.path, props.refreshKey] as const,
    async ([path]) =>
      (await props.ws.call<DirEntry[]>("list_dir", { path })).filter((e) => !e.name.startsWith(".")),
  );
  return (
    <Show when={!entries.error} fallback={<div class="tree-note" style={indent(props.depth)}>{String(entries.error)}</div>}>
      <For each={entries()}>
        {(entry) => {
          const path = `${props.path.replace(/\/$/, "")}/${entry.name}`;
          if (entry.kind === "dir") return <DirNode {...props} path={path} name={entry.name} />;
          const openable = !!docKind(entry.name);
          return (
            <div
              class="tree-item file"
              classList={{ openable, active: props.activePath === path }}
              style={indent(props.depth)}
              title={openable ? path : "Only .qmd, .Rmd, .ipynb, .R and .py files open in Sandhill for now"}
              onClick={() => openable && props.onOpen(path)}
            >
              <FileIcon name={entry.name} />
              <span class="tree-name">{entry.name}</span>
              <Show when={openable ? props.marker?.(path) : undefined} fallback={<Show when={!openable}><span class="tree-meta">{formatBytes(entry.size)}</span></Show>}>
                {(state) => (
                  <span class="tree-meta">
                    <span class="dot" classList={{ busy: state() === "busy", idle: state() === "idle" }} />
                  </span>
                )}
              </Show>
            </div>
          );
        }}
      </For>
    </Show>
  );
}

function DirNode(props: TreeProps & { path: string; name: string; depth: number }) {
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <div class="tree-item dir" classList={{ open: open() }} style={indent(props.depth)} onClick={() => setOpen(!open())}>
        <span class="chev">
          <Icon name="chevronRight" small />
        </span>
        <FileIcon name={props.name} dir />
        <span class="tree-name">{props.name}</span>
      </div>
      <Show when={open()}>
        <Dir {...props} depth={props.depth + 1} />
      </Show>
    </>
  );
}

/**
 * Top-level icons start 14px from the window's edge, like the rest of the left
 * pane; each level indents 15px, so files line up with their folder's icon.
 */
function indent(depth: number) {
  return { "padding-left": `${8 + depth * 15}px` };
}
