// The title bar (31px): after the window buttons, the toolbar capsule; in
// the middle of the window, a tab per machine; at the right, the edit
// capsule. Capsules are fully rounded glass bars, as in Safari on macOS 26.
// Empty space drags the window.

import { Index, type JSX, Show } from "solid-js";
import { LOCAL_HOST } from "../backend";
import { machineName, projectName, shortPath } from "../model/places";
import type { Workspace } from "../model/workspace";
import { ConnIcon, stateWord } from "./Connection";
import { Icon, type IconName, type V32IconId } from "./icons";

export function TitleBar(props: { children: JSX.Element }) {
  return (
    <div class="titlebar" data-tauri-drag-region>
      {props.children}
    </div>
  );
}

export function Capsule(props: { class: string; label: string; children: JSX.Element }) {
  return (
    <div class={`cap ${props.class}`} role="toolbar" aria-label={props.label}>
      {props.children}
    </div>
  );
}

/** A thin line between groups of buttons in a capsule. */
export function CapDivider() {
  return <span class="capdiv" />;
}

/** A round 28 × 21 button in a capsule. Edit buttons keep the focus where it was, in the editor. */
export function CapButton(props: {
  icon: IconName | V32IconId;
  title: string;
  onClick?: () => void;
  pressed?: boolean;
  disabled?: boolean;
  /** Opens a menu: `pressed` says whether it is open. */
  menu?: boolean;
  dot?: boolean;
  keepFocus?: boolean;
  ref?: (el: HTMLButtonElement) => void;
}) {
  return (
    <button
      class="tbi"
      ref={props.ref}
      title={props.title}
      disabled={props.disabled || !props.onClick}
      aria-pressed={props.menu || props.pressed === undefined ? undefined : props.pressed}
      aria-expanded={props.menu ? !!props.pressed : undefined}
      onMouseDown={(e) => props.keepFocus && e.preventDefault()}
      onClick={() => props.onClick?.()}
    >
      <Icon name={props.icon} />
      <Show when={props.dot}>
        <span class="dot" />
      </Show>
    </button>
  );
}

/** A machine's tab: its project, its connection, and whether an agent there waits for you. */
export interface MachineTab {
  host: string;
  path: string;
  /** The tab's connection, once it has one. */
  ws?: Workspace;
  waiting: boolean;
}

/**
 * One tab per machine, centred in the window, then + to connect to another
 * (the Start screen). The picked tab is a raised pill; on the Start screen
 * none is.
 */
export function MachineTabs(props: {
  tabs: MachineTab[];
  current: string | null;
  onPick: (host: string) => void;
  onAdd: () => void;
  /** Right-click: closing the tab. */
  onMenu?: (host: string, e: MouseEvent) => void;
}) {
  const title = (tab: MachineTab) =>
    [
      tab.host === LOCAL_HOST ? "This Mac" : tab.host,
      tab.path,
      tab.ws && stateWord(tab.ws),
      tab.waiting && "an agent is waiting for you",
    ]
      .filter(Boolean)
      .join(" · ");
  return (
    <div class="cap captabs">
      <div class="mtabs" role="tablist" aria-label="Machines">
        <Index each={props.tabs}>
          {(tab) => (
            <button
              class="mtab"
              role="tab"
              aria-selected={tab().host === props.current}
              title={title(tab())}
              onClick={() => props.onPick(tab().host)}
              onContextMenu={(e) => {
                if (!props.onMenu) return;
                e.preventDefault();
                props.onMenu(tab().host, e);
              }}
            >
              <span class="mt-ic">
                <ConnIcon host={tab().host} ws={tab().ws} />
              </span>
              <span class="mt-n">{machineName(tab().host)}</span>
              <span class="mt-p">{shortPath(tab().path, tab().ws?.host()?.home) || projectName(tab().path)}</span>
              <Show when={tab().waiting && tab().host !== props.current}>
                <span class="dot wait" />
              </Show>
            </button>
          )}
        </Index>
      </div>
      <Show when={props.tabs.length}>
        <CapDivider />
      </Show>
      <CapButton icon="plus" title="Connect to server ⇧⌘O" onClick={props.onAdd} />
    </div>
  );
}

/** The left capsule: the left pane, Agents, the right pane | search and the menu. */
export function ToolbarCapsule(props: {
  left?: boolean;
  agents?: boolean;
  waiting?: boolean;
  right?: boolean;
  menu?: boolean;
  onLeft?: () => void;
  onAgents?: () => void;
  onRight?: () => void;
  onSearch?: () => void;
  onMenu?: () => void;
  menuRef?: (el: HTMLButtonElement) => void;
}) {
  return (
    <Capsule class="tb-left" label="Panes">
      <CapButton icon="sidebar.left" title="Left pane: files, kernels and resources ⌘B" pressed={props.left} onClick={props.onLeft} />
      <CapButton
        icon="chat"
        title={props.waiting ? "Agents ⌘I: an agent is waiting for you" : "Agents: Claude Code, Codex and Antigravity ⌘I"}
        pressed={props.agents}
        onClick={props.onAgents}
        dot={props.waiting}
      />
      <CapButton icon="sidebar.right" title="Right pane: Variables, Help, Packages, Git, Plots and Viewer ⌥⌘B" pressed={props.right} onClick={props.onRight} />
      <CapDivider />
      <CapButton icon="search" title="Go to a file or run a command ⌘K" onClick={props.onSearch} />
      <CapButton icon="menu" title="Menu" menu pressed={props.menu} onClick={props.onMenu} ref={props.menuRef} />
    </Capsule>
  );
}

/** What the edit capsule can do now. */
export interface EditButtons {
  undo?: () => void;
  redo?: () => void;
  cut?: () => void;
  copy?: () => void;
  paste?: () => void;
  save?: () => void;
  saveAll?: () => void;
  /** The document Save saves, for its tooltip. */
  saveName?: string;
  /** The documents with unsaved changes, for Save all's tooltip. */
  unsaved?: string[];
}

/** The right capsule: undo, redo | cut, copy, paste | save, save all. Buttons that can't act are dimmed. */
export function EditCapsule(props: { buttons: EditButtons }) {
  const b = () => props.buttons;
  const saveTitle = () => {
    const name = b().saveName;
    if (!name) return "Save ⌘S (no document open)";
    return b().save ? `Save ${name} ⌘S` : `Save ${name} ⌘S (no unsaved changes)`;
  };
  const saveAllTitle = () => {
    const names = b().unsaved ?? [];
    return names.length ? `Save all: ${names.join(", ")} ⌥⌘S` : "Save all ⌥⌘S (nothing unsaved)";
  };
  return (
    <Capsule class="editcap" label="Edit">
      <CapButton icon="edit.undo" title="Undo ⌘Z" onClick={b().undo} keepFocus />
      <CapButton icon="edit.redo" title="Redo ⇧⌘Z" onClick={b().redo} keepFocus />
      <CapDivider />
      <CapButton icon="edit.cut" title="Cut ⌘X" onClick={b().cut} keepFocus />
      <CapButton icon="edit.copy" title="Copy ⌘C" onClick={b().copy} keepFocus />
      <CapButton icon="edit.paste" title="Paste ⌘V" onClick={b().paste} keepFocus />
      <CapDivider />
      <CapButton icon="doc.save" title={saveTitle()} onClick={b().save} keepFocus />
      <CapButton icon="doc.save-all" title={saveAllTitle()} onClick={b().saveAll} keepFocus />
    </Capsule>
  );
}
