import { type Accessor, batch, createEffect, createMemo, createSignal, For, on, onCleanup, Show, untrack } from "solid-js";
import { Portal } from "solid-js/web";
import type { ColumnSummary, FileStat, SavedExtra, SessionInfo, TableColumn, TableFilter, TableRows, TableSort, TableSummary } from "../backend/types";
import { documentPath } from "../model/agents";
import { languageLabel, languageVersion } from "../model/machine";
import {
  columnWidth,
  exportName,
  filterError,
  filterText,
  formatCell,
  formatNumber,
  missingText,
  nextSort,
  opsFor,
  PAGE,
  sortText,
  tableCode,
} from "../model/table";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { ask, askText, notify } from "./dialogs";
import { Icon, LangIcon } from "./icons";
import { closeOnOutside } from "./popup";

/** A row's height in the grid, in px. */
const ROW = 24;
/** Rows drawn above and below the ones on screen. */
const OVERSCAN = 8;
/** The header row's height, in px. */
const HEAD = 36;
/**
 * The tallest the grid gets, in px. A taller table scrolls by proportion:
 * WebKit can't lay out much past 33 million px, about 1.4 million rows.
 */
const MAX_HEIGHT = 8_000_000;
/** Columns read at a time, for a page's cells or the columns' stats. */
const BLOCK = 100;
/** Blocks of cells kept; farther ones are read again if scrolled back to. */
const KEEP_PAGES = 24;
/** A row of the column list, in px, with the gap under it. */
const SIDE_ROW = 39;
/** How long scrolling rests before rows are read, and a run's end before the table is. */
const WAIT_MS = 120;
const REFRESH_MS = 250;
/** What the daemon says when the session runs other code. */
const BUSY = "the session is busy running code";
/** Colors of a category's values, as in the mockup. */
const COLORS = ["#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7", "#9c755f", "#bab0ac"];

/** A table open in the editor: a data frame or matrix in a session, by its path in the Variables pane. */
export interface TableTab {
  kind: "table";
  id: string;
  session: string;
  language: string;
  path: string[];
  /** The table as code (`seu@meta.data`), once read; until then its path. */
  code: Accessor<string>;
  setCode: (code: string) => void;
  /** "618 of 2,638 rows", for the status bar. */
  count: Accessor<string>;
  setCount: (count: string) => void;
  /** How its rows are sorted and filtered, kept with the project's tabs. */
  sort: Accessor<TableSort[]>;
  setSort: (sort: TableSort[]) => void;
  filters: Accessor<TableFilter[]>;
  setFilters: (filters: TableFilter[]) => void;
  /** The columns last read, to keep the names of those sorted and filtered on. */
  columns: Accessor<{ name: string }[]>;
  setColumns: (columns: { name: string }[]) => void;
  /**
   * For a tab brought back, the names of the columns its sorting and
   * filters were on, by position, so they follow their columns if the
   * table changed meanwhile.
   */
  named?: { name: string }[];
}

/** A table tab as the project keeps it. */
export type SavedTable = Extract<SavedExtra, { kind: "table" }>;

export function tableTab(session: Pick<SessionInfo, "id" | "language">, path: string[], saved?: Pick<SavedTable, "sort" | "filters">): TableTab {
  const [code, setCode] = createSignal(path.join(normalizeLanguage(session.language) === "r" ? "$" : "."));
  const [count, setCount] = createSignal("");
  const keys = (list: { column: number; name: string }[] | undefined) => (list ?? []).map(({ name: _, ...key }) => key);
  const [sort, setSort] = createSignal<TableSort[]>(keys(saved?.sort));
  const [filters, setFilters] = createSignal<TableFilter[]>(keys(saved?.filters) as TableFilter[]);
  const [columns, setColumns] = createSignal<{ name: string }[]>([]);
  let named: { name: string }[] | undefined;
  for (const key of [...(saved?.sort ?? []), ...(saved?.filters ?? [])]) (named ??= [])[key.column] = { name: key.name };
  return {
    kind: "table",
    id: tableId(session.id, path),
    session: session.id,
    language: normalizeLanguage(session.language),
    path,
    code,
    setCode,
    count,
    setCount,
    sort,
    setSort,
    filters,
    setFilters,
    columns,
    setColumns,
    named,
  };
}

/** A table tab to keep with the project: its sorting and filters with their columns' names. */
export function savedTable(tab: TableTab, session: string): SavedTable {
  const name = (column: number) => tab.columns()[column]?.name ?? tab.named?.[column]?.name ?? "";
  const saved: SavedTable = { kind: "table", session, language: tab.language, path: tab.path };
  if (tab.sort().length) saved.sort = tab.sort().map((k) => ({ ...k, name: name(k.column) }));
  if (tab.filters().length) saved.filters = tab.filters().map((f) => ({ ...f, name: name(f.column) }));
  return saved;
}

export function tableId(session: string, path: string[]): string {
  return [session, ...path].join("\u0000");
}

type Page = Pick<TableRows, "labels" | "values">;

/**
 * The Data Explorer: a data frame or matrix of a session as a grid, read a
 * page of rows at a time. The columns are listed at the side with a small
 * histogram or bar chart each, and the picked column's numbers under them.
 * Filters and sorting run in the session, so a table of a million cells
 * scrolls like one of a hundred. The table is read again after each run in
 * its session, so it stays live.
 */
