import { describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { TerminalEvent, TerminalInfo } from "../backend/types";
import { TerminalStore } from "./terminals";

function info(id: string, extra: Partial<TerminalInfo> = {}): TerminalInfo {
  return { id, kind: "claude", title: "Claude Code", cwd: "/p", project: "/p", created_ms: 1, running: true, end: 0, ...extra };
}

function fakeBackend(terminals: TerminalInfo[]) {
  const calls: { method: string; params?: Record<string, unknown> }[] = [];
  let listener: (e: TerminalEvent) => void = () => {};
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params?: Record<string, unknown>): Promise<T> {
      calls.push({ method, params });
      if (method === "list_terminals") return terminals as T;
      if (method === "subscribe_terminal") return { terminal: params!.terminal, from: params!.from ?? 0 } as T;
      if (method === "start_terminal") return info("new", { kind: params!.kind as string }) as T;
      return null as T;
    },
    onEvent() {},
    onTerminal(l) {
      listener = l;
    },
    onLink() {},
    blobUrl: () => "",
    openLink() {},
  };
  const send = (terminal: string, offset: number, text: string) =>
    listener({ terminal, offset, data: btoa(text) });
  return { backend, calls, send, emit: (e: TerminalEvent) => listener(e) };
}

const text = (chunks: Uint8Array[]) => chunks.map((c) => new TextDecoder().decode(c)).join("");

describe("TerminalStore", () => {
  it("lists only this project's terminals and follows each", async () => {
    const fake = fakeBackend([info("a"), info("b", { project: "/other" }), info("c", { kind: "shell", created_ms: 0 })]);
    const store = new TerminalStore(fake.backend, "/p");
    await store.refresh();
    expect(store.list().map((t) => t.id)).toEqual(["c", "a"]);
    expect(store.agents().map((t) => t.id)).toEqual(["a"]);
    expect(store.shells().map((t) => t.id)).toEqual(["c"]);
    const subscribed = fake.calls.filter((c) => c.method === "subscribe_terminal").map((c) => c.params);
    // The first time, the daemon picks the recent output.
    expect(subscribed).toEqual([
      { terminal: "c", from: undefined },
      { terminal: "a", from: undefined },
    ]);
  });

  it("keeps output until a view attaches, then passes it straight on", async () => {
    const fake = fakeBackend([info("a")]);
    const store = new TerminalStore(fake.backend, "/p");
    await store.refresh();
    fake.send("a", 100, "hello ");
    const got: Uint8Array[] = [];
    store.attach("a", { write: (d) => got.push(d) });
    fake.send("a", 106, "world");
    expect(text(got)).toBe("hello world");
  });

  it("skips output it already has and resumes from where it got to", async () => {
    const fake = fakeBackend([info("a")]);
    const store = new TerminalStore(fake.backend, "/p");
    await store.refresh();
    const got: Uint8Array[] = [];
    store.attach("a", { write: (d) => got.push(d) });
    fake.send("a", 0, "abc");
    fake.send("a", 1, "bcdef");
    fake.send("a", 0, "ab");
    expect(text(got)).toBe("abcdef");

    // After a reconnect it asks for what came after.
    fake.calls.length = 0;
    await store.refresh();
    expect(fake.calls.find((c) => c.method === "subscribe_terminal")?.params).toEqual({ terminal: "a", from: 6 });
  });

  it("tracks what agents report and when they end", async () => {
    const fake = fakeBackend([info("a")]);
    const store = new TerminalStore(fake.backend, "/p");
    await store.refresh();
    fake.emit({ terminal: "a", agent: { state: "waiting", message: "May I?" } });
    expect(store.waiting().map((t) => t.id)).toEqual(["a"]);
    fake.emit({ terminal: "a", agent: { state: "busy" } });
    expect(store.waiting()).toEqual([]);
    expect(store.get("a")?.agent).toEqual({ state: "busy" });
    fake.emit({ terminal: "a", exit: { code: 0, ended_ms: 5 } });
    expect(store.get("a")?.running).toBe(false);
  });

  it("starts terminals in the project and follows them from the start", async () => {
    const fake = fakeBackend([]);
    const store = new TerminalStore(fake.backend, "/p");
    await store.start("claude", { model: "opus" }, { cols: 90, rows: 30 });
    expect(fake.calls.map((c) => c.method)).toEqual(["start_terminal", "subscribe_terminal"]);
    expect(fake.calls[0].params).toEqual({ kind: "claude", project: "/p", cols: 90, rows: 30, options: { model: "opus" } });
    expect(fake.calls[1].params).toEqual({ terminal: "new", from: 0 });
    expect(store.agents().map((t) => t.id)).toEqual(["new"]);
    await store.close("new");
    expect(store.list()).toEqual([]);
  });
});
