// Every run the UI knows about, kept current from the daemon's events.
// Documents look up their cells' runs here by id.

import {
  type CellOutputs,
  type CellRef,
  type DaemonEvent,
  EVENT_RUN_FINISHED,
  EVENT_RUN_STARTED,
  type Output,
  type SessionRun,
} from "../backend/types";
import { OutputBuilder } from "./outputs";

export type RunPhase = "queued" | "running" | "done";

/** Scripts' runs show their output in the Console; notebooks keep theirs under the cell. */
export function isScript(path: string): boolean {
  return /\.(r|py)$/i.test(path);
}

/** Whether code ran from a notebook's or Quarto document's cell. */
export function fromNotebook(cell: CellRef | undefined): boolean {
  return !!cell && !isScript(cell.file);
}

export interface RunState {
  id: string;
  session: string;
  cell?: CellRef;
  /** A tool looking around (listing variables), kept out of the Console. */
  quiet?: boolean;
  /** The agent that ran it, like "Claude". */
  by?: string;
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
  /**
   * The session's runs that haven't finished. Runs seen only from their
   * later events (one started before the subscription, such as a quiet one
   * no snapshot lists) don't count: nothing says they are still going.
   */
  pending(session: string): RunState[] {
    return [...this.entries.values()]
      .filter((e) => e.complete)
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
      if (event.content?.quiet) entry.state.quiet = true;
      if (event.content?.by) entry.state.by = event.content.by;
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
    // Nothing shows a quiet run once it is done (the Variables pane lists after every run).
    if (entry.state.quiet && entry.state.phase === "done") this.entries.delete(event.run);
  }

  /** Brings in a run as of journal position `seq`, from `file_outputs`. */
  applySnapshot(cell: CellOutputs, file: string, seq: number) {
    this.snapshot({ ...cell, cell: { file, cell_id: cell.cell_id } }, cell.session, seq);
  }

  /** Brings in a run as of journal position `seq`, from `session_runs`. */
  applySessionRun(run: SessionRun, session: string, seq: number) {
    this.snapshot(run, session, seq);
  }

  private snapshot(run: SessionRun, session: string, seq: number) {
    const existing = this.entries.get(run.run);
    if (existing?.complete) return;
    const entry = this.create(run.run, session, run.code, run.cell, run.started_ms, run.outputs, true);
    const s = entry.state;
    if (run.by) s.by = run.by;
    if (run.finished_ms !== undefined) {
      s.phase = "done";
      s.status = run.status;
      s.finishedMs = run.finished_ms;
      s.executionCount = run.execution_count;
    } else {
      s.phase = run.outputs.length ? "running" : "queued";
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