export function DataExplorer(props: {
  ws: Workspace;
  tab: TableTab;
  project: string;
  /** The machine files are saved on, like "gpu01". */
  hostLabel: string;
  shortName: (path: string) => string;
  /** False while another tab is in front: nothing is read then. */
  shown: boolean;
}) {
  const ws = props.ws;
  const tab = props.tab;
  const { sort, filters } = tab;
  const setSort = (next: TableSort[] | ((now: TableSort[]) => TableSort[])) => tab.setSort(typeof next === "function" ? next(untrack(sort)) : next);
  const setFilters = (next: TableFilter[] | ((now: TableFilter[]) => TableFilter[])) =>
    tab.setFilters(typeof next === "function" ? next(untrack(filters)) : next);
  /** The last rows read: the columns, counts and code; their cells are in `pages`. */
  const [head, setHead] = createSignal<TableRows>();
  /** Cells read, by page of rows and block of columns (`page:block`). */
  const [pages, setPages] = createSignal<ReadonlyMap<string, Page>>(new Map());
  /** The column summaries read, a block at a time: `columns` is indexed by column, with holes. */
  const [summary, setSummary] = createSignal<TableSummary>();
  /** The summary with no filters on: every value a filter can pick from. */
  const [overall, setOverall] = createSignal<TableSummary>();
  const [error, setError] = createSignal<string>();
  const [picked, setPicked] = createSignal(0);
  const [view, setView] = createSignal({ top: 0, left: 0, width: 800, height: 400 });
  /** Column widths and the row labels' width, set from the first page of each table. */
  const [widths, setWidths] = createSignal<number[]>([]);
  const [labelWidth, setLabelWidth] = createSignal(48);
  /** Each category column's values' colors, kept as filters change. */
  const [colors, setColors] = createSignal<ReadonlyMap<number, ReadonlyMap<string, string>>>(new Map());
  const [editing, setEditing] = createSignal<{ index?: number; filter: TableFilter; at: { left: number; top: number }; anchor: HTMLElement }>();
  const [working, setWorking] = createSignal(false);
  const [sideShown, setSideShown] = createSignal(readSide());

  /** The session the table is in: its own, or the one that replaced it after a restart. */
  const sid = () => ws.current(tab.session);
  const session = () => ws.sessions().find((s) => s.id === sid());
  const ended = () => !session() || session()!.status === "ended";
  const busy = () => ws.isBusy(sid());
  const name = () => languageLabel(tab.language);
  const columns = () => head()?.columns ?? [];
  const rowCount = () => head()?.rows ?? 0;

  // What the tab and the status bar show.
  createEffect(() => {
    const h = head();
    if (!h) return;
    tab.setCode(h.code);
    tab.setCount(h.rows === h.total ? `${formatNumber(h.total)} rows` : `${formatNumber(h.rows)} of ${formatNumber(h.total)} rows`);
  });

  let generation = 0;
  /** Blocks of cells and of stats being read for this generation. */
  const reading = new Set<string>();
  const summing = new Set<number>();
  /** Set when the table should be read again once it can be: shown, idle. */
  let stale = true;
  const rowsAt = (page: number, block: number, cellsOnly = true) =>
    ws.call<TableRows>("table_rows", {
      session: sid(),
      path: tab.path,
      offset: page * PAGE,
      limit: PAGE,
      sort: sort(),
      filters: filters(),
      column_offset: block * BLOCK,
      column_limit: BLOCK,
      cells_only: cellsOnly,
    });
  const statsAt = (block: number) =>
    ws.call<TableSummary>("table_summary", { session: sid(), path: tab.path, filters: filters(), column_offset: block * BLOCK, column_limit: BLOCK });
  /** The summaries read, as one indexed by column. */
  const merged = (base: TableSummary | undefined, read: TableSummary[]): TableSummary | undefined => {
    if (!read.length) return base;
    const columns = base ? [...base.columns] : [];
    for (const r of read) for (const c of r.columns) columns[c.column] = c;
    return { ...read[0], columns };
  };

  /**
   * Reads the table again: the pages on screen, and with `stats` the column
   * summaries. With `top`, the grid goes back to the first row, as after a
   * new filter or sort.
   */
  async function reload(opts: { stats?: boolean; top?: boolean } = {}) {
    if (!props.shown || ended() || busy()) {
      stale = true;
      return;
    }
    stale = false;
    const mine = ++generation;
    reading.clear();
    if (opts.top && grid) {
      grid.scrollTop = 0;
      setView((v) => ({ ...v, top: 0 }));
    }
    // The table may have changed shape since: filters and sorting follow
    // their columns by name, and go with them.
    const before = untrack(head)?.columns ?? tab.named;
    if (!opts.top && before && (untrack(filters).length || untrack(sort).length)) {
      const bare = await ws
        .call<TableRows>("table_rows", { session: sid(), path: tab.path, offset: 0, limit: 0, sort: [], filters: [] })
        .catch(() => undefined);
      if (mine !== generation) return;
      if (bare) {
        follow(before, bare.columns);
        tab.named = undefined;
      }
    }
    const wanted = (opts.top ? [0] : untrack(visiblePages)).flatMap((page) => untrack(visibleBlocks).map((block) => [page, block] as const));
    const stats = opts.stats || !untrack(summary);
    const statBlocks = untrack(wantedStats);
    summing.clear();
    try {
      const [read, summed] = await Promise.all([
        Promise.all(wanted.map(([page, block], i) => rowsAt(page, block, i > 0))),
        stats ? Promise.all(statBlocks.map(statsAt)).catch((err) => new Error(message(err))) : undefined,
      ]);
      if (mine !== generation) return;
      batch(() => {
        const first = read[0];
        const sameColumns = sameNames(untrack(columns), first.columns);
        setHead(first);
        if (!sameColumns) tab.setColumns(first.columns);
        setPages(new Map(read.map((r, i) => [`${wanted[i][0]}:${wanted[i][1]}`, { labels: r.labels, values: r.values }])));
        const fresh = summed instanceof Error ? undefined : merged(undefined, summed ?? []);
        if (!sameColumns || !untrack(widths).length) {
          measured.clear();
          setWidths(first.columns.map((c) => columnWidth(c, [])));
          setLabelWidth(labelWidthFor(first));
          setColors(new Map());
          setOverall(undefined);
          if (untrack(picked) >= first.columns.length) setPicked(0);
        }
        for (const [i, r] of read.entries()) fitBlock(wanted[i][1], r, fresh);
        if (summed && !(summed instanceof Error)) statBlocks.forEach((b) => summing.add(b));
        if (summed instanceof Error) {
          if (!summed.message.includes(BUSY)) setSummary(undefined);
        } else if (fresh) {
          setSummary(fresh);
          if (!untrack(filters).length) setOverall(fresh);
          setColors((all) => colorValues(all, first.columns, summed!.flatMap((s) => s.columns)));
        }
        setError(undefined);
      });
    } catch (err) {
      if (mine !== generation) return;
      // Code started meanwhile: the table is read again when it finishes.
      if (message(err).includes(BUSY)) stale = true;
      else setError(message(err));
    }
  }

  function follow(before: { name: string }[], now: TableColumn[]) {
    if (before.length === now.length && before.every((c, i) => c?.name === now[i].name)) return;
    const at = (i: number) => now.findIndex((c) => c.name === before[i]?.name);
    batch(() => {
      setFilters((list) => list.map((f) => ({ ...f, column: at(f.column) })).filter((f) => f.column >= 0));
      setSort((list) => list.map((k) => ({ ...k, column: at(k.column) })).filter((k) => k.column >= 0));
      setPicked((i) => Math.max(0, at(i)));
    });
  }

  /** Blocks of columns whose widths were fitted to their cells, for this shape of table. */
  const measured = new Set<number>();
  /** Fits a block's columns to its first cells read, and to the smallest and largest numbers further down. */
  function fitBlock(block: number, read: TableRows, stats: TableSummary | undefined) {
    if (measured.has(block) || !read.labels.length) return;
    measured.add(block);
    const all = untrack(columns);
    setWidths((now) => {
      const next = [...now];
      for (let j = 0; j < (read.values[0]?.length ?? 0); j++) {
        const c = block * BLOCK + j;
        if (!all[c]) continue;
        const s = stats?.columns[c];
        const ends = [s?.min, s?.max].filter((n): n is number => n !== undefined).map(formatNumber);
        next[c] = columnWidth(all[c], [...read.values.map((row) => formatCell(row[j], tab.language)), ...ends]);
      }
      return next;
    });
  }

  /** Reads a block of cells scrolled to. */
  async function readPage(page: number, block: number) {
    const mine = generation;
    const key = `${page}:${block}`;
    if (reading.has(key) || busy()) return;
    reading.add(key);
    try {
      const read = await rowsAt(page, block);
      if (mine !== generation) return;
      batch(() => {
        fitBlock(block, read, untrack(summary));
        setPages((all) => {
          const next = new Map(all);
          next.set(key, { labels: read.labels, values: read.values });
          // The farthest blocks go.
          const far = (k: string) => {
            const [p, b] = k.split(":").map(Number);
            return Math.abs(p - page) + Math.abs(b - block);
          };
          const keys = [...next.keys()].sort((a, b) => far(b) - far(a));
          for (const k of keys.slice(0, Math.max(0, next.size - KEEP_PAGES))) next.delete(k);
          return next;
        });
      });
    } catch (err) {
      if (mine === generation && !message(err).includes(BUSY)) setError(message(err));
    } finally {
      if (mine === generation) reading.delete(key);
    }
  }

  /** Reads the stats of a block of columns come into view. */
  async function readStats(block: number) {
    const mine = generation;
    if (summing.has(block) || busy()) return;
    summing.add(block);
    try {
      const read = await statsAt(block);
      if (mine !== generation) return;
      batch(() => {
        setSummary((s) => merged(s, [read]));
        if (!untrack(filters).length) setOverall((s) => merged(s, [read]));
        setColors((all) => colorValues(all, untrack(columns), read.columns));
      });
    } catch {
      // Busy, or the table changed: they're read with the table next time.
      if (mine === generation) summing.delete(block);
    }
  }

  // First read, and again when shown after something changed.
  createEffect(() => {
    if (props.shown && !busy() && stale) untrack(() => void reload({ stats: true }));
  });
  // After each run in the session.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const unwatch = ws.runs.watchAll((run) => {
    if (run.session !== sid() || run.quiet || run.phase !== "done") return;
    stale = true;
    clearTimeout(timer);
    timer = setTimeout(() => void reload({ stats: true }), REFRESH_MS);
  });
  // After a reconnect.
  createEffect(
    on(
      () => ws.connectedSince(),
      (since) => {
        if (since !== undefined) void reload({ stats: true });
      },
      { defer: true },
    ),
  );
  // After a restart, from the session that replaced its own.
  createEffect(
    on(
      sid,
      () => {
        stale = true;
        void reload({ stats: true });
      },
      { defer: true },
    ),
  );
  // Cells and stats scrolled to, once scrolling rests.
  let pageTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    if (!head()) return;
    const missing = visiblePages().flatMap((p) => visibleBlocks().filter((b) => !pages().has(`${p}:${b}`)).map((b) => [p, b] as const));
    const stats = summary();
    const unsummed = stats ? wantedStats().filter((b) => !summing.has(b) && !hasStats(stats, b)) : [];
    if (!missing.length && !unsummed.length) return;
    clearTimeout(pageTimer);
    pageTimer = setTimeout(() => {
      missing.forEach(([p, b]) => void readPage(p, b));
      unsummed.forEach((b) => void readStats(b));
    }, WAIT_MS);
  });
  onCleanup(() => {
    unwatch();
    clearTimeout(timer);
    clearTimeout(pageTimer);
  });

  const changeSort = (next: TableSort[]) => {
    setSort(next);
    void reload({ top: true });
  };
  const changeFilters = (next: TableFilter[]) => {
    setFilters(next);
    void reload({ top: true, stats: true });
  };

  // The grid: only the rows and columns on screen are drawn.
  let grid: HTMLDivElement | undefined;
  const fullHeight = () => rowCount() * ROW;
  const rowsHeight = () => Math.min(fullHeight(), MAX_HEIGHT);
  /** The height rows show in, under the header. */
  const body = () => Math.max(ROW, view().height - HEAD);
  /** The row at the top of the grid, and how far into it. */
  const topRow = () => {
    if (fullHeight() <= rowsHeight()) return view().top / ROW;
    const room = Math.max(1, rowsHeight() - body());
    return ((Math.min(view().top, room) / room) * Math.max(0, fullHeight() - body())) / ROW;
  };
  const firstRow = () => Math.max(0, Math.floor(topRow()) - OVERSCAN);
  const lastRow = () => Math.min(rowCount(), Math.ceil(topRow() + body() / ROW) + OVERSCAN);
  /** Where the first row drawn goes, so the top row sits under the header. */
  const rowsTop = () => Math.max(0, view().top - (topRow() - firstRow()) * ROW);
  function visiblePages(): number[] {
    if (!rowCount()) return [0];
    const from = Math.floor(firstRow() / PAGE);
    const to = Math.floor(Math.max(firstRow(), lastRow() - 1) / PAGE);
    return Array.from({ length: to - from + 1 }, (_, i) => from + i);
  }
  const rowIndexes = createMemo(() => Array.from({ length: Math.max(0, lastRow() - firstRow()) }, (_, i) => firstRow() + i), [], {
    equals: (a, b) => a.length === b.length && a[0] === b[0],
  });
  const offsets = createMemo(() => {
    const out = [0];
    for (const w of widths()) out.push(out[out.length - 1] + w);
    return out;
  });
  const columnIndexes = createMemo(
    () => {
      const at = offsets();
      const n = widths().length;
      const left = view().left;
      const right = left + view().width - labelWidth();
      let from = 0;
      while (from < n - 1 && at[from + 1] < left) from++;
      let to = from;
      while (to < n - 1 && at[to + 1] < right) to++;
      from = Math.max(0, from - 2);
      to = Math.min(n - 1, to + 2);
      return n ? Array.from({ length: to - from + 1 }, (_, i) => from + i) : [];
    },
    [],
    { equals: (a, b) => a.length === b.length && a[0] === b[0] },
  );
  const leftPad = () => offsets()[columnIndexes()[0] ?? 0] ?? 0;
  const rightPad = () => {
    const at = offsets();
    const shown = columnIndexes();
    return (at[at.length - 1] ?? 0) - (at[(shown[shown.length - 1] ?? -1) + 1] ?? 0);
  };
  const visibleBlocks = createMemo(
    () => {
      const shown = columnIndexes();
      if (!shown.length) return [0];
      const from = Math.floor(shown[0] / BLOCK);
      const to = Math.floor(shown[shown.length - 1] / BLOCK);
      return Array.from({ length: to - from + 1 }, (_, i) => from + i);
    },
    [0],
    { equals: (a, b) => a.length === b.length && a[0] === b[0] },
  );
  /** The row's label, once any of its cells are read. */
  const rowAt = (i: number) => {
    const at = i % PAGE;
    for (const block of visibleBlocks()) {
      const page = pages().get(`${Math.floor(i / PAGE)}:${block}`);
      if (page && at < page.labels.length) return { label: page.labels[at] };
    }
    return undefined;
  };
  /** A cell, or undefined while its block is read. */
  const cellAt = (i: number, c: number) => pages().get(`${Math.floor(i / PAGE)}:${Math.floor(c / BLOCK)}`)?.values[i % PAGE]?.[c % BLOCK];
  const hasCell = (i: number, c: number) => (pages().get(`${Math.floor(i / PAGE)}:${Math.floor(c / BLOCK)}`)?.labels.length ?? 0) > i % PAGE;

  // The column list at the side: only the names in view are drawn.
  let side: HTMLDivElement | undefined;
  const [sideView, setSideView] = createSignal({ top: 0, height: 600 });
  const measureSide = () => {
    if (side) setSideView((v) => ({ top: side!.scrollTop, height: side!.clientHeight || v.height }));
  };
  const sideObserver = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(measureSide);
  onCleanup(() => sideObserver?.disconnect());
  const sideIndexes = createMemo(
    () => {
      const from = Math.max(0, Math.floor(sideView().top / SIDE_ROW) - OVERSCAN);
      const to = Math.min(columns().length, Math.ceil((sideView().top + sideView().height) / SIDE_ROW) + OVERSCAN);
      return Array.from({ length: Math.max(0, to - from) }, (_, i) => from + i);
    },
    [],
    { equals: (a, b) => a.length === b.length && a[0] === b[0] },
  );
  /** The blocks of columns whose stats show: in the list, in the grid, picked or filtered. */
  const wantedStats = createMemo(
    () => {
      const shown = sideShown() ? sideIndexes() : [];
      const blocks = new Set([...shown, picked(), ...filters().map((f) => f.column)].map((c) => Math.floor(c / BLOCK)));
      for (const b of visibleBlocks()) blocks.add(b);
      return [...blocks].sort((a, b) => a - b);
    },
    [0],
    { equals: (a, b) => a.length === b.length && a.every((x, i) => x === b[i]) },
  );
  const measure = () => {
    const el = grid;
    if (!el) return;
    // A hidden tab measures 0 across; it keeps its size for when it's back.
    setView((v) => ({ top: el.scrollTop, left: el.scrollLeft, width: el.clientWidth || v.width, height: el.clientHeight || v.height }));
  };
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(measure);
  onCleanup(() => observer?.disconnect());
  const showColumn = (i: number) => {
    setPicked(i);
    if (side) {
      const top = i * SIDE_ROW;
      if (top < side.scrollTop) side.scrollTop = top;
      else if (top + SIDE_ROW > side.scrollTop + side.clientHeight) side.scrollTop = top + SIDE_ROW - side.clientHeight;
    }
    if (!grid) return;
    const at = offsets();
    const start = at[i] ?? 0;
    const end = at[i + 1] ?? start;
    const room = grid.clientWidth - labelWidth();
    if (start < grid.scrollLeft) grid.scrollLeft = start;
    else if (end > grid.scrollLeft + room) grid.scrollLeft = end - room;
  };

  async function exportCsv() {
    const h = head();
    if (!h) return;
    const folderKey = `sandhill.exportFolder:${props.project}`;
    const folder = stored(folderKey) ?? "";
    const typed = (
      await askText(
        "Export the table as",
        `${folder ? `${folder}/` : ""}${exportName(tab.path)}`,
        "Export",
        `A CSV of the ${formatNumber(h.rows)} rows shown, filtered and sorted as they are, in the project's folder on ${props.hostLabel}.`,
      )
    )?.trim();
    if (!typed) return;
    const file = documentPath(props.project, typed);
    const shownName = props.shortName(file);
    setWorking(true);
    try {
      const [stat] = await ws.call<FileStat[]>("stat_files", { paths: [file] });
      if (stat?.mtime_ms !== undefined) {
        const choice = await ask(`Replace ${shownName}?`, ["Replace", "Cancel"], "A file with this name is already there.");
        if (choice !== "Replace") return;
      }
      await ws.call("table_export", { session: sid(), path: tab.path, sort: sort(), filters: filters(), file });
      const slash = typed.lastIndexOf("/");
      if (!typed.startsWith("/")) store(folderKey, slash > 0 ? typed.slice(0, slash) : "");
      notify(`Exported ${formatNumber(h.rows)} rows to ${shownName}.`, "info");
    } catch (err) {
      notify(message(err).includes(BUSY) ? `${name()} is running code; export once it finishes.` : `Could not export the table: ${message(err)}`);
    } finally {
      setWorking(false);
    }
  }

  async function copyCode() {
    const h = head();
    if (!h) return;
    try {
      await navigator.clipboard.writeText(tableCode(tab.language, h, sort(), filters()));
      notify(`Copied ${name()} code for this view.`, "info");
    } catch (err) {
      notify(`Could not copy the code: ${message(err)}`);
    }
  }

  const openFilter = (anchor: HTMLElement, index?: number) => {
    const box = anchor.getBoundingClientRect();
    const filter = index === undefined ? newFilter(picked(), columns(), summary()?.columns[picked()]) : filters()[index];
    setEditing({ index, filter, anchor, at: { left: Math.max(8, Math.min(box.left, window.innerWidth - 300)), top: box.bottom + 4 } });
  };
  const applyFilter = (filter: TableFilter) => {
    const edit = editing();
    setEditing(undefined);
    if (!edit) return;
    const list = filters();
    changeFilters(edit.index === undefined ? [...list, filter] : list.map((f, i) => (i === edit.index ? filter : f)));
  };
  /** A value clicked in the stats becomes a filter, or joins the column's "is one of". */
  const filterTo = (column: number, value: string) => {
    const list = filters();
    const at = list.findIndex((f) => f.column === column && f.op === "in");
    if (at < 0) return changeFilters([...list, { column, op: "in", values: [value] }]);
    const values = list[at].values ?? [];
    if (values.includes(value)) return;
    changeFilters(list.map((f, i) => (i === at ? { ...f, values: [...values, value] } : f)));
  };

  const showSide = (shown: boolean) => {
    setSideShown(shown);
    try {
      localStorage.setItem(SIDE_KEY, shown ? "shown" : "hidden");
    } catch {
      // Only a convenience; the next table shows the column stats again.
    }
  };

  const stateText = () => (ended() ? "session ended" : busy() ? "running code…" : "live");

  return (
    <div class="de" classList={{ busy: busy(), noside: !sideShown() }}>
      <div class="kernel-bar de-top">
        <button
          class="bar-button de-sidetoggle"
          aria-pressed={sideShown()}
          title={`${sideShown() ? "Hide" : "Show"} the column list and its stats, so the table gets the whole width`}
          onClick={() => showSide(!sideShown())}
        >
          <Icon name="sidebar.left" small />
          {sideShown() ? "Hide column stats" : "Show column stats"}
        </button>
        <span class="kernel de-kern" data-state={ended() ? "off" : busy() ? "busy" : "idle"} title={`${name()} ${session() ? languageVersion(session()!) : ""} · ${stateText()}`}>
          <LangIcon language={tab.language} />
          <span class="de-what">
            {[session() ? languageVersion(session()!) : "", tab.code()].filter(Boolean).join(" · ")}, {stateText()}
          </span>
        </span>
        <span class="spacer" />
        <button class="bar-button" title="Read the table again" disabled={ended() || busy()} onClick={() => void reload({ stats: true })}>
          <Icon name="refresh" small />
        </button>
        <button class="bar-button" title={`Copy ${name()} code that filters and sorts the table as shown`} disabled={!head()} onClick={() => void copyCode()}>
          <Icon name="copy" small />
          Copy as code
        </button>
        <button class="bar-button" title="Save the rows shown as a CSV file" disabled={!head() || working() || ended()} onClick={() => void exportCsv()}>
          <Icon name="save" small />
          Export CSV
        </button>
      </div>
      <div class="de-body">
        <div class="de-side" aria-label="Columns">
          <div
            class="de-cols"
            ref={(el) => {
              side = el;
              sideObserver?.observe(el);
              queueMicrotask(measureSide);
            }}
            onScroll={measureSide}
          >
            <div class="de-cols-in" style={{ height: `${columns().length * SIDE_ROW}px`, "padding-top": `${(sideIndexes()[0] ?? 0) * SIDE_ROW}px` }}>
              <For each={sideIndexes()}>
                {(i) => {
                  const column = () => columns()[i];
                  return (
                    <Show when={column()}>
                      {(c) => (
                        <button class="de-col" classList={{ on: picked() === i }} title={`${c().name} (${c().type})`} onClick={() => showColumn(i)}>
                          <span>
                            <b>{c().name}</b>
                            <small>{columnNote(c(), summary()?.columns[i])}</small>
                          </span>
                          <Spark column={c()} summary={summary()?.columns[i]} colors={colors().get(i)} />
                        </button>
                      )}
                    </Show>
                  );
                }}
              </For>
            </div>
          </div>
          <Show when={columns()[picked()]}>
            {(column) => <Stats column={column()} summary={summary()?.columns[picked()]} colors={colors().get(picked())} language={tab.language} onValue={(v) => filterTo(picked(), v)} />}
          </Show>
        </div>
        <div class="de-main">
          <div class="de-bar">
            <For each={filters()}>
              {(filter, i) => (
                <span class="chip" title="Change this filter">
                  <button class="chip-text" onClick={(e) => openFilter(e.currentTarget.parentElement as HTMLElement, i())}>
                    {filterText(filter, columns())}
                  </button>
                  <button class="chip-x" title="Remove this filter" onClick={() => changeFilters(filters().filter((_, j) => j !== i()))}>
                    <Icon name="x" small />
                  </button>
                </span>
              )}
            </For>
            <button class="chip ghost" disabled={!columns().length} onClick={(e) => openFilter(e.currentTarget)}>
              <Icon name="filter" small />
              Add filter
            </button>
            <span class="spacer" />
            <Show when={head()}>
              {(h) => (
                <span class="count">
                  {h().rows === h().total ? `${formatNumber(h().total)} rows` : `${formatNumber(h().rows)} of ${formatNumber(h().total)} rows`} ·{" "}
                  {`${formatNumber(h().width)} ${h().width === 1 ? "column" : "columns"}`}
                </span>
              )}
            </Show>
          </div>
          <Show when={busy() || ended() || error()}>
            <div class="de-note" classList={{ err: !!error() && !busy() }}>
              {ended()
                ? `The ${name()} session this table was in has ended.`
                : busy()
                  ? `${name()} is running code; the table updates when it finishes.`
                  : error()}
            </div>
          </Show>
          <div
            class="de-grid"
            ref={(el) => {
              grid = el;
              observer?.observe(el);
              queueMicrotask(measure);
            }}
            onScroll={measure}
          >
            <Show when={head()} fallback={<p class="vempty">{error() || busy() || ended() ? "" : "Reading the table…"}</p>}>
              <div class="de-canvas" style={{ width: `${labelWidth() + (offsets()[offsets().length - 1] ?? 0)}px` }}>
                <div class="de-tr de-th-row">
                  <div class="de-rn de-corner" style={{ width: `${labelWidth()}px` }} />
                  <div class="de-pad" style={{ width: `${leftPad()}px` }} />
                  <For each={columnIndexes()}>
                    {(c) => {
                      const column = () => columns()[c];
                      const key = () => sort().findIndex((s) => s.column === c);
                      return (
                        <div
                          class="de-th"
                          classList={{ num: column()?.kind === "number", on: picked() === c }}
                          style={{ width: `${widths()[c]}px` }}
                          role="columnheader"
                          aria-sort={key() < 0 ? "none" : sort()[key()].descending ? "descending" : "ascending"}
                          title={`Sort by ${column()?.name} (shift-click to sort by it next)`}
                          onClick={(e) => {
                            setPicked(c);
                            changeSort(nextSort(sort(), c, e.shiftKey));
                          }}
                        >
                          <span class="de-name">
                            <span>{column()?.name}</span>
                            <Show when={key() >= 0}>
                              <span class="srt">
                                {sort()[key()].descending ? "▼" : "▲"}
                                {sort().length > 1 ? key() + 1 : ""}
                              </span>
                            </Show>
                          </span>
                          <small>{column()?.type}</small>
                        </div>
                      );
                    }}
                  </For>
                  <div class="de-pad" style={{ width: `${rightPad()}px` }} />
                </div>
                <div class="de-rows" style={{ height: `${rowsHeight()}px`, "padding-top": `${rowsTop()}px` }}>
                  <For each={rowIndexes()}>
                    {(i) => {
                      const row = () => rowAt(i);
                      return (
                        <div class="de-tr" classList={{ wait: !row() }}>
                          <div class="de-rn" style={{ width: `${labelWidth()}px` }} title={row()?.label}>
                            {row()?.label ?? ""}
                          </div>
                          <div class="de-pad" style={{ width: `${leftPad()}px` }} />
                          <For each={columnIndexes()}>
                            {(c) => (
                              <Cell
                                value={cellAt(i, c)}
                                column={columns()[c]}
                                width={widths()[c]}
                                language={tab.language}
                                color={colors().get(c)}
                                on={picked() === c}
                                waiting={!hasCell(i, c)}
                                onClick={() => setPicked(c)}
                              />
                            )}
                          </For>
                          <div class="de-pad" style={{ width: `${rightPad()}px` }} />
                        </div>
                      );
                    }}
                  </For>
                </div>
              </div>
            </Show>
          </div>
          <div class="de-foot">
            <span>{sortText(sort(), columns())}</span>
            <span>Sorting, filters and stats run in the {name()} session; only the rows on screen are read.</span>
          </div>
        </div>
      </div>
      <Show when={editing()}>
        {(edit) => (
          <FilterPopup
            columns={columns()}
            summaries={(overall() ?? summary())?.columns}
            start={edit().filter}
            at={edit().at}
            anchor={edit().anchor}
            editing={edit().index !== undefined}
            onApply={applyFilter}
            onClose={() => setEditing(undefined)}
          />
        )}
      </Show>
    </div>
  );
}

