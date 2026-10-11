// How the UI reaches sandhilld. In the Mac app, the Tauri core holds the
// connection; during development in a browser, dev/bridge.mjs relays a
// WebSocket to the daemon. Both look the same to the rest of the UI.

import type { DaemonEvent } from "./types";

export type LinkStatus = "connecting" | "connected" | "disconnected";

export interface LinkState {
  status: LinkStatus;
  error?: string;
}

export interface Backend {
  call<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T>;
  /** Every event from every subscribed session. */
  onEvent(listener: (event: DaemonEvent) => void): void;
  /** Called with the current state right away and on every change. */
  onLink(listener: (state: LinkState) => void): void;
  /** A URL the webview can load a blob from, decoded to bytes for binary MIME types. */
  blobUrl(hash: string, mime: string, near?: string): string;
}

export async function createBackend(): Promise<Backend> {
  if ("__TAURI_INTERNALS__" in window) {
    const { TauriBackend } = await import("./tauri");
    return TauriBackend.create();
  }
  const { WebSocketBackend } = await import("./websocket");
  const bridge = new URLSearchParams(location.search).get("bridge") ?? "127.0.0.1:4639";
  return new WebSocketBackend(bridge);
}

/** Small helper both backends use to fan out callbacks. */
export class Listeners<T> {
  private items = new Set<(value: T) => void>();
  add(listener: (value: T) => void) {
    this.items.add(listener);
  }
  emit(value: T) {
    for (const listener of this.items) {
      try {
        listener(value);
      } catch (err) {
        console.error(err);
      }
    }
  }
}
