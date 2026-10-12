import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type Backend, type LinkState, Listeners, type Shell, type WindowContext } from "./index";
import type { DaemonEvent, TerminalEvent } from "./types";

export class TauriShell implements Shell {
  context(): Promise<WindowContext> {
    return invoke<WindowContext>("window_context");
  }

  connect(host: string): Promise<Backend> {
    return TauriBackend.create(host);
  }

  openProject(host: string, path: string): Promise<void> {
    return invoke("open_project_window", { host, path });
  }

  showStart(): Promise<void> {
    return invoke("show_start_window");
  }

  closeWindow(): Promise<void> {
    // The start screen has nothing to confirm, so no close request.
    return getCurrentWindow().destroy();
  }

  listServers(): Promise<string[]> {
    return invoke("list_servers");
  }

  addServer(host: string): Promise<string[]> {
    return invoke<string[]>("add_server", { host }).catch((err) => {
      throw new Error(String(err));
    });
  }

  removeServer(host: string): Promise<string[]> {
    return invoke("remove_server", { host });
  }
}

class TauriBackend implements Backend {
  private events = new Listeners<DaemonEvent>();
  private terminals = new Listeners<TerminalEvent>();
  private links = new Listeners<LinkState>();
  private link: LinkState = { status: "connecting" };

  private constructor(readonly host: string) {}

  static async create(host: string): Promise<TauriBackend> {
    const backend = new TauriBackend(host);
    // The app sends a host's events only to the windows using it.
    await listen<DaemonEvent>("daemon://event", (e) => backend.events.emit(e.payload));
    await listen<TerminalEvent>("daemon://terminal", (e) => backend.terminals.emit(e.payload));
    await listen<LinkState & { host: string }>("daemon://link", (e) => {
      if (e.payload.host === host) backend.setLink(e.payload);
    });
    backend.setLink(await invoke<LinkState>("use_host", { host }));
    return backend;
  }

  call<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    return invoke<T>("daemon_call", { host: this.host, method, params: params ?? null }).catch((err) => {
      throw new Error(String(err));
    });
  }

  onEvent(listener: (event: DaemonEvent) => void) {
    this.events.add(listener);
  }

  onTerminal(listener: (event: TerminalEvent) => void) {
    this.terminals.add(listener);
  }

  onLink(listener: (state: LinkState) => void) {
    this.links.add(listener);
    listener(this.link);
  }

  blobUrl(hash: string, mime: string, near?: string): string {
    const query = new URLSearchParams({ host: this.host, mime });
    if (near) query.set("near", near);
    return `${convertFileSrc(hash, "sandhill-blob")}?${query}`;
  }

  openLink(url: string) {
    invoke("open_link", { url }).catch((err) => console.warn(err));
  }

  onMenu(listener: (command: string) => void) {
    void listen<string>("menu://command", (e) => listener(e.payload));
  }

  onCloseRequested(handler: () => Promise<boolean>) {
    void getCurrentWindow().onCloseRequested(async (event) => {
      if (await handler()) return;
      event.preventDefault();
      // If this close was part of Quit, the other windows stay too.
      await invoke("cancel_quit");
    });
  }

  private setLink(state: LinkState) {
    this.link = { status: state.status, error: state.error, detail: state.detail };
    this.links.emit(this.link);
  }
}
