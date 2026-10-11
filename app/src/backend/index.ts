// How the UI reaches sandhilld. In the Mac app, the Tauri core holds the
// connections; during development in a browser, dev/bridge.mjs relays a
// WebSocket to the daemon. Both look the same to the rest of the UI.

import type { DaemonEvent } from "./types";

export type LinkStatus = "connecting" | "connected" | "disconnected";

export interface LinkState {
  status: LinkStatus;
  error?: string;
  /** What a slow connect is doing, such as installing sandhilld on a server. */
  detail?: string;
}

/** The daemon on this Mac. A server is named by its ssh destination, like `lab` or `me@host`. */
export const LOCAL_HOST = "local";

/** What a window shows: the start screen, or one project. */
export type WindowContext = { kind: "start" } | { kind: "project"; host: string; path: string };

/** One daemon, as one window sees it. */
export interface Backend {
  readonly host: string;
  call<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T>;
  /** Every event from every subscribed session. */
  onEvent(listener: (event: DaemonEvent) => void): void;
  /** Called with the current state right away and on every change. */
  onLink(listener: (state: LinkState) => void): void;
  /** A URL the webview can load a blob from, decoded to bytes for binary MIME types. */
  blobUrl(hash: string, mime: string, near?: string): string;
  /** Native menu commands (the Mac app only): "save", "close-tab", "run-all". */
  onMenu?(listener: (command: string) => void): void;
  /** Asked before the window closes; resolve false to keep it open. */
  onCloseRequested?(handler: () => Promise<boolean>): void;
}

/** The app around the windows: which window this is, and opening others. */
export interface Shell {
  context(): Promise<WindowContext>;
  connect(host: string): Promise<Backend>;
  /** Opens a project in its own window, or brings that window forward. */
  openProject(host: string, path: string): Promise<void>;
  showStart(): Promise<void>;
  /** Closes this window (the start screen, once a project is open). */
  closeWindow(): Promise<void>;
  /** Servers the start window offers besides this Mac. */
  listServers(): Promise<string[]>;
  /** Adds a server and returns the new list. */
  addServer(host: string): Promise<string[]>;
  removeServer(host: string): Promise<string[]>;
}

export async function createShell(): Promise<Shell> {
  if ("__TAURI_INTERNALS__" in window) {
    const { TauriShell } = await import("./tauri");
    return new TauriShell();
  }
  const { BrowserShell } = await import("./websocket");
  return new BrowserShell();
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
