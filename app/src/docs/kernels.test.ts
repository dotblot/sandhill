import { describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { KernelspecInfo, ProjectSetup, SessionInfo } from "../backend/types";
import { type KernelWanted, Workspace } from "../model/workspace";
import { DocKernels } from "./kernels";

const SPECS: KernelspecInfo[] = [
  { name: "ark", display_name: "R (Ark)", language: "R" },
  { name: "python3", display_name: "Python 3", language: "python" },
  { name: "conda-env-omicverse-py", display_name: "Python (omicverse)", language: "python", env: "omicverse" },
];

/** A daemon that starts sessions instantly and counts them. */
function fakeDaemon(sessions: SessionInfo[] = [], setup?: ProjectSetup) {
  let next = 0;
  const started: string[] = [];
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      if (method === "list_sessions") return sessions.map((s) => ({ ...s })) as T;
      if (method === "list_kernelspecs" || method === "list_project_kernelspecs") return SPECS as T;
      if (method === "get_project_settings") return (setup ?? null) as T;
      if (method === "start_session") {
        const spec = SPECS.find((k) => k.name === params.kernel)!;
        const info: SessionInfo = {
          id: `s${++next}`,
          kernel: spec.name,
          language: spec.language,
          cwd: params.cwd as string,
          status: "running",
          created_ms: next,
          last_seq: 0,
          project: params.project as string | undefined,
        };
        sessions.push(info);
        started.push(info.id);
        return info as T;
      }
      if (method === "shutdown") {
        const s = sessions.find((s) => s.id === params.session)!;
        s.status = "ended";
        return null as T;
      }
      if (method === "execute") return { run: `run-${params.session}` } as T;
      return null as T;
    },
    onEvent() {},
    onTerminal() {},
    onRequest() {},
    openLink() {},
    onLink() {},
    blobUrl: () => "",
    reconnect: async () => {},
    disconnect: async () => {},
  };
  return { backend, sessions, started };
}

async function workspace(sessions?: SessionInfo[], setup?: ProjectSetup) {
  const daemon = fakeDaemon(sessions, setup);
  const ws = new Workspace(daemon.backend, "/p");
  await ws.refreshKernelspecs();
  await ws.refreshSessions();
  await ws.refreshSetup();
  return { ws, ...daemon };
}

/** A project whose settings say `settings`, and whose kernels are as given. */
function projectSetup(settings: Partial<ProjectSetup["settings"]>, kernels: { r?: string; python?: string } = {}): ProjectSetup {
  return { settings: { documents: "document", env_files: [".env"], ...settings }, saved: true, found: [], ...kernels };
}

const doc = (ws: Workspace, dir = "/p", wanted?: KernelWanted) => new DocKernels(ws, () => dir, () => wanted);

function running(id: string, kernel: string, extra: Partial<SessionInfo> = {}): SessionInfo {
  const language = SPECS.find((k) => k.name === kernel)!.language;
  return { id, kernel, language, cwd: "/p", status: "running", created_ms: 0, last_seq: 0, project: "/p", ...extra };
}