function Cell(props: {
  value: number | string | null | undefined;
  column: TableColumn | undefined;
  width: number;
  language: string;
  color?: ReadonlyMap<string, string>;
  on: boolean;
  waiting: boolean;
  onClick: () => void;
}) {
  const missing = () => !props.waiting && (props.value === null || props.value === undefined);
  const text = () => (props.waiting ? "" : formatCell(props.value, props.language));
  const dot = () => (props.value !== null && props.value !== undefined ? props.color?.get(String(props.value)) : undefined);
  return (
    <div
      class="de-td"
      classList={{ num: props.column?.kind === "number", na: missing(), on: props.on }}
      style={{ width: `${props.width}px` }}
      title={text().length > 24 ? text() : undefined}
      onClick={() => props.onClick()}
    >
      <Show when={dot()}>
        <i class="ct" style={{ background: dot() }} />
      </Show>
      {text()}
    </div>
  );
}

/** A column's sparkline: a histogram of numbers, or bars for its commonest values. */
function Spark(props: { column: TableColumn; summary?: ColumnSummary; colors?: ReadonlyMap<string, string> }) {
  const bars = (): { height: number; color?: string }[] => {
    const s = props.summary;
    if (!s) return [];
    const counts: { count: number; color?: string }[] = s.histogram?.length
      ? s.histogram.map((count) => ({ count }))
      : (s.top ?? []).map((t) => ({ count: t.count, color: props.colors?.get(t.value) }));
    const top = Math.max(1, ...counts.map((c) => c.count));
    return counts.map((c) => ({ height: c.count ? Math.max(1, (c.count / top) * 20) : 0, color: c.color }));
  };
  return (
    <svg class="de-spark" viewBox="0 0 70 22" preserveAspectRatio="none" aria-hidden="true">
      <For each={bars()}>
        {(bar, i) => {
          // One value fills the width; a few get wide bars.
          const step = () => 70 / Math.max(bars().length, 1);
          return <rect x={i() * step()} y={22 - bar.height} width={Math.max(1, step() - 1)} height={bar.height} fill={bar.color ?? "var(--accent)"} fill-opacity={bar.color ? 1 : 0.7} />;
        }}
      </For>
    </svg>
  );
}

