import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, SessionInfo, TableFilter, TableRows, TableSort, TableSummary } from "../backend/types";
import { Workspace } from "../model/workspace";
import { DataExplorer, type SavedTable, savedTable, tableTab } from "./DataExplorer";
import { Dialogs } from "./dialogs";

const PROJECT = "/home/huan/pbmc";
const SESSION: SessionInfo = {
  id: "s1",
  kernel: "ir",
  language: "R",
  cwd: PROJECT,
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 0,
  project: PROJECT,
  pending_runs: 0,
  language_version: "4.4.1",
};

const COLUMNS = [
  { name: "cell", kind: "text", type: "chr" },
  { name: "cluster", kind: "category", type: "fct" },
  { name: "n_genes", kind: "number", type: "int" },
  { name: "pct", kind: "number", type: "dbl" },
];
const ROWS: [string, (string | number | null)[]][] = [
  ["AAAC-1", ["a", "0", 812, 1.5]],
  ["AAAC-2", ["b", "1", 1043, 4.25]],
  ["AAAC-3", ["c", "0", 655, 0.5]],
  ["AAAC-4", ["d", "2", null, 2]],
  ["AAAC-5", ["e", "1", 990, 7.75]],
];
const SUMMARY: TableSummary = {
  session: "s1",
  path: ["meta"],
  rows: 5,
  columns: [
    { column: 0, missing: 0, distinct: 5, histogram: [], top: ROWS.map(([, r]) => ({ value: r[0] as string, count: 1 })) },
    {
      column: 1,
      missing: 0,
      distinct: 3,
      histogram: [],
      top: [
        { value: "0", count: 2 },
        { value: "1", count: 2 },
        { value: "2", count: 1 },
      ],
    },
    { column: 2, missing: 1, distinct: 4, min: 655, max: 1043, mean: 875, median: 901, histogram: [1, 0, 2, 1], top: [] },
    { column: 3, missing: 0, distinct: 5, min: 0.5, max: 7.75, mean: 3.2, median: 2, histogram: [2, 1, 1, 1], top: [] },
  ],
};

/** The table as the kernel would filter it, for the filters the tests use. */
function passes(row: (string | number | null)[], filters: TableFilter[]): boolean {
  return filters.every((f) => {
    const v = row[f.column];
    if (f.op === "<") return v !== null && Number(v) < Number(f.values![0]);
    if (f.op === "in") return v !== null && f.values!.includes(String(v));
    return true;
  });
}

/** A table of 2 rows and `width` number columns, `g0` on, read a block of columns at a time. */
function wideRows(params: Record<string, unknown>, width: number): TableRows {
  const from = params.column_offset as number;
  const to = Math.min(width, from + (params.column_limit as number));
  const columns = Array.from({ length: width }, (_, c) => ({ name: `g${c}`, kind: "number", type: "dbl" }));
  const cells = (r: number) => Array.from({ length: Math.max(0, to - from) }, (_, j) => r * 1000 + from + j);
  return { session: "s1", path: ["meta"], code: "wide", columns: params.cells_only ? [] : columns, column_offset: from, width, total: 2, rows: 2, offset: 0, labels: ["r0", "r1"], values: [cells(0), cells(1)] };
}

function wideSummary(params: Record<string, unknown>, width: number): TableSummary {
  const from = params.column_offset as number;
  const to = Math.min(width, from + (params.column_limit as number));
  const columns = Array.from({ length: Math.max(0, to - from) }, (_, j) => ({ column: from + j, missing: 0, distinct: 2, min: from + j, max: 1000 + from + j, histogram: [1, 1], top: [] }));
  return { session: "s1", path: ["meta"], rows: 2, columns };
}

