import { createSignal, type JSX } from "solid-js";
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
    language_version: "3.12.4",
    cwd: "/w",
    pid: 1,
    status: "running",
    created_ms: 0,
    last_seq: 0,
  } as SessionInfo;
}

function fakeDoc(
  sessionId: () => string | undefined,
  languages = ["python"],
  shown: () => string[] = () => [],
  assigned: [string, string][] = [],
): Doc {
  return {
    languages: () => languages,
    dirty: () => false,
    shownOutputs: shown,
    kernels: {
      session: (language: string) => (language === "python" ? sessionId() : undefined),
      isStarting: () => false,
      assign: (language: string, id: string) => assigned.push([language, id]),
    },
  } as unknown as Doc;
}

function fakeWorkspace(sessions: SessionInfo[]): Workspace {
  return {
    backend: { host: "local" },
    call: async () => ({ sessions: [] }),
    sessions: () => sessions,
    kernelspecs: () => [{ name: "python3", display_name: "Python 3 (ipykernel)", language: "python" }],
    liveSessions: () => sessions,
    projectSessions: () => sessions,
    isBusy: () => false,
  } as unknown as Workspace;
}

/** Renders into the page, since Solid handles clicks at the document. */
function mount(view: () => JSX.Element) {
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(view, root);
  return {
    root,
    picker: () => root.querySelector(".kernel-pick") as HTMLButtonElement,
    /** The session the drop-down marks as current. */
    current: () => root.querySelector(".kpop .mm-i kbd")?.closest(".mm-i")?.querySelector("span:nth-child(2)")?.textContent,
    done() {
      dispose();
      root.remove();
    },
  };
}

describe("KernelBar", () => {
  it("shows the language icon and version, and the session only in the drop-down", () => {
    const ws = fakeWorkspace([session("aaaaaa111")]);
    const view = mount(() => <KernelBar ws={ws} doc={fakeDoc(() => "aaaaaa111")} onError={() => {}} />);

    expect(view.root.querySelector(".kernel svg.lang")).not.toBeNull();
    expect(view.picker().textContent).toBe("3.12");
    expect(view.root.textContent).not.toContain("aaaaaa");
    view.picker().click();
    expect(view.root.querySelector(".kp-h")?.textContent).toContain("Python 3.12");
    expect(view.root.querySelector(".kp-d")?.textContent).toContain("Python 3 (ipykernel)");
    expect(view.current()).toBe("aaaaaa11");
    view.done();
  });

  it("shows each document's own session after switching tabs", () => {
    const ws = fakeWorkspace([session("aaaaaa111"), session("bbbbbb222")]);
    const [doc, setDoc] = createSignal(fakeDoc(() => "aaaaaa111"));
    const view = mount(() => <KernelBar ws={ws} doc={doc()} onError={() => {}} />);
    view.picker().click();

    expect(view.current()).toBe("aaaaaa11");
    setDoc(fakeDoc(() => "bbbbbb222"));
    expect(view.current()).toBe("bbbbbb22");
    setDoc(fakeDoc(() => "aaaaaa111"));
    expect(view.current()).toBe("aaaaaa11");
    view.done();
  });

  it("follows a new tab that picks up its session, and the first tab again after", () => {
    const ws = fakeWorkspace([session("aaaaaa111"), session("bbbbbb222")]);
    const first = fakeDoc(() => "aaaaaa111", ["python", "r"]);
    const [adopted, setAdopted] = createSignal<string>();
    const [doc, setDoc] = createSignal(first);
    const view = mount(() => <KernelBar ws={ws} doc={doc()} onError={() => {}} />);

    setDoc(fakeDoc(adopted));
    expect(view.picker().textContent).toBe("not started");
    setAdopted("bbbbbb222");
    expect(view.picker().textContent).toBe("3.12");
    view.picker().click();
    expect(view.current()).toBe("bbbbbb22");
    setDoc(first);
    expect(view.current()).toBe("aaaaaa11");
    view.done();
  });

  it("moves the document to another session of the project", () => {
    const ws = fakeWorkspace([session("aaaaaa111"), session("bbbbbb222")]);
    const assigned: [string, string][] = [];
    const view = mount(() => <KernelBar ws={ws} doc={fakeDoc(() => "aaaaaa111", ["python"], () => [], assigned)} onError={() => {}} />);
    view.picker().click();
    const use = [...view.root.querySelectorAll<HTMLButtonElement>(".kpop .mm-i")].find((b) => b.textContent?.startsWith("Use"));

    expect(use?.textContent).toContain("Python 3.12 · bbbbbb22");
    use!.click();
    expect(assigned).toEqual([["python", "bbbbbb222"]]);
    expect(view.root.querySelector(".kpop")).toBeNull();
    view.done();
  });

  it("shows every output, or hides them once any is shown", () => {
    const ws = fakeWorkspace([]);
    const [shown, setShown] = createSignal<string[]>([]);
    const calls: boolean[] = [];
    const doc = { ...fakeDoc(() => undefined, ["python"], shown), setAllOutputs: (v: boolean) => calls.push(v) } as unknown as Doc;
    // Solid handles clicks at the document.
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <KernelBar ws={ws} doc={doc} onError={() => {}} />, root);
    const toggle = () => root.querySelector(".outputs-toggle") as HTMLButtonElement;

    expect(toggle().textContent).toBe("Show outputs");
    toggle().click();
    setShown(["load"]);
    expect(toggle().textContent).toBe("Hide outputs");
    toggle().click();
    expect(calls).toEqual([true, false]);
    dispose();
    root.remove();
  });
});