/** The picked column's numbers: its range and middle, or its commonest values. */
function Stats(props: { column: TableColumn; summary?: ColumnSummary; colors?: ReadonlyMap<string, string>; language: string; onValue: (value: string) => void }) {
  const s = () => props.summary;
  const number = (n: number | undefined) => (n === undefined ? "" : formatNumber(n));
  return (
    <dl class="de-stats">
      <h4>{props.column.name}</h4>
      <Show when={s()} fallback={<dd class="de-wait">Reading…</dd>}>
        <Show when={props.column.kind === "number" && s()!.min !== undefined}>
          <dt>Min</dt>
          <dd>{number(s()!.min)}</dd>
          <dt>Median</dt>
          <dd>{number(s()!.median)}</dd>
          <dt>Mean</dt>
          <dd>{number(s()!.mean)}</dd>
          <dt>Max</dt>
          <dd>{number(s()!.max)}</dd>
        </Show>
        <For each={s()!.top ?? []}>
          {(t) => (
            <>
              <dt>
                <button class="de-val" title={`Show only rows where ${props.column.name} is ${t.value}`} onClick={() => props.onValue(t.value)}>
                  <Show when={props.colors?.get(t.value)}>
                    <i class="ct" style={{ background: props.colors!.get(t.value) }} />
                  </Show>
                  {t.value}
                </button>
              </dt>
              <dd>{formatNumber(t.count)}</dd>
            </>
          )}
        </For>
        <Show when={s()!.top?.length && s()!.distinct !== undefined && s()!.distinct! > s()!.top!.length}>
          <dt class="de-more">… {formatNumber(s()!.distinct! - s()!.top!.length)} more values</dt>
          <dd />
        </Show>
        <Show when={props.column.kind !== "number" && s()!.distinct !== undefined}>
          <dt>Distinct</dt>
          <dd>{formatNumber(s()!.distinct!)}</dd>
        </Show>
        <dt>Missing ({missingText(props.language)})</dt>
        <dd>{formatNumber(s()!.missing)}</dd>
      </Show>
    </dl>
  );
}

