// Which session each language of a document runs in. A document gets its
// own sessions by default (like a notebook's kernel), started in the
// document's folder the first time code in that language runs.

import { createSignal, type Setter } from "solid-js";
import { normalizeLanguage, pickKernel, type Workspace } from "../model/workspace";

export class DocKernels {
  readonly assigned: () => Record<string, string>;
  private setAssigned: Setter<Record<string, string>>;
  private starting = new Map<string, Promise<string>>();
  /** Languages with a kernel starting, as a signal for the kernel bar. */
  private startingNow: () => string[];
  private setStartingNow: Setter<string[]>;

  constructor(
    private ws: Workspace,
    private cwd: string,
    /** Kernel the document asks for, e.g. a notebook's kernelspec name. */
    private preferred: (language: string) => string | undefined = () => undefined,
  ) {
    [this.assigned, this.setAssigned] = createSignal<Record<string, string>>({});
    [this.startingNow, this.setStartingNow] = createSignal<string[]>([]);
  }

  session(language: string): string | undefined {
    const id = this.assigned()[normalizeLanguage(language)];
    return id && this.ws.liveSessions().some((s) => s.id === id) ? id : undefined;
  }

  assign(language: string, session: string) {
    this.setAssigned((a) => ({ ...a, [normalizeLanguage(language)]: session }));
  }

  /** The session for `language`, starting a kernel if there is none. */
  ensure(language: string): Promise<string> {
    const lang = normalizeLanguage(language);
    const existing = this.session(lang);
    if (existing) return Promise.resolve(existing);
    return this.starting.get(lang) ?? this.track(lang, this.start(lang));
  }

  isStarting(language: string): boolean {
    return this.startingNow().includes(normalizeLanguage(language));
  }

  startNew(language: string, kernel?: string): Promise<string> {
    const lang = normalizeLanguage(language);
    return this.track(lang, this.start(lang, kernel));
  }

  private track(language: string, pending: Promise<string>): Promise<string> {
    this.starting.set(language, pending);
    this.setStartingNow([...this.starting.keys()]);
    const done = () => {
      if (this.starting.get(language) === pending) this.starting.delete(language);
      this.setStartingNow([...this.starting.keys()]);
    };
    pending.then(done, done);
    return pending;
  }

  async restart(language: string) {
    const old = this.session(language);
    const kernel = this.ws.sessions().find((s) => s.id === old)?.kernel;
    if (old) await this.ws.shutdown(old).catch(() => {});
    await this.startNew(language, kernel);
  }

  async interrupt(language: string) {
    const id = this.session(language);
    if (id) await this.ws.interrupt(id);
  }

  private async start(language: string, kernel?: string): Promise<string> {
    const spec = kernel
      ? this.ws.kernelspecs().find((k) => k.name === kernel)
      : pickKernel(this.ws.kernelspecs(), language, this.preferred(language));
    if (!spec) throw new Error(`No ${language} kernel is installed where sandhilld runs`);
    const info = await this.ws.startSession(spec.name, this.cwd);
    this.assign(language, info.id);
    return info.id;
  }
}
