// What the app shell needs from an open document, whatever its format.

import { createSignal } from "solid-js";
import type { KernelWanted, Workspace } from "../model/workspace";
import type { RenderContext } from "../render/outputs";
import { DocKernels } from "./kernels";

export type DocKind = "qmd" | "ipynb";

export function docKind(path: string): DocKind | undefined {
  const ext = path.split(".").pop()?.toLowerCase();
  if (ext === "qmd" || ext === "rmd") return "qmd";
  if (ext === "ipynb") return "ipynb";
  return undefined;
}

export abstract class Doc {
  abstract readonly kind: DocKind;
  readonly name: string;
  readonly dir: string;
  readonly dirty: () => boolean;
  protected setDirty: (dirty: boolean) => void;
  readonly kernels: DocKernels;
  /** Modification time of the file as last read or written, to detect outside changes. */
  protected mtime = 0;

  constructor(
    protected ws: Workspace,
    readonly path: string,
    preferredKernel: (language: string) => KernelWanted | undefined = () => undefined,
  ) {
    const slash = path.lastIndexOf("/");
    this.name = path.slice(slash + 1);
    this.dir = path.slice(0, slash) || "/";
    const [dirty, setDirty] = createSignal(false);
    this.dirty = dirty;
    this.setDirty = setDirty;
    this.kernels = new DocKernels(ws, this.dir, preferredKernel);
  }

  /** Languages with code in this document, for the kernel bar. */
  abstract languages(): string[];
  abstract mount(parent: HTMLElement): void;
  abstract focus(): void;
  /** Saves; without `force`, refuses (CONFLICT_ERROR) if the file changed on disk. */
  abstract save(force?: boolean): Promise<void>;
  abstract runAll(): Promise<void>;
  abstract dispose(): void;

  protected renderContext(onResize: () => void): RenderContext {
    return {
      blobUrl: (hash, mime) => this.ws.backend.blobUrl(hash, mime, this.path),
      blobText: async (hash) =>
        (await this.ws.call<{ data: string }>("get_blob", { hash, near: this.path })).data,
      onResize,
    };
  }

  /** Runs `code` in this document's session for `language`, tied to a cell. */
  protected async execute(language: string, code: string, cellId: string): Promise<string> {
    const session = await this.kernels.ensure(language);
    return this.ws.execute(session, code, { file: this.path, cell_id: cellId });
  }

  /** Points each language at the session that last ran this file's cells, if it's still running. */
  protected adoptSessions(cells: { language: string; session: string; started_ms: number }[]) {
    const live = new Set(this.ws.liveSessions().map((s) => s.id));
    for (const cell of [...cells].sort((a, b) => a.started_ms - b.started_ms)) {
      if (live.has(cell.session)) this.kernels.assign(cell.language, cell.session);
    }
  }
}
