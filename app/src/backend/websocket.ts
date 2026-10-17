// Development transport: the UI in an ordinary browser, talking to sandhilld
// through dev/bridge.mjs. Not used by the Mac app.
//
// The page stands in for one window: `?project=<folder>` is a project
// window, no project is the start screen. Opening a project navigates the
// page instead of opening a window.

import { type Backend, LOCAL_HOST, type LinkState, Listeners, type Shell, type WindowContext } from "./index";
import type { AppRequest, DaemonEvent, TerminalEvent } from "./types";

export class BrowserShell implements Shell {
  private params = new URLSearchParams(location.search);

  async context(): Promise<WindowContext> {
    const path = this.params.get("project");
    return path ? { kind: "project", host: LOCAL_HOST, path } : { kind: "start" };
  }

  async connect(_host: string): Promise<Backend> {
    return new WebSocketBackend(this.params.get("bridge") ?? "127.0.0.1:4639");
  }

  async openProject(_host: string, path: string) {
    location.search = this.query({ project: path });
  }

  async showStart() {
    location.search = this.query({});
  }

  async closeWindow() {
    // The page was already replaced by the project it opened.
  }

  // The development bridge reaches only the daemon on this machine;
  // `?servers=gpu01` lists it again under that name, to see a server's tab.
  async listServers(): Promise<string[]> {
    return (this.params.get("servers") ?? "").split(",").filter(Boolean);
  }

  async addServer(_host: string): Promise<string[]> {
    throw new Error("Servers can be added in the Mac app, not in the browser");
  }

  async removeServer(_host: string): Promise<string[]> {
    return [];
  }

  private query(extra: Record<string, string>): string {
    const query = new URLSearchParams(extra);
    const bridge = this.params.get("bridge");
    if (bridge) query.set("bridge", bridge);
    return query.toString();
  }
}

interface Pending {
  resolve(value: unknown): void;
  reject(err: Error): void;
}

export class WebSocketBackend implements Backend {
  readonly host = LOCAL_HOST;
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

  constructor(private bridge: string) {
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
