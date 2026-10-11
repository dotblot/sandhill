import { createResource, createSignal, For, Show } from "solid-js";
import type { DirEntry } from "../backend/types";
import { docKind } from "../docs/doc";
import type { Workspace } from "../model/workspace";

export function FileTree(props: {
  ws: Workspace;
  root: string;
  onOpen: (path: string) => void;
  activePath?: string;
  refreshKey: number;
}) {
  return (
    <div class="tree" role="tree">
      <Dir ws={props.ws} path={props.root} depth={0} onOpen={props.onOpen} activePath={props.activePath} refreshKey={props.refreshKey} open />
    </div>
  );
}

function Dir(props: {
  ws: Workspace;
  path: string;
  depth: number;
  open?: boolean;
  onOpen: (path: string) => void;
  activePath?: string;
  refreshKey: number;
}) {
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
              title={openable ? path : "Only .qmd, .Rmd and .ipynb files open in Sandhill for now"}
              onClick={() => openable && props.onOpen(path)}
            >
              <span class="tree-icon">{iconFor(entry.name)}</span>
              {entry.name}
            </div>
          );
        }}
      </For>
    </Show>
  );
}

function DirNode(props: {
  ws: Workspace;
  path: string;
  name: string;
  depth: number;
  onOpen: (path: string) => void;
  activePath?: string;
  refreshKey: number;
}) {
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <div class="tree-item dir" style={indent(props.depth)} onClick={() => setOpen(!open())}>
        <span class="tree-icon">{open() ? "▾" : "▸"}</span>
        {props.name}
      </div>
      <Show when={open()}>
        <Dir {...props} depth={props.depth + 1} />
      </Show>
    </>
  );
}

function indent(depth: number) {
  return { "padding-left": `${8 + depth * 14}px` };
}

function iconFor(name: string): string {
  const kind = docKind(name);
  if (kind === "ipynb") return "◫";
  if (kind === "qmd") return "◧";
  return "·";
}
