// Every run the UI knows about, kept current from the daemon's events.
// Documents look up their cells' runs here by id.

import {
  type CellOutputs,
  type CellRef,
  type DaemonEvent,
  EVENT_RUN_FINISHED,
  EVENT_RUN_STARTED,
  type Output,
} from "../backend/types";
import { OutputBuilder } from "./outputs";

export type RunPhase = "queued" | "running" | "done";

export interface RunState {
  id: string;
  session: string;
  cell?: CellRef;
  code: string;
  phase: RunPhase;
  /** Kernel status once done: ok, error, aborted, or unknown. */
  status?: string;
  executionCount?: number;
  startedMs: number;
  finishedMs?: number;
  outputs: Output[];
}

interface Entry {
  state: RunState;
  builder: OutputBuilder;
  /** Highest event seq applied; replayed events at or below it are skipped. */
  lastSeq: number;
  /**
   * False when events arrived before we had the run's start: they are held
   * until a snapshot (file_outputs) supplies everything up to its seq.
   */
  complete: boolean;
  held: DaemonEvent[];
}

type Listener = (run: RunState) => void;

export class RunStore {
  private entries = new Map<string, Entry>();
  private listeners = new Map<string, Set<Listener>>();
  private any = new Set<Listener>();

  get(id: string): RunState | undefined {
    return this.entries.get(id)?.state;
  }

  /** Runs of a session that haven't finished. */
  pending(session: string): RunState[] {
    return [...this.entries.values()]
      .map((e) => e.state)
      .filter((r) => r.session === session && r.phase !== "done");
  }

  watch(id: string, listener: Listener): () => void {
    let set = this.listeners.get(id);
    if (!set) this.listeners.set(id, (set = new Set()));
    set.add(listener);
    return () => set!.delete(listener);
  }

  watchAll(listener: Listener): () => void {
    this.any.add(listener);
    return () => this.any.delete(listener);
  }

  apply(event: DaemonEvent) {
    if (!event.run) return;
    let entry = this.entries.get(event.run);
    if (!entry) {
      if (event.msg_type !== EVENT_RUN_STARTED) {
        // Output for a run we haven't seen start: hold it until a snapshot.
        entry = this.create(event.run, event.session, "", undefined, event.ts_ms, [], false);
        entry.held.push(event);
        return;
      }
      entry = this.create(event.run, event.session, event.content?.code ?? "", event.content?.cell ?? undefined, event.ts_ms, [], true);
      entry.lastSeq = event.seq;
      this.notify(entry);
      return;
    }
    if (!entry.complete) {
      entry.held.push(event);
      return;
    }
    if (event.seq <= entry.lastSeq) return;
    entry.lastSeq = event.seq;
    if (this.applyToState(entry, event)) this.notify(entry);
  }

  /** Brings in a run as of journal position `seq`, from `file_outputs`. */
  applySnapshot(cell: CellOutputs, file: string, seq: number) {
    const existing = this.entries.get(cell.run);
    if (existing?.complete) return;
    const entry = this.create(
      cell.run,
      cell.session,
      cell.code,
      { file, cell_id: cell.cell_id },
      cell.started_ms,
      cell.outputs,
      true,
    );
    const s = entry.state;
    if (cell.finished_ms !== undefined) {
      s.phase = "done";
      s.status = cell.status;
      s.finishedMs = cell.finished_ms;
      s.executionCount = cell.execution_count;
    } else {
      s.phase = cell.outputs.length ? "running" : "queued";
    }
    entry.lastSeq = seq;
    for (const event of existing?.held ?? []) {
      if (event.seq > entry.lastSeq) {
        entry.lastSeq = event.seq;
        this.applyToState(entry, event);
      }
    }
    this.notify(entry);
  }

  /** Marks a session's unfinished runs as ended, e.g. when the kernel dies. */
  endSession(session: string) {
    for (const entry of this.entries.values()) {
      if (entry.state.session === session && entry.state.phase !== "done") {
        entry.state = { ...entry.state, phase: "done", status: entry.state.status ?? "unknown" };
        this.notify(entry);
      }
    }
  }

  private create(
    id: string,
    session: string,
    code: string,
    cell: CellRef | undefined,
    startedMs: number,
    outputs: Output[],
    complete: boolean,
  ): Entry {
    const builder = new OutputBuilder(outputs);
    const entry: Entry = {
      state: { id, session, cell, code, phase: "queued", startedMs, outputs: builder.outputs },
      builder,
      lastSeq: 0,
      complete,
      held: [],
    };
    this.entries.set(id, entry);
    return entry;
  }

  private applyToState(entry: Entry, event: DaemonEvent): boolean {
    const s = entry.state;
    switch (event.msg_type) {
      case "execute_input":
        if (s.phase !== "queued") return false;
        entry.state = { ...s, phase: "running" };
        return true;
      case EVENT_RUN_FINISHED:
        entry.state = {
          ...s,
          phase: "done",
          status: event.content?.status,
          executionCount: event.content?.execution_count ?? s.executionCount,
          finishedMs: event.ts_ms,
        };
        return true;
      default: {
        if (!entry.builder.push(event.msg_type, event.content)) return false;
        entry.state = { ...s, phase: s.phase === "queued" ? "running" : s.phase, outputs: entry.builder.outputs };
        return true;
      }
    }
  }

  private notify(entry: Entry) {
    for (const listener of this.listeners.get(entry.state.id) ?? []) listener(entry.state);
    for (const listener of this.any) listener(entry.state);
  }
}
