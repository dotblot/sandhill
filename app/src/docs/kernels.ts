// Which session each language of a document runs in. The first time code
// in a language runs, the document joins a session of its project already
// running in the same folder (as RStudio's scripts share one console), or
// starts one there. A document that names its kernel, like a notebook, only
// joins sessions of that kernel. "New …" in the kernel bar starts a separate
// session for one document.

import { createSignal, type Setter } from "solid-js";
import type { KernelspecInfo } from "../backend/types";
import { type KernelWanted, normalizeLanguage, pickKernel, type Workspace } from "../model/workspace";

export class DocKernels {
  readonly assigned: () => Record<string, string>;
  private setAssigned: Setter<Record<string, string>>;
  private starting = new Map<string, Promise<string>>();
  /** Languages with a kernel starting, as a signal for the kernel bar. */
  private startingNow: () => string[];
  private setStartingNow: Setter<string[]>;

  constructor(
    private ws: Workspace,
    /** Where its sessions run, which a project setting can change. */
    private cwd: () => string,
    /** Kernel the document asks for, e.g. a notebook's kernelspec. */
    private preferred: (language: string) => KernelWanted | undefined = () => undefined,
  ) {
    [this.assigned, this.setAssigned] = createSignal<Record<string, string>>({});
    [this.startingNow, this.setStartingNow] = createSignal<string[]>([]);
  }

  session(language: string): string | undefined {
    const assigned = this.assigned()[normalizeLanguage(language)];
    const id = assigned && this.ws.current(assigned);
    return id && this.ws.liveSessions().some((s) => s.id === id) ? id : undefined;
  }

  /** The running sessions this document uses, one per language at most. */
  sessions(): string[] {
    return Object.keys(this.assigned())
      .map((language) => this.session(language))
      .filter((id): id is string => !!id);
  }

  assign(language: string, session: string) {
    this.setAssigned((a) => ({ ...a, [normalizeLanguage(language)]: session }));
  }

  /** The session for `language`: the one assigned, else one to join, else a new one. */
  ensure(language: string): Promise<string> {
    const lang = normalizeLanguage(language);
    const existing = this.session(lang);
    if (existing) return Promise.resolve(existing);
    const pending = this.starting.get(lang);
    if (pending) return pending;
    const join = this.ws.sessionToJoin(lang, this.cwd(), this.namedKernel(lang));
    if (join) {
      this.assign(lang, join.id);
      return Promise.resolve(join.id);
    }
    return this.track(lang, this.start(lang, undefined, true));
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

  /** Restarts the session; other documents using it move to the new one too. */
  async restart(language: string) {
    const old = this.session(language);
    const kernel = this.ws.sessions().find((s) => s.id === old)?.kernel;
    if (old) await this.ws.shutdown(old).catch(() => {});
    const fresh = await this.startNew(language, kernel);
    if (old) this.ws.replaced(old, fresh);
  }

  async interrupt(language: string) {
    const id = this.session(language);
    if (id) await this.ws.interrupt(id);
  }

  /** Starts a kernel; `shared` ones are for other documents to join too. */
  private async start(language: string, kernel?: string, shared = false): Promise<string> {
    const find = (specs: KernelspecInfo[]) =>
      kernel ? specs.find((k) => k.name === kernel) : this.ws.pick(language, this.preferred(language), specs);
    // The user may have installed a kernel since the app connected.
    const spec = find(this.ws.kernelspecs()) ?? find(await this.ws.refreshKernelspecs());
    if (!spec) throw new Error(missingKernel(this.ws, language));
    const cwd = this.cwd();
    const info = shared ? await this.ws.startShared(spec.name, cwd) : await this.ws.startSession(spec.name, cwd);
    this.assign(language, info.id);
    return info.id;
  }

  /** The kernel the project or else the document names for `language`, if one does. */
  private namedKernel(language: string): string | undefined {
    const project = this.ws.projectKernel(language);
    if (project) return project;
    const wanted = this.preferred(language);
    if (!wanted?.name && !wanted?.display_name) return undefined;
    return pickKernel(this.ws.kernelspecs(), language, wanted)?.name;
  }
}

/** Why no kernel for `language` can start, and what to do. */
export function missingKernel(ws: Workspace, language: string): string {
  const named = ws.projectKernel(language);
  if (named) {
    const name = language === "r" ? "R" : language === "python" ? "Python" : language;
    return `This project runs ${name} in the kernel ${named} (set in .sandhill/project.toml), which isn't installed here. Pick another in Project settings, in the ☰ menu.`;
  }
  if (language === "python") {
    return "No Python kernel was found. Install ipykernel in the environment you use (for conda: conda install -n <env> ipykernel) and run again.";
  }
  if (language === "r") {
    return "No R kernel was found. Install Ark, or IRkernel in R (install.packages(\"IRkernel\")), and run again.";
  }
  return `No ${language} kernel was found where sandhilld runs.`;
}
