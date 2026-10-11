import { describe, expect, it } from "vitest";
import type { DaemonEvent } from "../backend/types";
import { RunStore } from "./runs";

let seq = 0;
function ev(msg_type: string, content: any, run = "r1"): DaemonEvent {
  return { seq: ++seq, session: "s1", run, msg_type, ts_ms: 1000 + seq, content };
}

describe("RunStore", () => {
  it("follows a run from queued to done", () => {
    const store = new RunStore();
    const seen: string[] = [];
    store.watch("r1", (r) => seen.push(r.phase));
    store.apply(ev("sandhill.run_started", { code: "print(1)", cell: { file: "/a.qmd", cell_id: "x" } }));
    store.apply(ev("execute_input", { code: "print(1)" }));
    store.apply(ev("stream", { name: "stdout", text: "1\n" }));
    store.apply(ev("sandhill.run_finished", { status: "ok", execution_count: 4 }));
    const run = store.get("r1")!;
    expect(run.phase).toBe("done");
    expect(run.status).toBe("ok");
    expect(run.executionCount).toBe(4);
    expect(run.cell).toEqual({ file: "/a.qmd", cell_id: "x" });
    expect(run.outputs).toEqual([{ output_type: "stream", name: "stdout", text: "1\n" }]);
    expect(seen).toEqual(["queued", "running", "running", "done"]);
  });

  it("ignores events replayed after a resubscribe", () => {
    const store = new RunStore();
    const started = ev("sandhill.run_started", { code: "x" }, "r2");
    const out = ev("stream", { name: "stdout", text: "once" }, "r2");
    for (const e of [started, out, out]) store.apply(e);
    expect(store.get("r2")!.outputs[0].text).toBe("once");
  });

  it("joins a snapshot with output that arrived before it", () => {
    const store = new RunStore();
    // Live output for a run whose start we never saw.
    const early = ev("stream", { name: "stdout", text: "step 1\n" }, "r3");
    const late = ev("stream", { name: "stdout", text: "step 2\n" }, "r3");
    store.apply(early);
    store.apply(late);
    expect(store.get("r3")!.outputs).toEqual([]);
    // The snapshot already covers `early`.
    store.applySnapshot(
      {
        cell_id: "c",
        run: "r3",
        session: "s1",
        language: "python",
        code: "loop()",
        started_ms: 1,
        outputs: [{ output_type: "stream", name: "stdout", text: "step 0\nstep 1\n" }],
      },
      "/n.ipynb",
      early.seq,
    );
    const run = store.get("r3")!;
    expect(run.phase).toBe("running");
    expect(run.outputs[0].text).toBe("step 0\nstep 1\nstep 2\n");
    expect(store.pending("s1").map((r) => r.id)).toContain("r3");
  });

  it("closes runs when their session ends", () => {
    const store = new RunStore();
    store.apply(ev("sandhill.run_started", { code: "x" }, "r4"));
    store.endSession("s1");
    expect(store.get("r4")!.phase).toBe("done");
    expect(store.get("r4")!.status).toBe("unknown");
  });
});
