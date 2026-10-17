import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import type { SessionInfo, SystemStats } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { KernelRows, resourceRows } from "./LeftPane";

function session(id: string, extra: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id,
    kernel: "python3",
    language: "python",
    language_version: "3.12.4",
    cwd: "/p",
    status: "running",
    created_ms: Date.now(),
    last_seq: 0,
    project: "/p",
    ...extra,
  };
}

const GB = 1024 ** 3;

function stats(extra: Partial<SystemStats> = {}): SystemStats {
  return {
    cpu: { percent: 38.4, cores: 16, model: "Apple M3 Max", load: 3.2 },
    memory: { total: 64 * GB, used: 26 * GB },
    gpus: [],
    disk: { path: "/p", total: 1000 * GB, used: 600 * GB, available: 400 * GB },
    sessions: [],
    ...extra,
  };
}

function setup(initial: SessionInfo[], usedBy: Record<string, string[]>, machine?: SystemStats) {
  const [sessions, setSessions] = createSignal(initial);
  const stopped: string[] = [];
  const ws = {
    backend: { host: "local" },
    kernelspecs: () => [{ name: "python3", display_name: "Python 3 (ipykernel)", language: "python", env: "omicverse" }],
    call: async () => machine,
    isBusy: (id: string) => (sessions().find((s) => s.id === id)?.pending_runs ?? 0) > 0,
    shutdown: async (id: string) => {
      stopped.push(id);
      setSessions((all) => all.filter((s) => s.id !== id));
    },
  } as unknown as Workspace;
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(
    () => (
      <KernelRows
        ws={ws}
        sessions={sessions()}
        stats={machine}
        describe={(s) => (usedBy[s.id] ?? []).join(", ") || "not used by an open file"}
        unused={(s) => !usedBy[s.id]?.length}
        usedBy={(s) => usedBy[s.id] ?? []}
      />
    ),
    root,
  );
  const rows = () => [...root.querySelectorAll(".krow:not(.kunused)")] as HTMLElement[];
  const click = async (el: Element | null | undefined) => {
    expect(el).toBeTruthy();
    (el as HTMLElement).click();
    await new Promise((r) => setTimeout(r, 0));
  };
  const button = (text: string) => [...root.querySelectorAll("button")].find((b) => b.textContent?.trim() === text);
  return { root, rows, stopped, click, button, dispose: () => (dispose(), root.remove()) };
}

