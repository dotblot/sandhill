// Builds nbformat outputs from a run's kernel messages, as they stream in.
// crates/daemon/src/outputs.rs applies the same rules when it rebuilds
// outputs from the journal; tests/fixtures/outputs.json keeps them in step.
//
// Outputs are replaced rather than mutated when they change, so renderers
// can tell what changed by comparing object identity.

import type { Output } from "../backend/types";

interface Entry {
  value: Output;
  displayId?: string;
}

export class OutputBuilder {
  private entries: Entry[] = [];
  private clearPending = false;
  private cached: Output[] | null = null;

  constructor(initial: Output[] = []) {
    this.entries = initial.map((value) => ({ value }));
  }

  /** Applies one event; returns whether the outputs changed. */
  push(msgType: string, content: any): boolean {
    switch (msgType) {
      case "clear_output":
        if (content?.wait) {
          this.clearPending = true;
          return false;
        }
        this.clearPending = false;
        return this.replaceAll([]);
      case "update_display_data": {
        const id = content?.transient?.display_id;
        if (typeof id !== "string") return false;
        let changed = false;
        for (const entry of this.entries) {
          if (entry.displayId === id) {
            entry.value = { ...entry.value, data: content.data, metadata: metadataOf(content) };
            changed = true;
          }
        }
        if (changed) this.cached = null;
        return changed;
      }
      case "stream": {
        this.takePendingClear();
        const name = content?.name ?? "stdout";
        const text = content?.text ?? "";
        const last = this.entries[this.entries.length - 1];
        if (last && last.value.output_type === "stream" && last.value.name === name) {
          last.value = { ...last.value, text: last.value.text + text };
          this.cached = null;
          return true;
        }
        return this.add({ output_type: "stream", name, text });
      }
      case "display_data":
        this.takePendingClear();
        return this.add(
          { output_type: "display_data", data: content?.data, metadata: metadataOf(content) },
          content?.transient?.display_id,
        );
      case "execute_result":
        this.takePendingClear();
        return this.add(
          {
            output_type: "execute_result",
            execution_count: content?.execution_count ?? null,
            data: content?.data,
            metadata: metadataOf(content),
          },
          content?.transient?.display_id,
        );
      case "error":
        this.takePendingClear();
        return this.add({
          output_type: "error",
          ename: content?.ename,
          evalue: content?.evalue,
          traceback: content?.traceback,
        });
      default:
        return false;
    }
  }

  get outputs(): Output[] {
    if (!this.cached) this.cached = this.entries.map((e) => e.value);
    return this.cached;
  }

  private add(value: Output, displayId?: unknown): boolean {
    this.entries.push({ value, displayId: typeof displayId === "string" ? displayId : undefined });
    this.cached = null;
    return true;
  }

  private replaceAll(entries: Entry[]): boolean {
    const changed = this.entries.length > 0 || entries.length > 0;
    this.entries = entries;
    this.cached = null;
    return changed;
  }

  private takePendingClear() {
    if (this.clearPending) {
      this.entries = [];
      this.cached = null;
      this.clearPending = false;
    }
  }
}

function metadataOf(content: any): Record<string, unknown> {
  const m = content?.metadata;
  return m && typeof m === "object" && !Array.isArray(m) ? { ...m } : {};
}
