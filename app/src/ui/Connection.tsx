// A machine's connection, as the status bar and the left pane's machine tabs
// show it (draft 19): the machine's icon in the link's state, and a pop-up
// with what the connection is, Reconnect, Disconnect or Restart, and the
// daemon's log.

import { createResource, createSignal, For, onCleanup, Show } from "solid-js";
import { LOCAL_HOST } from "../backend";
import type { LogTail } from "../backend/types";
import { BUILD } from "../build";
import { languageLabel } from "../model/machine";
import type { Workspace } from "../model/workspace";
import { notify } from "./dialogs";
import { Icon, type IconName } from "./icons";
import { closeOnOutside } from "./popup";

/** Connected, reconnecting, or disconnected. */
export type ConnState = "on" | "busy" | "off";

export function connState(ws: Workspace): ConnState {
  const status = ws.link().status;
  return status === "connected" ? "on" : status === "connecting" ? "busy" : "off";
}

/**
 * The machine's icon, a screen or a cloud, showing its link: plain when
 * connected, orange and pulsing while reconnecting, red and struck through
 * when disconnected. Without a workspace, plain.
 */
export function ConnIcon(props: { host: string; ws?: Workspace }) {
  const state = () => (props.ws ? connState(props.ws) : "on");
  return (
    <span class="conn-ico" classList={{ busy: state() === "busy", off: state() === "off" }}>
      <Icon name={props.host === LOCAL_HOST ? "monitor" : "cloud"} />
    </span>
  );
}

/** "running" (this Mac), "connected", "connecting", "reconnecting" or "disconnected". */
export function stateWord(ws: Workspace): string {
  const state = connState(ws);
  if (state === "on") return ws.backend.host === LOCAL_HOST ? "running" : "connected";
  if (state === "busy") return ws.link().error ? "reconnecting" : "connecting";
  return "disconnected";
}

/** The status bar's first item: the machine picked in the left pane, and its pop-up. */
export function ConnectionItem(props: { ws: Workspace }) {
  const [open, setOpen] = createSignal(false);
  const [log, setLog] = createSignal<Workspace>();
  const local = () => props.ws.backend.host === LOCAL_HOST;
  const title = () => {
    const version = props.ws.host()?.daemon_version;
    return [local() ? "This Mac" : props.ws.backend.host, version && `sandhilld ${version}`, stateWord(props.ws)].filter(Boolean).join(" · ");
  };
  let button!: HTMLButtonElement;
  return (
    <>
      <button
        ref={button}
        class="status-item conn"
        classList={{ off: connState(props.ws) === "off" }}
        title={title()}
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <ConnIcon host={props.ws.backend.host} ws={props.ws} />
        {local() ? "Local" : props.ws.backend.host}
      </button>
      <Show when={open()}>
        <ConnectionPopup
          ws={props.ws}
          anchor={button}
          onClose={() => setOpen(false)}
          onLog={() => {
            setOpen(false);
            setLog(props.ws);
          }}
        />
      </Show>
      <Show when={log()}>{(ws) => <LogDialog ws={ws()} onClose={() => setLog(undefined)} />}</Show>
    </>
  );
}

