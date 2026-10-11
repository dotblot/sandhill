import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import type { SessionInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { Sessions } from "./App";
import { Dialogs } from "./dialogs";

function session(id: string, extra: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id,
    kernel: "python3",
    language: "python",
    cwd: "/p",
    status: "running",
    created_ms: Date.now(),
    last_seq: 0,
    project: "/p",
    ...extra,
  };
}

function setup(initial: SessionInfo[], usedBy: Record<string, string[]>) {
  const [sessions, setSessions] = createSignal(initial);
  const stopped: string[] = [];
  const ws = {
    projectSessions: () => sessions().filter((s) => s.status !== "ended"),
    kernelspecs: () => [{ name: "python3", display_name: "Python 3", language: "python" }],
    isBusy: (id: string) => (sessions().find((s) => s.id === id)?.pending_runs ?? 0) > 0,
    refreshSessions: async () => {},
    shutdown: async (id: string) => {
      stopped.push(id);
      setSessions((all) => all.map((s) => (s.id === id ? { ...s, status: "ended" as const } : s)));
    },
  } as unknown as Workspace;
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(
    () => (
      <>
        <Sessions ws={ws} hostName="lab" usedBy={(id) => usedBy[id] ?? []} />
        <Dialogs />
      </>
    ),
    root,
  );
  const rows = () => [...root.querySelectorAll(".session-row")] as HTMLElement[];
  const answer = async (choice: string) => {
    await new Promise((r) => setTimeout(r, 0));
    const button = [...root.querySelectorAll(".modal button")].find((b) => b.textContent === choice) as HTMLElement;
    expect(button, `a "${choice}" button`).toBeTruthy();
    button.click();
    await new Promise((r) => setTimeout(r, 0));
  };
  return { root, rows, stopped, answer, dispose: () => (dispose(), root.remove()) };
}

describe("Sessions", () => {
  it("stops one session after asking", async () => {
    const t = setup([session("aaaaaa1"), session("bbbbbb2")], { aaaaaa1: ["a.qmd"] });
    expect(t.rows()).toHaveLength(2);
    (t.rows()[0].querySelector(".session-stop") as HTMLElement).click();
    expect(t.root.querySelector(".modal-detail")?.textContent).toContain("a.qmd will start a new session");
    await t.answer("Cancel");
    expect(t.stopped).toEqual([]);
    (t.rows()[0].querySelector(".session-stop") as HTMLElement).click();
    await t.answer("Stop");
    expect(t.stopped).toEqual(["aaaaaa1"]);
    expect(t.rows()).toHaveLength(1);
    t.dispose();
  });

  it("stops the sessions no open file uses, except ones running code", async () => {
    const t = setup(
      [session("used"), session("idle1"), session("idle2"), session("busy", { pending_runs: 1 })],
      { used: ["a.qmd"] },
    );
    expect(t.rows().map((r) => r.classList.contains("unused"))).toEqual([false, true, true, true]);
    (t.root.querySelector(".stop-unused") as HTMLElement).click();
    await t.answer("Stop");
    expect(t.stopped).toEqual(["idle1", "idle2"]);
    expect(t.root.querySelector(".stop-unused")).toBeNull();
    t.dispose();
  });
});
