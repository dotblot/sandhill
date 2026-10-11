import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type Backend, type LinkState, Listeners } from "./index";
import type { DaemonEvent } from "./types";

export class TauriBackend implements Backend {
  private events = new Listeners<DaemonEvent>();
  private links = new Listeners<LinkState>();
  private link: LinkState = { status: "connecting" };

  static async create(): Promise<TauriBackend> {
    const backend = new TauriBackend();
    await listen<DaemonEvent>("daemon://event", (e) => backend.events.emit(e.payload));
    await listen<LinkState>("daemon://link", (e) => backend.setLink(e.payload));
    backend.setLink(await invoke<LinkState>("link_state"));
    return backend;
  }

  call<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    return invoke<T>("daemon_call", { method, params: params ?? null }).catch((err) => {
      throw new Error(String(err));
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
    return `${convertFileSrc(hash, "sandhill-blob")}?${query}`;
  }

  onMenu(listener: (command: string) => void) {
    void listen<string>("menu://command", (e) => listener(e.payload));
  }

  onCloseRequested(handler: () => Promise<boolean>) {
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
      getCurrentWindow().onCloseRequested(async (event) => {
        if (!(await handler())) event.preventDefault();
      }),
    );
  }

  private setLink(state: LinkState) {
    this.link = state;
    this.links.emit(state);
  }
}