/** Adding or changing a filter: a column, a condition, and its values. 11px, like every popup. */
function FilterPopup(props: {
  columns: TableColumn[];
  summaries?: ColumnSummary[];
  start: TableFilter;
  at: { left: number; top: number };
  anchor: HTMLElement;
  editing: boolean;
  onApply: (filter: TableFilter) => void;
  onClose: () => void;
}) {
  const [column, setColumn] = createSignal(props.start.column);
  const [op, setOp] = createSignal(props.start.op);
  const [values, setValues] = createSignal<string[]>(props.start.values ?? []);
  /** Values typed as a list, "a, b, c". */
  const [typed, setTyped] = createSignal((props.start.values ?? []).join(", "));
  const [tried, setTried] = createSignal(false);
  const kind = () => props.columns[column()]?.kind ?? "other";
  const ops = () => opsFor(kind());
  const current = () => ops().find((o) => o.op === op()) ?? ops()[0];
  /** A category's values to tick, when all of them are known. */
  const choices = () => {
    const s = props.summaries?.[column()];
    return s?.top?.length && s.distinct !== undefined && s.distinct <= s.top.length ? s.top.map((t) => t.value) : undefined;
  };
  const filter = (): TableFilter => {
    const wanted = current().values;
    const list =
      wanted === "many"
        ? (choices() ? values() : typed().split(",")).map((v) => v.trim()).filter(Boolean)
        : values()
            .slice(0, wanted)
            .map((v) => v.trim());
    return { column: column(), op: current().op, values: list };
  };
  const problem = () => filterError(filter(), props.columns[column()]);
  const apply = () => {
    setTried(true);
    if (!problem()) props.onApply(filter());
  };
  const pickColumn = (c: number) => {
    setColumn(c);
    if (!opsFor(kind()).some((o) => o.op === op())) setOp(opsFor(kind())[0].op);
    // Ticked values belong to their column; a typed number can stay.
    if (current().values === "many") setValues([]);
  };
  const setValue = (i: number, v: string) => setValues((list) => Object.assign([...list], { [i]: v }));
  const toggle = (v: string) => setValues((list) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]));
  let popup!: HTMLFormElement;
  closeOnOutside(() => popup, props.anchor, props.onClose);
  return (
    <Portal>
      <form
        ref={popup}
        class="kpop de-filter"
        style={{ left: `${props.at.left}px`, top: `${props.at.top}px` }}
        onSubmit={(e) => {
          e.preventDefault();
          apply();
        }}
      >
        <label class="df-row">
          <span>Column</span>
          <select value={column()} onChange={(e) => pickColumn(Number(e.currentTarget.value))}>
            <For each={props.columns}>
              {(c, i) => (
                <option value={i()} selected={i() === column()}>
                  {c.name}
                </option>
              )}
            </For>
          </select>
        </label>
        <label class="df-row">
          <span>Condition</span>
          <select value={current().op} onChange={(e) => setOp(e.currentTarget.value)}>
            <For each={ops()}>
              {(o) => (
                <option value={o.op} selected={o.op === current().op}>
                  {o.label}
                </option>
              )}
            </For>
          </select>
        </label>
        <Show when={current().values === 1 || current().values === 2}>
          <div class="df-row">
            <span>{current().values === 2 ? "From" : "Value"}</span>
            <span class="df-values">
              <input aria-label="Value" ref={(el) => queueMicrotask(() => el.focus())} value={values()[0] ?? ""} onInput={(e) => setValue(0, e.currentTarget.value)} />
              <Show when={current().values === 2}>
                <span>and</span>
                <input aria-label="Second value" value={values()[1] ?? ""} onInput={(e) => setValue(1, e.currentTarget.value)} />
              </Show>
            </span>
          </div>
        </Show>
        <Show when={current().values === "many"}>
          <Show
            when={choices()}
            fallback={
              <label class="df-row">
                <span>Values</span>
                <input
                  aria-label="Values, separated by commas"
                  placeholder="a, b, c"
                  ref={(el) => queueMicrotask(() => el.focus())}
                  value={typed()}
                  onInput={(e) => setTyped(e.currentTarget.value)}
                />
              </label>
            }
          >
            {(list) => (
              <div class="df-choices">
                <For each={list()}>
                  {(v) => (
                    <label>
                      <input type="checkbox" checked={values().includes(v)} onChange={() => toggle(v)} />
                      <span>{v}</span>
                    </label>
                  )}
                </For>
              </div>
            )}
          </Show>
        </Show>
        <Show when={tried() && problem()}>
          <div class="df-err">{problem()}</div>
        </Show>
        <div class="df-acts">
          <button type="button" class="pz-btn" onClick={props.onClose}>
            Cancel
          </button>
          <button type="submit" class="pz-btn pri">
            {props.editing ? "Change filter" : "Add filter"}
          </button>
        </div>
      </form>
    </Portal>
  );
}

