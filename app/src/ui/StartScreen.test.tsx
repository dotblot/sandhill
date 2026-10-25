import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { LinkState, Nav, Shell } from "../backend";
import type { ProjectInfo, SessionInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { openedAgo, StartScreen } from "./StartScreen";

function project(name: string, exists = true): ProjectInfo {
  return { path: `/w/${name}`, name, opened_ms: Date.now(), exists, state: null };
}

function fakes(projects: ProjectInfo[], host = "local", sessions: Partial<SessionInfo>[] = [], initialServers: string[] = []) {
  const calls: [string, unknown][] = [];
  const [link] = createSignal<LinkState>({ status: "connected" });
  const ws = {
    backend: { host },
    link,
    sessions: () => sessions,
    host: () => ({ home: "/w", hostname: host === "local" ? "mac" : "lab-01", daemon_version: "0.1.0", protocol_version: 1 }),
    call: async (method: string, params?: { path?: string }) => {
      calls.push([method, params]);
      if (method === "list_recent_projects") return projects;
      if (method === "open_project") return project(params!.path!.split("/").pop()!);
      if (method === "list_dir") return [];
      if (method === "forget_project") {
        projects = projects.filter((p) => p.path !== params!.path);
        return null;
      }
      throw new Error(`unexpected ${method}`);
    },
  } as unknown as Workspace;
  let servers = initialServers;
  const shell = {
    listServers: async () => servers,
    addServer: async (h: string) => (servers = [...servers, h]),
    removeServer: async (h: string) => (servers = servers.filter((s) => s !== h)),
  } as unknown as Shell;
  const nav = { openProject: vi.fn(), showStart: vi.fn() } satisfies Nav;
  return { ws, shell, nav, calls };
}

/** The start screen with `ws` for this Mac and `others` for servers. */
function startScreen(ws: Workspace, { shell, nav }: { shell: Shell; nav: Nav }, others: Record<string, Workspace> = {}) {
  const root = document.body.appendChild(document.createElement("div"));
  const workspace = async (host: string) => (host === "local" ? ws : others[host]);
  const dispose = render(() => <StartScreen shell={shell} nav={nav} workspace={workspace} />, root);
  return { root, dispose };
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
    const { ws, shell, nav, calls } = fakes([project("atlas"), project("gone", false)]);
    let root: HTMLElement;
    ({ root, dispose } = startScreen(ws, { shell, nav }));
    await settle();

    const items = [...root.querySelectorAll(".recent-item")];
    expect(items.map((i) => i.querySelector(".recent-name")!.textContent)).toEqual(["atlas", "gone"]);
    expect((items[1].querySelector(".recent-open") as HTMLButtonElement).disabled).toBe(true);

    (items[0].querySelector(".recent-open") as HTMLButtonElement).click();
    await settle();
    expect(calls).toContainEqual(["open_project", { path: "/w/atlas" }]);
    expect(nav.openProject).toHaveBeenCalledWith("local", "/w/atlas");
  });

  it("forgets a project without opening it", async () => {
    const { ws, shell, nav, calls } = fakes([project("gone", false)]);
    let root: HTMLElement;
    ({ root, dispose } = startScreen(ws, { shell, nav }));
    await settle();

    (root.querySelector(".recent-forget") as HTMLButtonElement).click();
    await settle();
    expect(calls).toContainEqual(["forget_project", { path: "/w/gone" }]);
    expect(root.querySelectorAll(".recent-item")).toHaveLength(0);
    expect(nav.openProject).not.toHaveBeenCalled();
  });

  it("offers the folder used before projects until there is one", async () => {
    localStorage.setItem("sandhill.root", "/w/old-analysis");
    const { ws, shell, nav } = fakes([]);
    let root: HTMLElement;
    ({ root, dispose } = startScreen(ws, { shell, nav }));
    await settle();

    expect(root.querySelector(".recent-name")!.textContent).toBe("old-analysis");
    (root.querySelector(".recent-open") as HTMLButtonElement).click();
    await settle();
    expect(nav.openProject).toHaveBeenCalledWith("local", "/w/old-analysis");
  });
});

describe("StartScreen with servers", () => {
  let dispose = () => {};
  afterEach(() => {
    dispose();
    document.body.replaceChildren();
    localStorage.clear();
  });

  const card = (root: HTMLElement, host: string) => root.querySelector(`.host-card[data-host="${host}"]`)!;

  it("adds a server, lists its projects and opens one there", async () => {
    const local = fakes([project("here")]);
    const lab = fakes([project("atlas")], "lab");
    let root: HTMLElement;
    ({ root, dispose } = startScreen(local.ws, local, { lab: lab.ws }));
    await settle();
    expect(card(root, "local").querySelector(".recent-name")!.textContent).toBe("here");

    (root.querySelector(".start-actions .connect") as HTMLButtonElement).click();
    await settle();
    const input = root.querySelector(".add-server input") as HTMLInputElement;
    input.value = "lab";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    root.querySelector<HTMLFormElement>(".add-server")!.requestSubmit();
    await settle();
    await settle();

    // Servers come first, then this Mac, each with its own projects.
    expect([...root.querySelectorAll(".host-name")].map((n) => n.textContent)).toEqual(["lab", "This Mac"]);
    expect(root.querySelector(".add-server")).toBeNull();
    (card(root, "lab").querySelector(".recent-open") as HTMLButtonElement).click();
    await settle();
    expect(local.nav.openProject).toHaveBeenCalledWith("lab", "/w/atlas");
  });

  it("says which projects have kernels running", async () => {
    const local = fakes([project("atlas"), project("quiet")], "local", [
      { id: "a", project: "/w/atlas", status: "running" },
      { id: "b", project: "/w/atlas", status: "running" },
      { id: "c", project: "/w/quiet", status: "ended" },
    ]);
    let root: HTMLElement;
    ({ root, dispose } = startScreen(local.ws, local));
    await settle();

    const when = [...root.querySelectorAll(".recent-when")].map((w) => w.textContent);
    expect(when).toEqual(["2 kernels", "opened today"]);
  });

  it("removes a server", async () => {
    const local = fakes([], "local", [], ["lab"]);
    const lab = fakes([project("atlas")], "lab");
    let root: HTMLElement;
    ({ root, dispose } = startScreen(local.ws, local, { lab: lab.ws }));
    await settle();
    await settle();

    (card(root, "lab").querySelector(".host-remove") as HTMLButtonElement).click();
    await settle();
    expect([...root.querySelectorAll(".host-name")].map((n) => n.textContent)).toEqual(["This Mac"]);
    expect(card(root, "local").querySelector(".host-remove")).toBeNull();
  });

  it("opens the folder picker on the server used last", async () => {
    const local = fakes([], "local", [], ["lab", "hpc"]);
    const others = { lab: fakes([], "lab").ws, hpc: fakes([], "hpc").ws };
    let root: HTMLElement;
    ({ root, dispose } = startScreen(local.ws, local, others));
    await settle();
    await settle();

    const picker = () => root.querySelector(".picker-head select") as HTMLSelectElement | null;
    const openFolder = () => (root.querySelector(".start-actions .primary") as HTMLButtonElement).click();
    openFolder();
    await settle();
    expect(picker()!.value).toBe("lab");
    openFolder();
    expect(picker()).toBeNull();

    localStorage.setItem("sandhill.startHost", "hpc");
    openFolder();
    await settle();
    expect(picker()!.value).toBe("hpc");
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