describe("DocKernels", () => {
  it("lets files in the same folder share one session per language", async () => {
    const { ws, started } = await workspace();
    const a = doc(ws);
    const b = doc(ws);
    const first = await a.ensure("python");
    expect(await b.ensure("python")).toBe(first);
    expect(await b.ensure("r")).not.toBe(first);
    expect(await a.ensure("r")).toBe(await b.ensure("r"));
    expect(started).toHaveLength(2);
  });

  it("starts one session when two files run code at the same time", async () => {
    const { ws, started } = await workspace();
    const [x, y] = await Promise.all([doc(ws).ensure("python"), doc(ws).ensure("python")]);
    expect(x).toBe(y);
    expect(started).toHaveLength(1);
  });

  it("joins a session left running by an earlier visit, the newest one", async () => {
    const { ws, started } = await workspace([
      running("old", "conda-env-omicverse-py", { created_ms: 1 }),
      running("new", "conda-env-omicverse-py", { created_ms: 2 }),
    ]);
    expect(await doc(ws).ensure("python")).toBe("new");
    expect(started).toHaveLength(0);
  });

  it("prefers the session this window last ran code in", async () => {
    const { ws } = await workspace([
      running("old", "python3", { created_ms: 1 }),
      running("new", "python3", { created_ms: 2 }),
    ]);
    await ws.execute("old", "1");
    expect(await doc(ws).ensure("python")).toBe("old");
  });

  it("only joins sessions of this project and folder", async () => {
    const { ws, started } = await workspace([
      running("other-project", "python3", { project: "/elsewhere" }),
      running("no-project", "python3", { project: undefined }),
      running("subfolder", "python3", { cwd: "/p/analysis" }),
      running("ended", "python3", { status: "ended" }),
    ]);
    const id = await doc(ws).ensure("python");
    expect(started).toEqual([id]);
    expect(await doc(ws, "/p/analysis").ensure("python")).toBe("subfolder");
  });

  it("joins only sessions of the kernel a notebook names", async () => {
    const { ws, started } = await workspace([running("base", "python3")]);
    const notebook = doc(ws, "/p", { name: "conda-env-omicverse-py", display_name: "Python (omicverse)" });
    const id = await notebook.ensure("python");
    expect(id).not.toBe("base");
    expect(ws.sessions().find((s) => s.id === id)?.kernel).toBe("conda-env-omicverse-py");
    expect(started).toEqual([id]);
    // A file that names no kernel joins whichever ran last.
    await ws.execute(id, "1");
    expect(await doc(ws).ensure("python")).toBe(id);
  });

  it("runs the project's kernel, over the one a notebook names", async () => {
    const setup = projectSetup({ python: "conda-env-omicverse-py" }, { python: "conda-env-omicverse-py" });
    const { ws } = await workspace([running("base", "python3")], setup);
    const notebook = doc(ws, "/p", { name: "python3", display_name: "Python 3 (ipykernel)" });
    const id = await notebook.ensure("python");
    expect(id).not.toBe("base");
    expect(ws.sessions().find((s) => s.id === id)?.kernel).toBe("conda-env-omicverse-py");
    // R isn't set: the usual kernel.
    const r = await notebook.ensure("r");
    expect(ws.sessions().find((s) => s.id === r)?.kernel).toBe("ark");
  });

  it("runs documents in the project's folder when the project says so", async () => {
    const { ws } = await workspace([], projectSetup({ documents: "project" }));
    expect(ws.documentFolder("/p/analysis")).toBe("/p");
    const id = await new DocKernels(ws, () => ws.documentFolder("/p/analysis")).ensure("python");
    expect(ws.sessions().find((s) => s.id === id)?.cwd).toBe("/p");
    const other = await workspace([], projectSetup({ documents: "document" }));
    expect(other.ws.documentFolder("/p/analysis")).toBe("/p/analysis");
  });

  it("says so when the project's kernel isn't installed here", async () => {
    const { ws, started } = await workspace([], projectSetup({ python: "conda-env-gone-py" }, { python: "conda-env-gone-py" }));
    await expect(doc(ws).ensure("python")).rejects.toThrow(/conda-env-gone-py \(set in \.sandhill\/project\.toml\)/);
    expect(started).toEqual([]);
  });

  it("starts a separate session when asked for a new one", async () => {
    const { ws } = await workspace();
    const a = doc(ws);
    const shared = await a.ensure("python");
    const own = await a.startNew("python", "python3");
    expect(own).not.toBe(shared);
    expect(a.session("python")).toBe(own);
  });

  it("moves every file to the new session after a restart", async () => {
    const { ws } = await workspace();
    const a = doc(ws);
    const b = doc(ws);
    const first = await a.ensure("python");
    await b.ensure("python");
    await a.restart("python");
    const fresh = a.session("python");
    expect(fresh).toBeDefined();
    expect(fresh).not.toBe(first);
    expect(b.session("python")).toBe(fresh);
    expect(b.sessions()).toEqual([fresh]);
  });
});

describe("Workspace.isBusy", () => {
  it("uses the daemon's count for sessions this window does not follow", async () => {
    const { ws } = await workspace([running("busy", "python3", { pending_runs: 1 }), running("idle", "python3")]);
    expect(ws.isBusy("busy")).toBe(true);
    expect(ws.isBusy("idle")).toBe(false);
  });
});