describe("KernelRows", () => {
  it("shows each session's version, files and memory, with its language as the icon", () => {
    const t = setup(
      [session("aaaaaa1"), session("bbbbbb2", { language: "r", kernel: "ark", language_version: "4.4.1" })],
      { aaaaaa1: ["a.qmd", "b.py"] },
      stats({ sessions: [{ session: "bbbbbb2", rss: 2.1 * GB }] }),
    );
    expect(t.rows().map((r) => r.querySelector(".kn")?.textContent)).toEqual(["3.12a.qmd, b.py", "4.4.1not used by an open file"]);
    expect(t.rows()[1].title).toBe("R 4.4.1 · idle. Click for details");
    expect(t.rows().map((r) => r.querySelector(".km")?.textContent)).toEqual(["", "2.1 GB"]);
    expect(t.rows()[1].classList.contains("unused")).toBe(true);
    t.dispose();
  });

  it("asks in the row before shutting a session down", async () => {
    const t = setup([session("aaaaaa1"), session("bbbbbb2")], { aaaaaa1: ["a.qmd"], bbbbbb2: ["b.qmd"] });
    await t.click(t.rows()[0].querySelector(".kx"));
    expect(t.rows()[0].classList.contains("ask")).toBe(true);
    expect(t.rows()[0].textContent).toContain("Shut down Python 3.12?");
    await t.click(t.button("Cancel"));
    expect(t.stopped).toEqual([]);
    expect(t.root.querySelector(".krow.ask")).toBeNull();
    await t.click(t.rows()[0].querySelector(".kx"));
    await t.click(t.button("Shut down"));
    expect(t.stopped).toEqual(["aaaaaa1"]);
    expect(t.rows()).toHaveLength(1);
    t.dispose();
  });

  it("shuts down the sessions no open file uses, except ones running code", async () => {
    const t = setup(
      [session("used"), session("idle1"), session("idle2"), session("busy", { pending_runs: 1 })],
      { used: ["a.qmd"] },
    );
    const offer = t.root.querySelector(".kunused");
    expect(offer?.textContent).toBe("Shut down 2 unused sessions");
    await t.click(offer);
    expect(t.root.querySelector(".krow.ask")?.textContent).toContain("Shut down 2 sessions?");
    await t.click(t.button("Shut down"));
    expect(t.stopped).toEqual(["idle1", "idle2"]);
    expect(t.root.querySelector(".kunused")).toBeNull();
    t.dispose();
  });

  it("says when no kernels run", () => {
    const t = setup([], {});
    expect(t.root.querySelector(".krow.dim")?.textContent).toBe("No kernels running");
    t.dispose();
  });

  it("opens a session's details beside its row", async () => {
    const t = setup([session("aaaaaa1"), session("bbbbbb2")], { aaaaaa1: ["a.qmd"] }, stats({ sessions: [{ session: "aaaaaa1", rss: GB }] }));
    await t.click(t.rows()[0]);
    const pop = () => document.querySelector(".kpop");
    expect(pop()?.querySelector(".kp-h")?.textContent).toBe("Python 3.12idle");
    const details = () => [...pop()!.querySelectorAll("dt")].map((dt) => `${dt.textContent}: ${dt.nextElementSibling?.textContent}`);
    expect(details()).toEqual([
      "Kernel: Python 3 (ipykernel)",
      "Environment: omicverse",
      "Runs on: This Mac",
      "Used by: a.qmd",
      "Memory: 1.0 GB",
      "Session ID: aaaaaa1",
    ]);
    // Shut down… asks in the row, as the trash button does.
    await t.click([...pop()!.querySelectorAll("button")].find((b) => b.textContent === "Shut down…"));
    expect(pop()).toBeNull();
    expect(t.rows()[0].textContent).toContain("Shut down Python 3.12?");
    // The trash button asks without opening the details.
    await t.click(t.rows()[1].querySelector(".kx"));
    expect(pop()).toBeNull();
    t.dispose();
  });

  it("collapses to a count from its header", async () => {
    const t = setup([session("aaaaaa1"), session("bbbbbb2")], {});
    const head = t.root.querySelector(".sec-h") as HTMLElement;
    expect(head.textContent).toBe("Kernels");
    await t.click(head);
    expect(t.rows()).toHaveLength(0);
    expect(head.getAttribute("aria-expanded")).toBe("false");
    expect(head.textContent).toBe("Kernels2");
    await t.click(head);
    expect(t.rows()).toHaveLength(2);
    t.dispose();
  });
});

describe("resourceRows", () => {
  it("has CPU, memory and disk, and a GPU row only with a GPU", () => {
    const rows = resourceRows(
      stats({ sessions: [{ session: "r1", rss: 2.1 * GB }] }),
      [session("r1", { language: "r", language_version: "4.4.1" })],
    );
    expect(rows.map((r) => [r.label, r.percent])).toEqual([
      ["CPU", 38],
      ["MEM", 41],
      ["Disk", 60],
    ]);
    expect(rows[0].main).toBe("Apple M3 Max · 16 cores");
    expect(rows[1].main).toBe("26 GB of 64 GB used");
    expect(rows[1].detail.replace(/\u00a0/g, " ")).toBe("Kernels: R 4.4.1 2.1 GB");
    expect(rows[2].detail).toBe("400 GB free for /p");
  });

  it("shows the busiest of several GPUs", () => {
    const rows = resourceRows(
      stats({
        gpus: [
          { name: "NVIDIA A100", percent: 12, memory_used: 4 * GB, memory_total: 80 * GB },
          { name: "NVIDIA A100", percent: 84, memory_used: 60 * GB, memory_total: 80 * GB },
        ],
      }),
      [],
    );
    const gpu = rows.find((r) => r.key === "gpu")!;
    expect(gpu.percent).toBe(84);
    expect(gpu.name).toBe("GPU (2)");
    expect(gpu.main).toBe("2 × NVIDIA A100");
    expect(gpu.detail.split("\n")).toHaveLength(2);
  });
});