function fakeWorkspace(opts: { pending?: number; wide?: number } = {}) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  /** Set to put a column in front, as code that changes the table would. */
  const shape = { extra: false };
  let emit: (event: DaemonEvent) => void = () => {};
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      const off = shape.extra ? 1 : 0;
      const filters = ((params.filters as TableFilter[]) ?? []).map((f) => ({ ...f, column: f.column - off }));
      const kept = ROWS.filter(([, r]) => passes(r, filters));
      switch (method) {
        case "list_sessions":
          return [{ ...SESSION, pending_runs: opts.pending ?? 0 }] as T;
        case "table_rows": {
          if (opts.wide) return wideRows(params, opts.wide) as T;
          const sort = ((params.sort as TableSort[]) ?? []).map((k) => ({ ...k, column: k.column - off }));
          const sorted = [...kept];
          for (const s of [...sort].reverse()) {
            sorted.sort((a, b) => {
              const [x, y] = [a[1][s.column], b[1][s.column]];
              if (x === null) return 1;
              if (y === null) return -1;
              return (x < y ? -1 : x > y ? 1 : 0) * (s.descending ? -1 : 1);
            });
          }
          const page = sorted.slice(params.offset as number, (params.offset as number) + (params.limit as number));
          const read: TableRows = {
            session: "s1",
            path: params.path as string[],
            code: "meta",
            columns: shape.extra ? [{ name: "score", kind: "number", type: "dbl" }, ...COLUMNS] : COLUMNS,
            width: 4 + off,
            total: ROWS.length,
            rows: kept.length,
            offset: params.offset as number,
            labels: page.map(([label]) => label),
            values: page.map(([, r]) => (shape.extra ? [9, ...r] : r)),
          };
          return read as T;
        }
        case "table_summary":
          if (opts.wide) return wideSummary(params, opts.wide) as T;
          return { ...SUMMARY, rows: kept.length } as T;
        case "stat_files":
          return (params.paths as string[]).map((path) => ({ path, size: 0 })) as T;
        case "table_export":
          return { path: params.file, mtime_ms: 1 } as T;
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
  const ws = new Workspace(backend, PROJECT);
  return { ws, calls, shape, emit: (event: DaemonEvent) => emit(event) };
}

