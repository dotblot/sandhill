// How the UI reaches sandhilld. In the Mac app, the Tauri core holds the
// connections; during development in a browser, dev/bridge.mjs relays a
// WebSocket to the daemon. Both look the same to the rest of the UI.

import type { AppRequest, DaemonEvent, TerminalEvent } from "./types";

export type LinkStatus = "connecting" | "connected" | "disconnected";

export interface LinkState {
  status: LinkStatus;
  error?: string;
  /** What a slow connect is doing, such as installing sandhilld on a server. */
  detail?: string;
  /** Disconnected on request: it waits for `reconnect`. */
  stopped?: boolean;
}

/** The daemon on this Mac. A server is named by its ssh destination, like `lab` or `me@host`. */
export const LOCAL_HOST = "local";

/** A project on a machine. */
export interface Place {
  host: string;
  path: string;
}

/**
 * What a window has open: one tab per machine, each with its project, and
 * the machine shown, or none for the Start screen.
 */
export interface WindowPlaces {
  tabs: Place[];
  current: string | null;
}

/** One daemon, as one window sees it. */
export interface Backend {
  readonly host: string;
  call<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T>;
  /** Every event from every subscribed session. */
  onEvent(listener: (event: DaemonEvent) => void): void;
  /** Output and news from every subscribed terminal. */
  onTerminal(listener: (event: TerminalEvent) => void): void;
  /** Requests for windows, such as an agent's edit to an open document; answered with `answer`. */
  onRequest(listener: (request: AppRequest) => void): void;
  /** Called with the current state right away and on every change. */
  onLink(listener: (state: LinkState) => void): void;
  /** Drops the connection and connects again at once. */
  reconnect(): Promise<void>;
  /** Drops the connection until `reconnect`. */
  disconnect(): Promise<void>;
  /** A URL the webview can load a blob from, decoded to bytes for binary MIME types. */
  blobUrl(hash: string, mime: string, near?: string): string;
  /** Opens a web link in the browser. */
  openLink(url: string): void;
  /** Shows a file in Finder (in the app; the UI offers it for this Mac's files). */
  reveal?(path: string): Promise<void>;
  /** Copies a file of the daemon's machine to this Mac (to Downloads), and says where it went. */
  download?(path: string): Promise<string>;
}

/** The app around the windows: what this window had open, and its menu bar. */
export interface Shell {
  context(): Promise<WindowPlaces>;
  /** Remembers what the window has open, for the next launch, and names the window after it. */
  savePlaces(places: WindowPlaces, title: string): Promise<void>;
  connect(host: string): Promise<Backend>;
  /** Servers the Start screen offers besides this Mac. */
  listServers(): Promise<string[]>;
  /** Adds a server and returns the new list. */
  addServer(host: string): Promise<string[]>;
  removeServer(host: string): Promise<string[]>;
  /**
   * Native menu commands (the Mac app only): "save", "save-all", "close-tab",
   * "run-all", "open-project" and "connect".
   */
  onMenu?(listener: (command: string) => void): void;
  /** Asked before the window closes; resolve false to keep it open. */
  onCloseRequested?(handler: () => Promise<boolean>): void;
}

/** Moving around inside a window, which the Workbench does. */
export interface Nav {
  /** Shows a project, in its machine's tab: the tab switches to it if it showed another. */
  openProject(host: string, path: string): void;
  /** Shows the Start screen, to open a project or connect to a server. */
  showStart(): void;
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