/** Above the status bar item: what the connection is, and what can be done to it. */
function ConnectionPopup(props: { ws: Workspace; anchor: HTMLElement; onClose: () => void; onLog: () => void }) {
  const local = () => props.ws.backend.host === LOCAL_HOST;
  const state = () => connState(props.ws);
  const [now, setNow] = createSignal(Date.now());
  const [latency, setLatency] = createSignal<number>();
  // A ping's round trip, now and every few seconds while the pop-up is open.
  const measure = async () => {
    if (state() !== "on") return setLatency(undefined);
    const start = performance.now();
    try {
      await props.ws.call("ping");
      setLatency(Math.round(performance.now() - start));
    } catch {
      setLatency(undefined);
    }
  };
  void measure();
  const clock = setInterval(() => setNow(Date.now()), 1000);
  const pinger = setInterval(() => void measure(), 5000);
  onCleanup(() => {
    clearInterval(clock);
    clearInterval(pinger);
  });
  let popup!: HTMLDivElement;
  closeOnOutside(() => popup, props.anchor, props.onClose);
  const box = props.anchor.getBoundingClientRect();

  const rows = (): [string, string][] => {
    const info = props.ws.host();
    const rows: [string, string][] = [];
    if (!local()) {
      const { user, address } = destination(props.ws.backend.host);
      const named = info?.hostname && info.hostname.split(".")[0] !== address.split(".")[0] ? ` (${info.hostname})` : "";
      rows.push(["Address", address + named]);
      rows.push(["Via", `ssh, as ${info?.user || user || "you"}`]);
    }
    if (info) rows.push(["sandhilld", [info.daemon_version, startedBy(info.started_by)].filter(Boolean).join(" · ")]);
    if (state() === "off") {
      const error = props.ws.link().error;
      if (error) rows.push(["Error", error]);
    } else {
      const since = local() ? info?.started_ms || undefined : props.ws.connectedSince();
      if (since && state() === "on") rows.push([local() ? "Running for" : "Connected for", duration(now() - since)]);
      if (!local() && latency() !== undefined) rows.push(["Latency", `${latency()} ms`]);
      const kernels = props.ws.liveSessions().length;
      rows.push(["Kernels", kernels ? `${kernels} running` : "none running"]);
    }
    // This app's build, to tell an update arrived.
    rows.push(["Sandhill", `${BUILD.when}${BUILD.commit ? ` · ${BUILD.commit}` : ""}`]);
    return rows;
  };

  const act = (action: () => Promise<unknown>, failed: string) => {
    props.onClose();
    action().catch((e) => notify(`${failed}: ${e instanceof Error ? e.message : String(e)}`));
  };
  const actions = (): { icon: IconName; label: string; run: () => void }[] => {
    const ws = props.ws;
    if (local() && state() === "on") {
      // Kernels and agents keep running; the link starts a new daemon.
      return [{ icon: "restart", label: "Restart sandhilld", run: () => act(() => ws.call("stop_daemon"), "Could not restart sandhilld") }];
    }
    if (state() === "off" && (local() || ws.link().stopped)) {
      return [{ icon: local() ? "restart" : "cloud", label: "Connect", run: () => act(() => ws.backend.reconnect(), "Could not connect") }];
    }
    return [
      { icon: "restart", label: "Reconnect", run: () => act(() => ws.backend.reconnect(), "Could not reconnect") },
      { icon: "x", label: "Disconnect", run: () => act(() => ws.backend.disconnect(), "Could not disconnect") },
    ];
  };

  return (
    <div
      ref={popup}
      class="kpop connpop"
      role="dialog"
      aria-label="Connection"
      style={{ left: `${Math.max(8, box.left)}px`, bottom: `${window.innerHeight - box.top + 6}px` }}
    >
      <div class="kp-h">
        <ConnIcon host={props.ws.backend.host} ws={props.ws} />
        <b>{local() ? "This Mac" : props.ws.backend.host}</b>
        <span class="kp-st">{stateWord(props.ws)}</span>
      </div>
      <dl class="kp-d">
        <For each={rows()}>
          {([name, value]) => (
            <>
              <dt>{name}</dt>
              <dd title={value}>{value}</dd>
            </>
          )}
        </For>
      </dl>
      <div class="mm-sep" />
      <For each={actions()}>
        {(a) => (
          <button class="mm-i" onClick={a.run}>
            <Icon name={a.icon} small />
            <span>{a.label}</span>
            <kbd />
          </button>
        )}
      </For>
      <button class="mm-i" disabled={state() !== "on"} onClick={props.onLog}>
        <Icon name="file" small />
        <span>Open the sandhilld log</span>
        <kbd />
      </button>
    </div>
  );
}

/** The end of a daemon's log, scrolled to its last line. */
function LogDialog(props: { ws: Workspace; onClose: () => void }) {
  const [tail, { refetch }] = createResource(() => props.ws.call<LogTail>("read_log", { bytes: 256 * 1024 }));
  let text: HTMLPreElement | undefined;
  const scrollDown = (el: HTMLPreElement) => {
    text = el;
    queueMicrotask(() => (el.scrollTop = el.scrollHeight));
  };
  return (
    <div class="modal-backdrop" onKeyDown={(e) => e.key === "Escape" && props.onClose()} onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="modal log-modal" role="dialog" aria-modal="true" aria-label="sandhilld log">
        <p class="modal-message">
          sandhilld log on {props.ws.backend.host === LOCAL_HOST ? "this Mac" : props.ws.backend.host}
        </p>
        <p class="modal-detail" title={tail()?.path}>
          {tail.error ? String(tail.error) : (tail()?.path ?? "Loading…")}
        </p>
        <Show when={tail()}>
          {(t) => (
            <pre class="log-text" ref={scrollDown}>
              {plain(t().text) || "The log is empty."}
            </pre>
          )}
        </Show>
        <div class="modal-buttons">
          <button
            onClick={() => {
              void Promise.resolve(refetch()).then(() => text && queueMicrotask(() => (text!.scrollTop = text!.scrollHeight)));
            }}
          >
            Refresh
          </button>
          <button class="primary" onClick={props.onClose} autofocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

/** The kernels running on a machine other than the window's own, as status bar items. */
export function HostKernelStates(props: { ws: Workspace }) {
  return (
    <For each={props.ws.liveSessions()}>
      {(s) => {
        const busy = () => props.ws.isBusy(s.id);
        return (
          <span class="status-item" data-state={busy() ? "running" : "idle"}>
            <Show when={!busy()} fallback={<span class="spin" />}>
              <Icon name="sessions" small />
            </Show>
            {languageLabel(s.language)} {busy() ? "running" : "idle"}
          </span>
        );
      }}
    </For>
  );
}

/** Without the colour codes older daemons wrote into their log. */
function plain(text: string): string {
  // biome-ignore lint/suspicious/noControlCharactersInRegex: ANSI escapes start with ESC.
  return text.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");
}

/** "app" → "started by Sandhill", and so on; nothing from an older daemon. */
function startedBy(who: string | undefined): string {
  if (who === undefined) return "";
  return { app: "started by Sandhill", launchd: "launchd agent", systemd: "systemd user service" }[who] ?? "started by hand";
}

/** An ssh destination's user and address: "me@gpu01" → me, gpu01. */
export function destination(host: string): { user?: string; address: string } {
  const at = host.lastIndexOf("@");
  return at < 0 ? { address: host } : { user: host.slice(0, at), address: host.slice(at + 1) };
}

/** "40 s", "12 min", "3 h 12 min", "2 d 4 h". */
export function duration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ${minutes % 60} min`;
  return `${Math.floor(hours / 24)} d ${hours % 24} h`;
}
