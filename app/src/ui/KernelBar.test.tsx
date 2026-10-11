import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import type { SessionInfo } from "../backend/types";
import type { Doc } from "../docs/doc";
import type { Workspace } from "../model/workspace";
import { KernelBar } from "./KernelBar";

function session(id: string): SessionInfo {
  return {
    id,
    kernel: "python3",
    language: "python",
    cwd: "/w",
    pid: 1,
    status: "running",
    created_ms: 0,
    last_seq: 0,
  } as SessionInfo;
}

function fakeDoc(sessionId: () => string | undefined, languages = ["python"]): Doc {
  return {
    languages: () => languages,
    dirty: () => false,
    kernels: {
      session: (language: string) => (language === "python" ? sessionId() : undefined),
      isStarting: () => false,
    },
  } as unknown as Doc;
}

describe("KernelBar", () => {
  it("shows each document's own session after switching tabs", () => {
    const sessions = [session("aaaaaa111"), session("bbbbbb222")];
    const ws = {
      sessions: () => sessions,
      kernelspecs: () => [{ name: "python3", display_name: "Python 3", language: "python" }],
      liveSessions: () => sessions,
      isBusy: () => false,
    } as unknown as Workspace;
    const [doc, setDoc] = createSignal(fakeDoc(() => "aaaaaa111"));
    const root = document.createElement("div");
    const dispose = render(() => <KernelBar ws={ws} doc={doc()} onError={() => {}} />, root);
    const select = () => root.querySelector("select") as HTMLSelectElement;

    expect(select().value).toBe("session:aaaaaa111");
    setDoc(fakeDoc(() => "bbbbbb222"));
    expect(select().value).toBe("session:bbbbbb222");
    setDoc(fakeDoc(() => "aaaaaa111"));
    expect(select().value).toBe("session:aaaaaa111");
    dispose();
  });

  it("keeps the choice when a new tab picks up its session and then closes", () => {
    const sessions = [session("aaaaaa111"), session("bbbbbb222")];
    const ws = {
      sessions: () => sessions,
      kernelspecs: () => [{ name: "python3", display_name: "Python 3", language: "python" }],
      liveSessions: () => sessions,
      isBusy: () => false,
    } as unknown as Workspace;
    const first = fakeDoc(() => "aaaaaa111", ["python", "r"]);
    const [adopted, setAdopted] = createSignal<string>();
    const opened = fakeDoc(adopted);
    const [doc, setDoc] = createSignal(first);
    const root = document.createElement("div");
    const dispose = render(() => <KernelBar ws={ws} doc={doc()} onError={() => {}} />, root);
    const select = () => root.querySelector("select") as HTMLSelectElement;

    setDoc(opened);
    expect(select().value).toBe("");
    setAdopted("bbbbbb222");
    expect(select().value).toBe("session:bbbbbb222");
    setDoc(first);
    expect(select().value).toBe("session:aaaaaa111");
    dispose();
  });
});
