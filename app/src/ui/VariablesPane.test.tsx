import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, SessionInfo, Variables } from "../backend/types";
import { Workspace } from "../model/workspace";
import { VariablesPane } from "./VariablesPane";

const SESSION: SessionInfo = {
  id: "s1",
  kernel: "python3",
  language: "python",
  cwd: "/home/huan/pbmc",
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 0,
  project: "/home/huan/pbmc",
  pending_runs: 0,
  language_version: "3.12.4",
};

const LISTED: Variables = {
  session: "s1",
  path: [],
  variables: [
    { name: "adata", kind: "data", class: "AnnData", size: 412 * 1024 * 1024, children: true, summary: "2,700 × 32,738" },
    { name: "n", kind: "value", class: "int", size: 28, children: false, summary: "3" },
  ],
  more: 0,
};

function fakeWorkspace() {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  let emit: (event: DaemonEvent) => void = () => {};
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "list_sessions":
          return [SESSION] as T;
        case "list_variables":
          return (params.path as string[]).length
            ? ({ session: "s1", path: params.path, variables: [{ name: "obs", kind: "data", class: "DataFrame", children: true, summary: "2,700 × 4" }], more: 0 } as T)
            : (LISTED as T);
        default:
          return null as T;
      }
    },
    onEvent(listener) {
      emit = listener;
    },
    onTerminal() {},
    onRequest() {},
    openLink() {},
    onLink() {},
    blobUrl: () => "",
    reconnect: async () => {},
    disconnect: async () => {},
  };
  const ws = new Workspace(backend, "/home/huan/pbmc");
  const listings = () => calls.filter((c) => c.method === "list_variables" && !(c.params.path as string[]).length).length;
  return { ws, calls, listings, emit: (event: DaemonEvent) => emit(event) };
}

let seq = 0;
const run = (emit: (e: DaemonEvent) => void, id: string, content: Record<string, unknown> = {}) => {
  emit({ seq: ++seq, session: "s1", run: id, msg_type: "sandhill.run_started", ts_ms: seq, content: { code: "x", ...content } });
  emit({ seq: ++seq, session: "s1", run: id, msg_type: "sandhill.run_finished", ts_ms: seq, content: { status: "ok" } });
};

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

describe("VariablesPane", () => {
  it("lists once when shown, and again only after the session runs code", async () => {
    const { ws, listings, emit } = fakeWorkspace();
    await ws.refreshSessions();
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <VariablesPane ws={ws} session={SESSION} sessions={[SESSION]} onSession={() => {}} shown />, root);
    await settle();
    expect(listings()).toBe(1);
    expect([...root.querySelectorAll(".vh")].map((e) => e.textContent)).toEqual(["Data", "Values"]);
    // Its own listing runs quietly: that doesn't list again.
    run(emit, "quiet1", { quiet: true });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(listings()).toBe(1);
    run(emit, "r1");
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(listings()).toBe(2);
    dispose();
  });

  it("opens a variable's parts and deletes what is ticked", async () => {
    const { ws, calls } = fakeWorkspace();
    await ws.refreshSessions();
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <VariablesPane ws={ws} session={SESSION} sessions={[SESSION]} onSession={() => {}} shown />, root);
    await settle();
    (root.querySelector(".var.opens") as HTMLElement).click();
    await settle();
    expect([...root.querySelectorAll(".var.child .nm")].map((e) => e.textContent)).toEqual(["obs"]);
    (root.querySelector(".vck") as HTMLInputElement).click();
    expect(root.querySelector(".vcount")?.textContent).toBe("1 selected · 412 MB");
    (root.querySelector(".vbtn.danger") as HTMLButtonElement).click();
    await settle();
    await settle();
    expect(calls.find((c) => c.method === "delete_variables")?.params).toEqual({ session: "s1", names: ["adata"] });
    expect(root.querySelector(".vfreed")?.textContent).toBe("Removed adata · 412 MB freed");
    dispose();
  });
});
