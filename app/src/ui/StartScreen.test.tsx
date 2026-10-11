import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { LinkState, Shell } from "../backend";
import type { ProjectInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { openedAgo, StartScreen } from "./StartScreen";

function project(name: string, exists = true): ProjectInfo {
  return { path: `/w/${name}`, name, opened_ms: Date.now(), exists, state: null };
}

function fakes(projects: ProjectInfo[]) {
  const calls: [string, unknown][] = [];
  const [link] = createSignal<LinkState>({ status: "connected" });
  const ws = {
    backend: { host: "local" },
    link,
    host: () => ({ home: "/w", hostname: "mac", daemon_version: "0.1.0", protocol_version: 1 }),
    call: async (method: string, params?: { path?: string }) => {
      calls.push([method, params]);
      if (method === "list_recent_projects") return projects;
      if (method === "open_project") return project(params!.path!.split("/").pop()!);
      if (method === "forget_project") {
        projects = projects.filter((p) => p.path !== params!.path);
        return null;
      }
      throw new Error(`unexpected ${method}`);
    },
  } as unknown as Workspace;
  const shell = { openProject: vi.fn(async () => {}), closeWindow: vi.fn(async () => {}) } as unknown as Shell;
  return { ws, shell, calls };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

// Solid handles clicks at the document, so each screen is rendered into it.
describe("StartScreen", () => {
  let dispose = () => {};
  afterEach(() => {
    dispose();
    document.body.replaceChildren();
    localStorage.clear();
  });

  it("opens a recent project in its window and closes itself", async () => {
    const { ws, shell, calls } = fakes([project("atlas"), project("gone", false)]);
    const root = document.body.appendChild(document.createElement("div"));
    dispose = render(() => <StartScreen ws={ws} shell={shell} />, root);
    await settle();

    const items = [...root.querySelectorAll(".recent-item")];
    expect(items.map((i) => i.querySelector(".recent-name")!.textContent)).toEqual(["atlas", "gone"]);
    expect((items[1].querySelector(".recent-open") as HTMLButtonElement).disabled).toBe(true);

    (items[0].querySelector(".recent-open") as HTMLButtonElement).click();
    await settle();
    expect(calls).toContainEqual(["open_project", { path: "/w/atlas" }]);
    expect(shell.openProject).toHaveBeenCalledWith("local", "/w/atlas");
    expect(shell.closeWindow).toHaveBeenCalled();
  });

  it("forgets a project without opening it", async () => {
    const { ws, shell, calls } = fakes([project("gone", false)]);
    const root = document.body.appendChild(document.createElement("div"));
    dispose = render(() => <StartScreen ws={ws} shell={shell} />, root);
    await settle();

    (root.querySelector(".recent-forget") as HTMLButtonElement).click();
    await settle();
    expect(calls).toContainEqual(["forget_project", { path: "/w/gone" }]);
    expect(root.querySelectorAll(".recent-item")).toHaveLength(0);
    expect(shell.openProject).not.toHaveBeenCalled();
  });

  it("offers the folder used before projects until there is one", async () => {
    localStorage.setItem("sandhill.root", "/w/old-analysis");
    const { ws, shell } = fakes([]);
    const root = document.body.appendChild(document.createElement("div"));
    dispose = render(() => <StartScreen ws={ws} shell={shell} />, root);
    await settle();

    expect(root.querySelector(".recent-name")!.textContent).toBe("old-analysis");
    (root.querySelector(".recent-open") as HTMLButtonElement).click();
    await settle();
    expect(shell.openProject).toHaveBeenCalledWith("local", "/w/old-analysis");
  });
});

describe("openedAgo", () => {
  it("says when, in words", () => {
    const now = new Date(2026, 9, 11, 15, 0).getTime();
    expect(openedAgo(new Date(2026, 9, 11, 9, 0).getTime(), now)).toBe("opened today");
    expect(openedAgo(new Date(2026, 9, 10, 23, 0).getTime(), now)).toBe("opened yesterday");
    expect(openedAgo(new Date(2026, 9, 8, 12, 0).getTime(), now)).toBe("opened 3 days ago");
  });
});
