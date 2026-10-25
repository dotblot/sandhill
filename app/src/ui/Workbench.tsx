// A window: a tab per machine in the title bar, each with one project open,
// or the Start screen. Everything below the title bar belongs to the machine
// picked there, so local and remote work never mix. Projects opened in the
// window stay alive while hidden, so switching back returns to where you
// were: the same tabs, Console session, plot and conversation.

import { createEffect, createSignal, For, type JSX, onCleanup, onMount, Show, untrack } from "solid-js";
import type { Backend, Nav, Place, Shell, WindowPlaces } from "../backend";
import { closeTab, machineName, openPlace, placeKey, shownPlace, windowTitle } from "../model/places";
import { Workspace } from "../model/workspace";
import { App } from "./App";
import { ask, Dialogs } from "./dialogs";
import { StartScreen } from "./StartScreen";
import { EditCapsule, MachineTabs, ToolbarCapsule, TitleBar } from "./TitleBar";

/** What the Workbench needs from a project it shows. */
export interface PlaceApi {
  /** Asks about unsaved documents before the project closes; false to keep it open. */
  confirmClose(): Promise<boolean>;
  /** Whether an agent in the project waits for you. */
  waiting(): boolean;
  /** A menu bar command: "save", "save-all", "close-tab" or "run-all". */
  command(name: string): void;
}

interface Opened extends Place {
  key: string;
  ws: Workspace;
}

export function Workbench(props: { shell: Shell; initial: WindowPlaces }) {
  const [places, setPlaces] = createSignal<WindowPlaces>(props.initial);
  // One connection per machine, shared by its projects and the Start screen.
  const backends = new Map<string, Promise<Backend>>();
  const backendFor = (host: string) => {
    let backend = backends.get(host);
    if (!backend) backends.set(host, (backend = props.shell.connect(host)));
    return backend;
  };
  /** The machine's own workspace, for the Start screen. */
  const machines = new Map<string, Promise<Workspace>>();
  const machineFor = (host: string) => {
    let ws = machines.get(host);
    if (!ws) machines.set(host, (ws = backendFor(host).then((backend) => new Workspace(backend))));
    return ws;
  };

  /** Projects opened in this window, shown or not. */
  const [opened, setOpened] = createSignal<Opened[]>([]);
  const [apis, setApis] = createSignal<Readonly<Record<string, PlaceApi>>>({});
  const opening = new Set<string>();
  const ensure = (place: Place) => {
    const key = placeKey(place);
    if (opening.has(key) || untrack(opened).some((o) => o.key === key)) return;
    opening.add(key);
    void backendFor(place.host)
      .then((backend) => setOpened((list) => [...list, { ...place, key, ws: new Workspace(backend, place.path) }]))
      .catch((err) => console.error(`cannot connect to ${place.host}`, err))
      .finally(() => opening.delete(key));
  };
  createEffect(() => {
    for (const tab of places().tabs) ensure(tab);
  });
  const shownKey = () => {
    const shown = shownPlace(places());
    return shown ? placeKey(shown) : undefined;
  };
  const tabWorkspace = (host: string) => {
    const tab = places().tabs.find((t) => t.host === host);
    return tab ? opened().find((o) => o.key === placeKey(tab))?.ws : undefined;
  };
  const waiting = (host: string) =>
    opened().some((o) => o.host === host && apis()[o.key]?.waiting());

  // The window remembers its tabs for the next launch, and is named after the project shown.
  createEffect(() => {
    const now = places();
    void props.shell.savePlaces(now, windowTitle(now)).catch((err) => console.warn("could not remember the window's tabs", err));
  });

  const nav: Nav = {
    openProject: (host, path) => setPlaces((w) => openPlace(w, { host, path })),
    showStart: () => setPlaces((w) => ({ ...w, current: null })),
  };
  const pick = (host: string) => setPlaces((w) => ({ ...w, current: host }));

  /** Closes a machine's tab, once its projects have nothing unsaved. */
  async function closeMachine(host: string) {
    const name = machineName(host);
    const choice = await ask(`Close the ${name} tab?`, ["Close", "Cancel"], "Kernels and agents there keep running.");
    if (choice !== "Close") return;
    const going = opened().filter((o) => o.host === host);
    for (const o of going) if (!(await apis()[o.key]?.confirmClose())) return;
    setApis((all) => Object.fromEntries(Object.entries(all).filter(([key]) => !going.some((o) => o.key === key))));
    setOpened((list) => list.filter((o) => o.host !== host));
    setPlaces((w) => closeTab(w, host));
  }

  const machineTabs = (): JSX.Element => (
    <MachineTabs
      tabs={places().tabs.map((t) => ({ ...t, ws: tabWorkspace(t.host), waiting: waiting(t.host) }))}
      current={places().current}
      onPick={pick}
      onAdd={nav.showStart}
      onMenu={(host) => void closeMachine(host)}
    />
  );

  onMount(() => {
    props.shell.onMenu?.((command) => {
      if (command === "open-project" || command === "connect") return nav.showStart();
      const key = shownKey();
      if (key) apis()[key]?.command(command);
    });
    props.shell.onCloseRequested?.(async () => {
      for (const o of opened()) if (!(await apis()[o.key]?.confirmClose())) return false;
      return true;
    });
    // In a browser the menu bar's shortcuts are the page's.
    const onKey = (e: KeyboardEvent) => {
      if (props.shell.onMenu || !(e.metaKey || e.ctrlKey) || e.altKey || e.key.toLowerCase() !== "o") return;
      e.preventDefault();
      nav.showStart();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <div class="workbench">
      <For each={opened()}>
        {(o) => (
          <div class="place" classList={{ away: shownKey() !== o.key }} data-host={o.host}>
            <App
              ws={o.ws}
              shell={props.shell}
              nav={nav}
              project={o.path}
              current={shownKey() === o.key}
              machineTabs={machineTabs}
              report={(api) => setApis((all) => ({ ...all, [o.key]: api }))}
            />
          </div>
        )}
      </For>
      <Show when={places().current !== null && !opened().some((o) => o.key === shownKey())}>
        <div class="place">
          <div class="window">
            <TitleBar>
              <ToolbarCapsule />
              {machineTabs()}
              <EditCapsule buttons={{}} />
            </TitleBar>
            <p class="pane-note">Connecting to {machineName(places().current ?? "")}…</p>
          </div>
        </div>
      </Show>
      <Show when={places().current === null}>
        <div class="place">
          <StartScreen
            shell={props.shell}
            nav={nav}
            workspace={machineFor}
            titleBar={
              <TitleBar>
                <ToolbarCapsule />
                {machineTabs()}
                <EditCapsule buttons={{}} />
              </TitleBar>
            }
          />
        </div>
      </Show>
      <Dialogs />
    </div>
  );
}
