// Development transport: the UI in an ordinary browser, talking to sandhilld
// through dev/bridge.mjs. Not used by the Mac app.

import { type Backend, type LinkState, Listeners } from "./index";
import type { DaemonEvent } from "./types";

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
  private links = new Listeners<LinkState>();
  private link: LinkState = { status: "connecting" };
  private retryMs = 500;

  constructor(private host: string) {
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

  onLink(listener: (state: LinkState) => void) {
    this.links.add(listener);
    listener(this.link);
  }

  blobUrl(hash: string, mime: string, near?: string): string {
    const query = new URLSearchParams({ mime });
    if (near) query.set("near", near);
    return `http://${this.host}/blob/${hash}?${query}`;
  }

  private connect() {
    const socket = new WebSocket(`ws://${this.host}/`);
    this.socket = socket;
    socket.onopen = () => {
      this.retryMs = 500;
      for (const line of this.queue.splice(0)) socket.send(line);
      this.setLink({ status: "connected" });
    };
    socket.onmessage = (msg) => {
      const message = JSON.parse(String(msg.data));
      if (message.type === "event") {
        this.events.emit(message.event);
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
      this.setLink({ status: "disconnected", error: "lost the connection to sandhilld" });
      setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 5000);
    };
  }

  private setLink(state: LinkState) {
    if (state.status === this.link.status) return;
    this.link = state;
    this.links.emit(state);
  }
}
