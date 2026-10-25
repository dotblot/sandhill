// Development transport: the UI in an ordinary browser, talking to sandhilld
// through dev/bridge.mjs. Not used by the Mac app.
//
// The page stands in for one window. `?project=<folder>` opens a project on
// this machine; `?tab=<host>:<folder>`, once per tab, opens projects on
// several machines, and `?current=<host>` picks the one shown. The page
// keeps them in its address as they change, so reloading keeps them too.

import { type Backend, LOCAL_HOST, type LinkState, Listeners, type Place, type Shell, type WindowPlaces } from "./index";
import type { AppRequest, DaemonEvent, FileBytes, TerminalEvent } from "./types";

export class BrowserShell implements Shell {
  private params = new URLSearchParams(location.search);

  async context(): Promise<WindowPlaces> {
    const tabs: Place[] = this.params.getAll("tab").flatMap((tab) => {
      const colon = tab.indexOf(":");
      return colon > 0 ? [{ host: tab.slice(0, colon), path: tab.slice(colon + 1) }] : [];
    });
    const project = this.params.get("project");
    if (project && !tabs.some((t) => t.host === LOCAL_HOST)) tabs.unshift({ host: LOCAL_HOST, path: project });
    const current = this.params.get("current");
    if (current === "start") return { tabs, current: null };
    return { tabs, current: tabs.some((t) => t.host === current) ? current : (tabs[0]?.host ?? null) };
  }

  async savePlaces(places: WindowPlaces, title: string) {
    const query = new URLSearchParams();
    for (const tab of places.tabs) query.append("tab", `${tab.host}:${tab.path}`);
    query.set("current", places.current ?? "start");
    for (const keep of ["bridge", "servers"]) {
      const value = this.params.get(keep);
      if (value) query.set(keep, value);
    }
    history.replaceState(null, "", `?${query}`);
    document.title = title;
  }

  // The development bridge reaches only the daemon on this machine; a tab
  // for another host, like `?servers=gpu01`, talks to it under that name.
  async connect(host: string): Promise<Backend> {
    return new WebSocketBackend(this.params.get("bridge") ?? "127.0.0.1:4639", host);
  }

  async listServers(): Promise<string[]> {
    return (this.params.get("servers") ?? "").split(",").filter(Boolean);
  }

  async addServer(_host: string): Promise<string[]> {
    throw new Error("Servers can be added in the Mac app, not in the browser");
  }

  async removeServer(_host: string): Promise<string[]> {
    return [];
  }
}

interface Pending {
  resolve(value: unknown): void;
  reject(err: Error): void;
}

export class WebSocketBackend implements Backend {
  private socket?: WebSocket;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private queue: string[] = [];
  private events = new Listeners<DaemonEvent>();
  private terminals = new Listeners<TerminalEvent>();
  private requests = new Listeners<AppRequest>();
  private links = new Listeners<LinkState>();
  private link: LinkState = { status: "connecting" };
  private retryMs = 500;
  private retry?: ReturnType<typeof setTimeout>;
  /** Closes in a row without opening; after a few the link shows as disconnected. */
  private failures = 0;
  private stopped = false;

  constructor(
    private bridge: string,
    readonly host: string = LOCAL_HOST,
  ) {
    this.connect();
  }

  call<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    const id = this.nextId++;
    const line = JSON.stringify(params === undefined ? { id, method } : { id, method, params });
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(line);
      else this.queue.push(line);
    });
  }

  onEvent(listener: (event: DaemonEvent) => void) {
    this.events.add(listener);
  }

  onTerminal(listener: (event: TerminalEvent) => void) {
    this.terminals.add(listener);
  }

  onRequest(listener: (request: AppRequest) => void) {
    this.requests.add(listener);
  }

  onLink(listener: (state: LinkState) => void) {
    this.links.add(listener);
    listener(this.link);
  }

  async reconnect() {
    this.stopped = false;
    this.retryMs = 500;
    clearTimeout(this.retry);
    // Closing reconnects; with nothing open, connect now.
    if (this.socket && this.socket.readyState <= WebSocket.OPEN) this.socket.close();
    else this.connect();
  }

  async disconnect() {
    this.stopped = true;
    clearTimeout(this.retry);
    this.socket?.close();
    this.setLink({ status: "disconnected", stopped: true });
  }

  blobUrl(hash: string, mime: string, near?: string): string {
    const query = new URLSearchParams({ mime });
    if (near) query.set("near", near);
    return `http://${this.bridge}/blob/${hash}?${query}`;
  }

  openLink(url: string) {
    window.open(url, "_blank", "noopener");
  }

  /** Saves the file through the browser, read from the daemon in parts. */
  async download(path: string): Promise<string> {
    const parts: Uint8Array<ArrayBuffer>[] = [];
    let offset = 0;
    for (;;) {
      const part = await this.call<FileBytes>("read_bytes", { path, offset });
      const bytes = Uint8Array.from(atob(part.data), (c) => c.charCodeAt(0));
      parts.push(bytes);
      offset += bytes.length;
      if (!bytes.length || offset >= part.size) break;
    }
    const name = path.slice(path.lastIndexOf("/") + 1);
    const link = document.createElement("a");
    link.href = URL.createObjectURL(new Blob(parts));
    link.download = name;
    link.click();
    setTimeout(() => URL.revokeObjectURL(link.href), 10_000);
    return name;
  }

  private connect() {
    const socket = new WebSocket(`ws://${this.bridge}/`);
    this.socket = socket;
    socket.onopen = () => {
      this.retryMs = 500;
      this.failures = 0;
      for (const line of this.queue.splice(0)) socket.send(line);
      this.setLink({ status: "connected" });
    };
    socket.onmessage = (msg) => {
      const message = JSON.parse(String(msg.data));
      if (message.type === "event") {
        this.events.emit(message.event);
      } else if (message.type === "terminal") {
        this.terminals.emit(message.terminal);
      } else if (message.type === "request") {
        this.requests.emit(message.request);
      } else if (message.type === "response") {
        const pending = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (!pending) return;
        if ("error" in message) pending.reject(new Error(message.error));
        else pending.resolve(message.result);
      }
    };
    socket.onclose = () => {
      for (const p of this.pending.values()) p.reject(new Error("connection to the daemon closed"));
      this.pending.clear();
      if (this.socket !== socket || this.stopped) return;
      // Reconnecting for the first few tries, then disconnected, as in the Mac app.
      this.failures += 1;
      this.setLink({ status: this.failures < 3 ? "connecting" : "disconnected", error: "lost the connection to sandhilld" });
      this.retry = setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 5000);
    };
  }

  private setLink(state: LinkState) {
    if (state.status === this.link.status && !!state.stopped === !!this.link.stopped) return;
    this.link = state;
    this.links.emit(state);
  }
}
