import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type Backend, type LinkState, Listeners, type Shell, type WindowPlaces } from "./index";
import type { AppRequest, DaemonEvent, TerminalEvent } from "./types";

export class TauriShell implements Shell {
  context(): Promise<WindowPlaces> {
    return invoke<WindowPlaces>("window_context");
  }

  savePlaces(places: WindowPlaces, title: string): Promise<void> {
    return invoke("set_window_places", { places, title });
  }

  connect(host: string): Promise<Backend> {
    return TauriBackend.create(host);
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
}

/** A daemon message as the app forwards it: which machine it came from, and the message. */
interface Hosted<T> {
  host: string;
  body: T;
}

class TauriBackend implements Backend {
  private events = new Listeners<DaemonEvent>();
  private terminals = new Listeners<TerminalEvent>();
  private requests = new Listeners<AppRequest>();
  private links = new Listeners<LinkState>();
  private link: LinkState = { status: "connecting" };

  private constructor(readonly host: string) {}

  static async create(host: string): Promise<TauriBackend> {
    const backend = new TauriBackend(host);
    // The app sends a host's events only to the windows using it; a window
    // with a tab per machine gets every one of its machines' events.
    await listen<Hosted<DaemonEvent>>("daemon://event", (e) => e.payload.host === host && backend.events.emit(e.payload.body));
    await listen<Hosted<TerminalEvent>>("daemon://terminal", (e) => e.payload.host === host && backend.terminals.emit(e.payload.body));
    await listen<Hosted<AppRequest>>("daemon://request", (e) => e.payload.host === host && backend.requests.emit(e.payload.body));
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

  onRequest(listener: (request: AppRequest) => void) {
    this.requests.add(listener);
  }

  onLink(listener: (state: LinkState) => void) {
    this.links.add(listener);
    listener(this.link);
  }

  reconnect(): Promise<void> {
    return invoke("reconnect_host", { host: this.host });
  }

  disconnect(): Promise<void> {
    return invoke("disconnect_host", { host: this.host });
  }

  blobUrl(hash: string, mime: string, near?: string): string {
    const query = new URLSearchParams({ host: this.host, mime });
    if (near) query.set("near", near);
    return `${convertFileSrc(hash, "sandhill-blob")}?${query}`;
  }

  openLink(url: string) {
    invoke("open_link", { url }).catch((err) => console.warn(err));
  }

  reveal(path: string): Promise<void> {
    return invoke<void>("reveal_path", { path });
  }

  download(path: string): Promise<string> {
    return invoke<string>("download_file", { host: this.host, path });
  }

  private setLink(state: LinkState) {
    this.link = { status: state.status, error: state.error, detail: state.detail, stopped: state.stopped };
    this.links.emit(this.link);
  }
}
