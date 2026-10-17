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
      <Dir {...props} path={props.root} depth={0} lines={[]} />
    </div>
  );
}

/** Where a row sits: its depth, and for each level above it whether that level's guide line runs on past it. */
interface Place {
  depth: number;
  lines: boolean[];
}

function Dir(props: TreeProps & Place & { path: string }) {
  const [entries] = createResource(
    () => [props.path, props.refreshKey] as const,
    async ([path]) =>
      (await props.ws.call<DirEntry[]>("list_dir", { path })).filter((e) => !e.name.startsWith(".")),
  );
  return (
    <Show when={!entries.error} fallback={<div class="tree-note" style={indent(props.depth)}>{String(entries.error)}</div>}>
      <For each={entries()}>
        {(entry, i) => {
          const path = `${props.path.replace(/\/$/, "")}/${entry.name}`;
          const last = () => i() === entries()!.length - 1;
          if (entry.kind === "dir") return <DirNode {...props} path={path} name={entry.name} last={last()} />;
          const openable = !!docKind(entry.name);
          return (
            <div
              class="tree-item file"
              classList={{ openable, active: props.activePath === path }}
              style={indent(props.depth)}
              title={openable ? path : "Only .qmd, .Rmd, .ipynb, .R and .py files open in Sandhill for now"}
              onClick={() => openable && props.onOpen(path)}
            >
              <Guides depth={props.depth} lines={props.lines} last={last()} />
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

function DirNode(props: TreeProps & Place & { path: string; name: string; last: boolean }) {
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <div
        class="tree-item dir"
        classList={{ open: open() }}
        style={indent(props.depth)}
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <Guides depth={props.depth} lines={props.lines} last={props.last} />
        <span class="chev">
          <Icon name="chevronRight" small />
        </span>
        <FileIcon name={props.name} dir />
        <span class="tree-name">{props.name}</span>
      </div>
      <Show when={open()}>
        <Dir {...props} depth={props.depth + 1} lines={[...props.lines, !props.last]} />
      </Show>
    </>
  );
}

/**
 * A row's guide lines: one running down from each open folder above it whose
 * children go on past this row, and an elbow from its own folder's line. Each
 * line runs down from the centre of its folder's chevron.
 */
function Guides(props: Place & { last: boolean }) {
  const x = (level: number) => ({ left: `${14 + 15 * level}px` });
  return (
    <Show when={props.depth > 0}>
      <For each={props.lines.slice(1)}>{(on, level) => <Show when={on}>{<i class="tg" style={x(level())} />}</Show>}</For>
      <i class="tg" classList={{ end: props.last }} style={x(props.depth - 1)} />
      <i class="te" style={x(props.depth - 1)} />
    </Show>
  );
}

/**
 * Top-level icons start 14px from the window's edge, like the rest of the left
 * pane; each level indents 15px, so files line up with their folder's icon.
 */
function indent(depth: number) {
  return { "padding-left": `${8 + depth * 15}px` };
}