const settle = async () => {
  for (let i = 0; i < 3; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

async function mount(opts: { pending?: number; wide?: number; saved?: SavedTable; extra?: boolean } = {}) {
  const fake = fakeWorkspace(opts);
  fake.shape.extra = !!opts.extra;
  await fake.ws.refreshSessions();
  const tab = tableTab(SESSION, ["meta"], opts.saved);
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(
    () => (
      <>
        <DataExplorer ws={fake.ws} tab={tab} project={PROJECT} hostLabel="This Mac" shortName={(p) => p.slice(PROJECT.length + 1)} shown />
        <Dialogs />
      </>
    ),
    root,
  );
  await settle();
  return { ...fake, tab, root, dispose };
}

const texts = (root: ParentNode, selector: string) => [...root.querySelectorAll(selector)].map((e) => e.textContent);
const reads = (calls: { method: string; params: Record<string, unknown> }[]) => calls.filter((c) => c.method === "table_rows");
const button = (root: ParentNode, text: string) => [...root.querySelectorAll("button")].find((b) => b.textContent?.trim() === text) as HTMLButtonElement;

afterEach(() => {
  vi.restoreAllMocks();
  document.body.replaceChildren();
  localStorage.clear();
});

describe("DataExplorer", () => {
  it("shows the table's columns, rows and the picked column's numbers", async () => {
    const { root, calls, tab, dispose } = await mount();
    expect(reads(calls)[0].params).toEqual({
      session: "s1",
      path: ["meta"],
      offset: 0,
      limit: 200,
      sort: [],
      filters: [],
      column_offset: 0,
      column_limit: 100,
      cells_only: false,
    });
    expect(calls.find((c) => c.method === "table_summary")?.params).toEqual({ session: "s1", path: ["meta"], filters: [], column_offset: 0, column_limit: 100 });
    expect(texts(root, ".de-col b")).toEqual(["cell", "cluster", "n_genes", "pct"]);
    expect(texts(root, ".de-col small")).toEqual(["chr · 5 values", "fct · 3 values", "int · 1 missing", "dbl"]);
    expect(texts(root, ".de-th .de-name")).toEqual(["cell", "cluster", "n_genes", "pct"]);
    expect(texts(root, ".de-rows .de-rn")).toEqual(["AAAC-1", "AAAC-2", "AAAC-3", "AAAC-4", "AAAC-5"]);
    const first = root.querySelector(".de-rows .de-tr")!;
    expect(texts(first, ".de-td")).toEqual(["a", "0", "812", "1.5"]);
    // A missing number reads as R writes it.
    expect(root.querySelectorAll(".de-rows .de-tr")[3].querySelectorAll(".de-td")[2].textContent).toBe("NA");
    // The cluster's values get colors.
    expect(first.querySelectorAll(".de-td")[1].querySelector(".ct")).not.toBeNull();
    expect(root.querySelector(".de-bar .count")?.textContent).toBe("5 rows · 4 columns");
    expect(root.querySelector(".de-what")?.textContent).toBe("4.4.1 · meta, live");
    expect([tab.code(), tab.count()]).toEqual(["meta", "5 rows"]);
    // A column picked at the side shows its numbers.
    (root.querySelectorAll(".de-col")[2] as HTMLButtonElement).click();
    expect(root.querySelector(".de-stats h4")?.textContent).toBe("n_genes");
    expect(texts(root, ".de-stats dt")).toEqual(["Min", "Median", "Mean", "Max", "Missing (NA)"]);
    expect(texts(root, ".de-stats dd")).toEqual(["655", "901", "875", "1,043", "1"]);
    dispose();
  });

  it("reads a wide table's cells and stats a block of columns at a time", async () => {
    const { root, calls, dispose } = await mount({ wide: 250 });
    expect(root.querySelector(".de-bar .count")?.textContent).toBe("2 rows · 250 columns");
    expect(reads(calls).map((c) => c.params.column_offset)).toEqual([0]);
    expect(texts(root.querySelector(".de-rows .de-tr")!, ".de-td").slice(0, 3)).toEqual(["0", "1", "2"]);
    // Scrolled to the right end, the last block's cells are read, and its stats.
    const grid = root.querySelector(".de-grid") as HTMLElement;
    grid.scrollLeft = 1e6;
    grid.dispatchEvent(new Event("scroll"));
    await new Promise((resolve) => setTimeout(resolve, 150));
    await settle();
    expect(reads(calls).at(-1)?.params).toMatchObject({ column_offset: 200, cells_only: true });
    expect(texts(root.querySelector(".de-rows .de-tr")!, ".de-td").at(-1)).toBe("249");
    expect(calls.filter((c) => c.method === "table_summary").map((c) => c.params.column_offset)).toEqual([0, 200]);
    dispose();
  });

  it("sorts by a column's name, and by a second with shift", async () => {
    const { root, calls, dispose } = await mount();
    const header = (i: number) => root.querySelectorAll(".de-th")[i] as HTMLElement;
    header(2).click();
    await settle();
    expect(reads(calls).at(-1)?.params.sort).toEqual([{ column: 2, descending: false }]);
    expect(texts(root, ".de-rows .de-rn")).toEqual(["AAAC-3", "AAAC-1", "AAAC-5", "AAAC-2", "AAAC-4"]);
    header(2).click();
    await settle();
    expect(header(2).querySelector(".srt")?.textContent).toBe("▼");
    expect(root.querySelector(".de-foot span")?.textContent).toBe("Sorted by n_genes, descending");
    header(1).dispatchEvent(new MouseEvent("click", { shiftKey: true, bubbles: true }));
    await settle();
    expect(reads(calls).at(-1)?.params.sort).toEqual([
      { column: 2, descending: true },
      { column: 1, descending: false },
    ]);
    expect(texts(root, ".de-th .srt")).toEqual(["▲2", "▼1"]);
    dispose();
  });

  it("adds a filter, changes it, and takes it off", async () => {
    const { root, calls, tab, dispose } = await mount();
    (root.querySelectorAll(".de-col")[3] as HTMLButtonElement).click();
    button(root, "Add filter").click();
    const popup = document.querySelector(".de-filter") as HTMLFormElement;
    expect(popup).not.toBeNull();
    // A number column starts at "<" its median.
    const [column, condition] = popup.querySelectorAll("select");
    expect([column.value, condition.value]).toEqual(["3", "<"]);
    const value = popup.querySelector("input[aria-label='Value']") as HTMLInputElement;
    expect(value.value).toBe("2");
    value.value = "five";
    value.dispatchEvent(new Event("input", { bubbles: true }));
    popup.requestSubmit();
    expect(popup.querySelector(".df-err")?.textContent).toBe("Give a number.");
    value.value = "5";
    value.dispatchEvent(new Event("input", { bubbles: true }));
    popup.requestSubmit();
    await settle();
    expect(document.querySelector(".de-filter")).toBeNull();
    expect(reads(calls).at(-1)?.params.filters).toEqual([{ column: 3, op: "<", values: ["5"] }]);
    expect(texts(root, ".chip-text")).toEqual(["pct < 5"]);
    expect(root.querySelector(".de-bar .count")?.textContent).toBe("4 of 5 rows · 4 columns");
    expect(tab.count()).toBe("4 of 5 rows");
    expect(calls.filter((c) => c.method === "table_summary").at(-1)?.params.filters).toEqual([{ column: 3, op: "<", values: ["5"] }]);

    // A category's values are ticked.
    button(root, "Add filter").click();
    const next = document.querySelector(".de-filter") as HTMLFormElement;
    const pick = next.querySelector("select") as HTMLSelectElement;
    pick.value = "1";
    pick.dispatchEvent(new Event("change"));
    expect((next.querySelectorAll("select")[1] as HTMLSelectElement).value).toBe("in");
    expect(texts(next, ".df-choices span")).toEqual(["0", "1", "2"]);
    (next.querySelectorAll(".df-choices input")[1] as HTMLInputElement).click();
    next.requestSubmit();
    await settle();
    expect(texts(root, ".chip-text")).toEqual(["pct < 5", "cluster is one of 1"]);
    expect(texts(root, ".de-rows .de-rn")).toEqual(["AAAC-2"]);

    // Clicking a chip changes it.
    (root.querySelectorAll(".chip-text")[0] as HTMLButtonElement).click();
    const edit = document.querySelector(".de-filter") as HTMLFormElement;
    expect(button(edit, "Change filter")).toBeDefined();
    const input = edit.querySelector("input[aria-label='Value']") as HTMLInputElement;
    expect(input.value).toBe("5");
    input.value = "10";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    edit.requestSubmit();
    await settle();
    expect(texts(root, ".chip-text")).toEqual(["pct < 10", "cluster is one of 1"]);

    (root.querySelector(".chip-x") as HTMLButtonElement).click();
    await settle();
    expect(reads(calls).at(-1)?.params.filters).toEqual([{ column: 1, op: "in", values: ["1"] }]);
    dispose();
  });

  it("filters to a value clicked in the column's stats", async () => {
    const { root, calls, dispose } = await mount();
    (root.querySelectorAll(".de-col")[1] as HTMLButtonElement).click();
    expect(texts(root, ".de-val")).toEqual(["0", "1", "2"]);
    (root.querySelectorAll(".de-val")[2] as HTMLButtonElement).click();
    await settle();
    (root.querySelectorAll(".de-val")[0] as HTMLButtonElement).click();
    await settle();
    expect(reads(calls).at(-1)?.params.filters).toEqual([{ column: 1, op: "in", values: ["2", "0"] }]);
    expect(texts(root, ".chip-text")).toEqual(["cluster is one of 2, 0"]);
    dispose();
  });

  it("reads the table again after a run in its session, not after a quiet one", async () => {
    const { root, calls, emit, dispose } = await mount();
    const before = reads(calls).length;
    let seq = 0;
    const run = (id: string, quiet = false) => {
      emit({ seq: ++seq, session: "s1", run: id, msg_type: "sandhill.run_started", ts_ms: seq, content: { code: "x", quiet } });
      emit({ seq: ++seq, session: "s1", run: id, msg_type: "sandhill.run_finished", ts_ms: seq, content: { status: "ok" } });
    };
    run("q1", true);
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(reads(calls).length).toBe(before);
    emit({ seq: ++seq, session: "s1", run: "r1", msg_type: "sandhill.run_started", ts_ms: seq, content: { code: "meta$x <- 1" } });
    expect(root.querySelector(".de-note")?.textContent).toBe("R is running code; the table updates when it finishes.");
    emit({ seq: ++seq, session: "s1", run: "r1", msg_type: "sandhill.run_finished", ts_ms: seq, content: { status: "ok" } });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(reads(calls).length).toBeGreaterThan(before);
    expect(root.querySelector(".de-note")).toBeNull();
    dispose();
  });

  it("keeps filters and sorting on their columns when the table changes", async () => {
    const { root, calls, emit, shape, dispose } = await mount();
    (root.querySelectorAll(".de-col")[1] as HTMLButtonElement).click();
    (root.querySelectorAll(".de-val")[0] as HTMLButtonElement).click();
    await settle();
    (root.querySelectorAll(".de-th")[2] as HTMLElement).click();
    await settle();
    expect(texts(root, ".de-rows .de-rn")).toEqual(["AAAC-3", "AAAC-1"]);
    // Code puts a column in front of them.
    shape.extra = true;
    emit({ seq: 1, session: "s1", run: "r1", msg_type: "sandhill.run_started", ts_ms: 1, content: { code: "meta <- cbind(score = 9, meta)" } });
    emit({ seq: 2, session: "s1", run: "r1", msg_type: "sandhill.run_finished", ts_ms: 2, content: { status: "ok" } });
    await new Promise((resolve) => setTimeout(resolve, 300));
    await settle();
    const last = reads(calls).at(-1)!.params;
    expect([last.filters, last.sort]).toEqual([[{ column: 2, op: "in", values: ["0"] }], [{ column: 3, descending: false }]]);
    expect(texts(root, ".chip-text")).toEqual(["cluster is one of 0"]);
    expect(texts(root, ".de-th .de-name")).toEqual(["score", "cell", "cluster", "n_genes▲", "pct"]);
    expect(texts(root, ".de-rows .de-rn")).toEqual(["AAAC-3", "AAAC-1"]);
    dispose();
  });

  it("comes back with its filters and sorting, on their columns if the table changed", async () => {
    const saved: SavedTable = {
      kind: "table",
      session: "s1",
      language: "r",
      path: ["meta"],
      sort: [{ column: 2, descending: true, name: "n_genes" }],
      filters: [{ column: 1, op: "in", values: ["0"], name: "cluster" }],
    };
    // Code put a column in front while the window was away.
    const { root, calls, tab, dispose } = await mount({ saved, extra: true });
    const last = reads(calls).at(-1)!.params;
    expect([last.filters, last.sort]).toEqual([[{ column: 2, op: "in", values: ["0"] }], [{ column: 3, descending: true }]]);
    expect(texts(root, ".chip-text")).toEqual(["cluster is one of 0"]);
    expect(texts(root, ".de-rows .de-rn")).toEqual(["AAAC-1", "AAAC-3"]);
    expect(savedTable(tab, "s1")).toEqual({ ...saved, sort: [{ column: 3, descending: true, name: "n_genes" }], filters: [{ column: 2, op: "in", values: ["0"], name: "cluster" }] });
    dispose();
  });

  it("waits for a busy session before reading", async () => {
    const { root, calls, dispose } = await mount({ pending: 1 });
    expect(reads(calls).length).toBe(0);
    expect(root.querySelector(".de-note")?.textContent).toMatch(/^R is running code/);
    dispose();
  });

  it("exports the rows shown as CSV, and copies code that does the same", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { root, calls, dispose } = await mount();
    (root.querySelectorAll(".de-th")[2] as HTMLElement).click();
    await settle();
    button(root, "Copy as code").click();
    await settle();
    expect(writeText).toHaveBeenCalledWith("x <- meta\nx <- x[order(x$n_genes), , drop = FALSE]\n");
    button(root, "Export CSV").click();
    await settle();
    const field = document.querySelector(".modal-input") as HTMLInputElement;
    expect(field.value).toBe("meta.csv");
    field.value = "results/meta.csv";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    (document.querySelector("form.modal") as HTMLFormElement).requestSubmit();
    await settle();
    expect(calls.find((c) => c.method === "table_export")?.params).toEqual({
      session: "s1",
      path: ["meta"],
      sort: [{ column: 2, descending: false }],
      filters: [],
      file: `${PROJECT}/results/meta.csv`,
    });
    expect(localStorage.getItem(`sandhill.exportFolder:${PROJECT}`)).toBe("results");
    dispose();
  });
});