/** A new filter on `column`: the usual condition for its kind. */
function newFilter(column: number, columns: TableColumn[], summary?: ColumnSummary): TableFilter {
  const kind = columns[column]?.kind ?? "other";
  const op = opsFor(kind)[0].op;
  const values = kind === "number" && summary?.median !== undefined && op === "<" ? [String(summary.median)] : [];
  return { column, op, values };
}

/** "dbl", "fct · 9 values", "chr · 2 missing". */
function columnNote(column: TableColumn, summary?: ColumnSummary): string {
  const parts = [column.type];
  if (summary?.distinct !== undefined && column.kind !== "number") parts.push(summary.distinct === 1 ? "1 unique" : `${formatNumber(summary.distinct)} values`);
  if (summary?.missing) parts.push(`${formatNumber(summary.missing)} missing`);
  return parts.join(" · ");
}

/** Colors for the values of category columns with at most 10 of them, keeping those given before. */
function colorValues(
  before: ReadonlyMap<number, ReadonlyMap<string, string>>,
  columns: TableColumn[],
  summaries: ColumnSummary[],
): ReadonlyMap<number, ReadonlyMap<string, string>> {
  const next = new Map(before);
  for (const s of summaries) {
    const column = columns[s.column];
    if (!column || (column.kind !== "category" && column.kind !== "logical") || !s.top?.length) continue;
    if (s.distinct === undefined || s.distinct > COLORS.length) continue;
    const map = new Map(next.get(s.column) ?? []);
    for (const t of s.top) if (!map.has(t.value) && map.size < COLORS.length) map.set(t.value, COLORS[map.size]);
    next.set(s.column, map);
  }
  return next;
}

/** Wide enough for the longest row label read, or the row count. */
function labelWidthFor(read: TableRows): number {
  const longest = Math.max(2, String(read.total).length, ...read.labels.map((l) => l.length));
  return Math.round(Math.max(40, Math.min(220, longest * 7.8 + 22)));
}

/** Whether a block of columns' stats were read. */
function hasStats(summary: TableSummary, block: number): boolean {
  return summary.columns[block * BLOCK] !== undefined;
}

function sameNames(a: TableColumn[], b: TableColumn[]): boolean {
  return a.length === b.length && a.every((c, i) => c.name === b[i].name && c.kind === b[i].kind);
}

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private mode; the folder just isn't remembered.
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

const SIDE_KEY = "sandhill.deColumnStats";

/** Whether the column list and stats are shown, as last chosen on this Mac. */
function readSide(): boolean {
  try {
    return localStorage.getItem(SIDE_KEY) !== "hidden";
  } catch {
    return true;
  }
}
