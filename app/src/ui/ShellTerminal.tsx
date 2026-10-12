import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import type { TerminalStore } from "../model/terminals";
import { notify } from "./dialogs";
import { TermView } from "./termView";

export interface ShellApi {
  /** Shows the terminal and runs `command` in it. */
  run(command: string): Promise<void>;
}

/**
 * The Console's Terminal tab: a login shell in the project's folder on the
 * daemon's machine. Like the agents, it keeps running when the window
 * closes, and comes back when the project opens again.
 */
export function ShellTerminal(props: {
  store: TerminalStore;
  shown: boolean;
  openLink: (url: string) => void;
  api?: (api: ShellApi) => void;
}) {
  const store = props.store;
  const view = new TermView(store, props.openLink);
  const [starting, setStarting] = createSignal(false);
  let host!: HTMLDivElement;
  const shell = () => {
    const shells = store.shells();
    return shells.find((s) => s.running) ?? shells[shells.length - 1];
  };

  async function ensure(): Promise<string | undefined> {
    await view.mount(host);
    const current = shell();
    if (current?.running) return current.id;
    if (starting()) return undefined;
    setStarting(true);
    try {
      // The ended one goes, so its output doesn't come back next time.
      if (current) await store.close(current.id).catch(() => {});
      view.term.reset();
      const info = await store.start("shell", {}, view.size());
      return info.id;
    } catch (err) {
      notify(`Could not open a terminal: ${err instanceof Error ? err.message : String(err)}`);
      return undefined;
    } finally {
      setStarting(false);
    }
  }

  // Opening the tab starts a shell the first time.
  createEffect(
    on(
      () => props.shown,
      (shown) => {
        if (!shown) return;
        void view.mount(host).then(() => {
          const current = shell();
          if (current) view.bind(current.id);
          else void ensure().then((id) => id && view.bind(id));
          view.focus();
        });
      },
    ),
  );
  // A shell found on the daemon after the tab opened, as after a reconnect.
  createEffect(() => {
    const current = shell();
    if (current && props.shown) void view.mount(host).then(() => view.bind(current.id));
  });
  onCleanup(() => view.dispose());

  props.api?.({
    async run(command: string) {
      const id = await ensure();
      if (!id) return;
      view.bind(id);
      await store.input(id, `${command}\r`);
      view.focus();
    },
  });

  return (
    <div class="shell-terminal" hidden={!props.shown}>
      <div class="term-host" ref={host} />
      <Show when={shell() && !shell()!.running}>
        <div class="agent-ended">
          <span>The terminal ended.</span>
          <span class="spacer" />
          <button onClick={() => void ensure().then((id) => id && view.bind(id))}>New terminal</button>
        </div>
      </Show>
    </div>
  );
}
