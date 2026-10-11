// ⌘K: go to a file of the project, or run a command, by typing part of its name.

import { createMemo, createResource, createSignal, For, type JSX, Show } from "solid-js";
import { matchScore } from "../model/projectFiles";
import { FileIcon, Icon } from "./icons";

export interface Command {
  label: string;
  keys?: string;
  run: () => void;
}

interface Item {
  key: string;
  label: string;
  detail?: string;
  icon: () => JSX.Element;
  run: () => void;
}

const SHOWN = 50;

export function Palette(props: {
  root: string;
  commands: Command[];
  files: () => Promise<string[]>;
  onOpenFile: (path: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = createSignal("");
  const [index, setIndex] = createSignal(0);
  const [files] = createResource(props.files);
  const relative = (path: string) => path.slice(props.root.replace(/\/$/, "").length + 1);

  const items = createMemo<Item[]>(() => {
    const fileItems: Item[] = (files() ?? []).map((path) => {
      const rel = relative(path);
      const slash = rel.lastIndexOf("/");
      const name = rel.slice(slash + 1);
      return {
        key: `file:${path}`,
        label: name,
        detail: slash > 0 ? rel.slice(0, slash) : undefined,
        icon: () => <FileIcon name={name} />,
        run: () => props.onOpenFile(path),
      };
    });
    const commandItems: Item[] = props.commands.map((c) => ({
      key: `command:${c.label}`,
      label: c.label,
      detail: c.keys,
      icon: () => <Icon name="command" small />,
      run: c.run,
    }));
    const q = query().trim();
    if (!q) return [...fileItems.slice(0, 8), ...commandItems];
    return [...fileItems, ...commandItems]
      .map((item) => ({ item, score: matchScore(item.detail && item.key.startsWith("file:") ? `${item.detail}/${item.label}` : item.label, q) }))
      .filter((m): m is { item: Item; score: number } => m.score !== undefined)
      .sort((a, b) => b.score - a.score)
      .slice(0, SHOWN)
      .map((m) => m.item);
  });

  const choose = (item: Item | undefined) => {
    if (!item) return;
    props.onClose();
    item.run();
  };
  let list!: HTMLDivElement;
  const move = (by: number) => {
    const n = items().length;
    if (!n) return;
    setIndex((i) => (i + by + n) % n);
    list.children[index()]?.scrollIntoView({ block: "nearest" });
  };

  return (
    <div class="palette" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="pal-card" role="dialog" aria-label="Go to file or command">
        <label class="pal-in">
          <Icon name="search" small />
          <input
            ref={(el) => queueMicrotask(() => el.focus())}
            value={query()}
            placeholder="Go to a file or run a command"
            spellcheck={false}
            autocapitalize="off"
            aria-label="Go to"
            onInput={(e) => {
              setQuery(e.currentTarget.value);
              setIndex(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") move(1);
              else if (e.key === "ArrowUp") move(-1);
              else if (e.key === "Enter") choose(items()[index()]);
              else if (e.key === "Escape") props.onClose();
              else return;
              e.preventDefault();
            }}
          />
          <kbd>esc</kbd>
        </label>
        <div class="pal-list" ref={list} role="listbox">
          <For each={items()}>
            {(item, i) => (
              <button
                class="pal-i"
                role="option"
                aria-selected={i() === index()}
                classList={{ on: i() === index() }}
                onMouseMove={() => setIndex(i())}
                onClick={() => choose(item)}
              >
                {item.icon()}
                <span class="pal-label">{item.label}</span>
                <Show when={item.detail}>
                  <small>{item.detail}</small>
                </Show>
              </button>
            )}
          </For>
          <Show when={!items().length}>
            <p class="pal-none">{files.loading ? "Looking through the project…" : "Nothing matches"}</p>
          </Show>
        </div>
      </div>
    </div>
  );
}
