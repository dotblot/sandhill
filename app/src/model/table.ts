// The Data Explorer's model: how cells and filters read, which filters a
// column takes, and the R or Python code that does what the view does.
// Filtering and sorting themselves run in the kernel (`table_rows`).

import type { TableColumn, TableFilter, TableRows, TableSort } from "../backend/types";

/** Rows read at a time. */
export const PAGE = 200;

export interface Op {
  op: string;
  label: string;
  /** How many values it takes: 0, 1, 2, or "many" (a list). */
  values: 0 | 1 | 2 | "many";
}

const LESS: Op[] = [
  { op: "<", label: "<", values: 1 },
  { op: "<=", label: "≤", values: 1 },
  { op: ">", label: ">", values: 1 },
  { op: ">=", label: "≥", values: 1 },
];
const EQUAL: Op[] = [
  { op: "=", label: "is", values: 1 },
  { op: "!=", label: "is not", values: 1 },
];
const SET: Op[] = [
  { op: "in", label: "is one of", values: "many" },
  { op: "not in", label: "is none of", values: "many" },
];
const MISSING: Op[] = [
  { op: "missing", label: "is missing", values: 0 },
  { op: "present", label: "is not missing", values: 0 },
];

/** The filters a column of `kind` takes, the usual one first. */
export function opsFor(kind: string): Op[] {
  switch (kind) {
    case "number":
      return [...LESS, { op: "between", label: "between", values: 2 }, ...EQUAL, ...MISSING];
    case "date":
      return [...LESS, { op: "between", label: "between", values: 2 }, ...MISSING];
    case "category":
    case "logical":
      return [...SET, ...MISSING];
    case "text":
      return [{ op: "contains", label: "contains", values: 1 }, ...EQUAL, ...SET, ...MISSING];
    default:
      return [{ op: "contains", label: "contains", values: 1 }, ...MISSING];
  }
}

export function opLabel(op: string): string {
  for (const kind of ["number", "text", "category"]) {
    const found = opsFor(kind).find((o) => o.op === op);
    if (found) return found.label;
  }
  return op;
}

/** A filter as it reads on its chip: "percent.mt < 5", "cell_type is one of B, NK". */
export function filterText(filter: TableFilter, columns: TableColumn[]): string {
  const name = columns[filter.column]?.name ?? `column ${filter.column + 1}`;
  const values = filter.values ?? [];
  switch (filter.op) {
    case "missing":
    case "present":
      return `${name} ${opLabel(filter.op)}`;
    case "between":
      return `${name} between ${values[0]} and ${values[1]}`;
    case "in":
    case "not in":
      return `${name} ${opLabel(filter.op)} ${values.join(", ")}`;
    case "contains":
      return `${name} contains "${values[0]}"`;
    default:
      return `${name} ${opLabel(filter.op)} ${values[0]}`;
  }
}

/** Whether a filter's values make sense for its column: numbers where numbers go. */
export function filterError(filter: TableFilter, column: TableColumn | undefined): string | undefined {
  const op = opsFor(column?.kind ?? "other").find((o) => o.op === filter.op);
  if (!op) return "Pick a condition.";
  const values = filter.values ?? [];
  const wanted = op.values === "many" ? Math.max(1, values.length) : op.values;
  if (values.length < wanted || values.slice(0, wanted).some((v) => !v.trim())) return "Give a value.";
  if (column?.kind === "number" && filter.op !== "in" && filter.op !== "not in") {
    if (values.some((v) => !Number.isFinite(Number(v)))) return "Give a number.";
  }
  return undefined;
}

/** A cell as shown: numbers with digit grouping and at most 4 decimals, "NA" where missing. */
export function formatCell(value: number | string | null | undefined, language = "r"): string {
  if (value === null || value === undefined) return missingText(language);
  if (typeof value === "number") return formatNumber(value);
  return value;
}

export function missingText(language: string): string {
  return language.toLowerCase() === "r" ? "NA" : "NaN";
}

export function formatNumber(n: number): string {
  if (Number.isInteger(n) && Math.abs(n) < 1e15) return n.toLocaleString("en-US");
  const size = Math.abs(n);
  if (size !== 0 && (size >= 1e9 || size < 1e-4)) return n.toExponential(3);
  return n.toLocaleString("en-US", { maximumFractionDigits: 4 });
}

/** "Sorted by nCount_RNA, descending, then cluster". */
export function sortText(sort: TableSort[], columns: TableColumn[]): string {
  if (!sort.length) return "Not sorted; click a column's name to sort by it";
  return `Sorted by ${sort
    .map((s, i) => `${columns[s.column]?.name ?? s.column}${s.descending ? ", descending" : i === 0 && sort.length === 1 ? ", ascending" : ""}`)
    .join(", then ")}`;
}

/**
 * What clicking a column's name does to the sort: ascending, then
 * descending, then off. With `add` (shift-click) it becomes another key.
 */
export function nextSort(sort: TableSort[], column: number, add: boolean): TableSort[] {
  const at = sort.findIndex((s) => s.column === column);
  const current = sort[at];
  const next: TableSort | undefined = !current ? { column, descending: false } : !current.descending ? { column, descending: true } : undefined;
  if (!add) return next ? [next] : [];
  const rest = sort.filter((s) => s.column !== column);
  if (!next) return rest;
  return at >= 0 ? sort.map((s) => (s.column === column ? next : s)) : [...rest, next];
}

/** A width for each column, from its name and the cells read so far, in px. */
export function columnWidth(column: TableColumn, cells: string[]): number {
  const longest = Math.max(column.name.length + 2, column.type.length, ...cells.map((c) => c.length));
  return Math.round(Math.max(64, Math.min(320, longest * 7.2 + 22)));
}

/**
 * Code that does what the view does, in the session's language: the
 * filters, then the sort, on the table `table.code` names.
 */
export function tableCode(language: string, table: Pick<TableRows, "code" | "matrix" | "columns">, sort: TableSort[], filters: TableFilter[]): string {
  return language.toLowerCase() === "r" ? rCode(table, sort, filters) : pythonCode(table, sort, filters);
}

function rCode({ code, matrix, columns }: Pick<TableRows, "code" | "matrix" | "columns">, sort: TableSort[], filters: TableFilter[]): string {
  const lines = [`x <- ${code}`];
  const col = (i: number) => {
    const name = columns[i]?.name ?? "";
    if (matrix) return /^\[,\d+\]$/.test(name) ? `x[, ${i + 1}]` : `x[, ${rString(name)}]`;
    return /^((([A-Za-z]|[.][._A-Za-z])[._A-Za-z0-9]*)|[.])$/.test(name) ? `x$${name}` : `x[[${rString(name)}]]`;
  };
  const value = (i: number, v: string) => (columns[i]?.kind === "number" ? v : rString(v));
  const tests = filters.map((f) => {
    const values = f.values ?? [];
    const c = col(f.column);
    switch (f.op) {
      case "missing":
        return `is.na(${c})`;
      case "present":
        return `!is.na(${c})`;
      case "between":
        return `${c} >= ${value(f.column, values[0])} & ${c} <= ${value(f.column, values[1])}`;
      case "in":
        return `${c} %in% c(${values.map(rString).join(", ")})`;
      case "not in":
        return `!(${c} %in% c(${values.map(rString).join(", ")})) & !is.na(${c})`;
      case "contains":
        return `grepl(${rString(values[0].toLowerCase())}, tolower(${c}), fixed = TRUE)`;
      default:
        return `${c} ${f.op === "=" ? "==" : f.op} ${value(f.column, values[0])}`;
    }
  });
  if (tests.length) lines.push(`x <- x[which(${tests.join(" & ")}), , drop = FALSE]`);
  if (sort.length) {
    const keys = sort.map((s) => (s.descending ? `-xtfrm(${col(s.column)})` : col(s.column)));
    lines.push(`x <- x[order(${keys.join(", ")}), , drop = FALSE]`);
  }
  return `${lines.join("\n")}\n`;
}

function pythonCode({ code, matrix, columns }: Pick<TableRows, "code" | "matrix" | "columns">, sort: TableSort[], filters: TableFilter[]): string {
  const lines = matrix ? ["import pandas as pd", `x = pd.DataFrame(${code})`] : [`x = ${code}`];
  // Columns named by position (an array's, or digits) are read by position.
  const numbered = (i: number) => /^\d+$/.test(columns[i]?.name ?? "");
  const col = (i: number) => (numbered(i) ? `x.iloc[:, ${i}]` : `x[${pyString(columns[i]?.name ?? "")}]`);
  const value = (i: number, v: string) => (columns[i]?.kind === "number" ? v : pyString(v));
  const tests = filters.map((f) => {
    const values = f.values ?? [];
    const c = col(f.column);
    const text = `${c}.astype(str)`;
    switch (f.op) {
      case "missing":
        return `${c}.isna()`;
      case "present":
        return `${c}.notna()`;
      case "between":
        return `${c}.between(${value(f.column, values[0])}, ${value(f.column, values[1])})`;
      case "in":
        return `${text}.isin([${values.map(pyString).join(", ")}]) & ${c}.notna()`;
      case "not in":
        return `~${text}.isin([${values.map(pyString).join(", ")}]) & ${c}.notna()`;
      case "contains":
        return `${text}.str.contains(${pyString(values[0])}, case=False, regex=False) & ${c}.notna()`;
      default: {
        const op = f.op === "=" ? "==" : f.op;
        if (columns[f.column]?.kind === "number") return `${c} ${op} ${values[0]}`;
        if (columns[f.column]?.kind === "date") return `${c} ${op} ${pyString(values[0])}`;
        return `${text} ${op} ${pyString(values[0])}`;
      }
    }
  });
  if (tests.length) lines.push(`x = x[${tests.map((t) => (tests.length > 1 || t.includes(" & ") ? `(${t})` : t)).join(" & ")}]`);
  if (sort.length) {
    const by = sort.map((s) => (numbered(s.column) ? `x.columns[${s.column}]` : pyString(columns[s.column]?.name ?? "")));
    const ascending = sort.map((s) => (s.descending ? "False" : "True"));
    const one = sort.length === 1;
    lines.push(`x = x.sort_values(${one ? by[0] : `[${by.join(", ")}]`}, ascending=${one ? ascending[0] : `[${ascending.join(", ")}]`})`);
  }
  return `${lines.join("\n")}\n`;
}

function rString(s: string): string {
  return JSON.stringify(s);
}

function pyString(s: string): string {
  return JSON.stringify(s);
}

/** A file name for exporting: the table's last part, made safe. */
export function exportName(path: string[]): string {
  const name = (path[path.length - 1] ?? "table").replace(/[^\w.-]+/g, "-").replace(/^[-.]+|-+$/g, "");
  return `${name || "table"}.csv`;
}

const R_PART = /^(?:[$@]([A-Za-z.][\w.]*|`[^`]+`|"[^"\\]*")|\[\[("[^"\\]*"|'[^'\\]*'|\d+)\]\])/;
const PY_PART = /^(?:\.([A-Za-z_]\w*)|\[("[^"\\]*"|'[^'\\]*'|\d+)\])/;

/**
 * Where in the Variables pane what `View(...)` or `%view ...` was given is:
 * `seu@meta.data` is `["seu", "meta.data"]` and `adata.obs` is
 * `["adata", "obs"]`. Undefined for an expression that names no variable,
 * like `head(df)`.
 */
export function viewPath(title: string, language: string): string[] | undefined {
  const r = language === "r";
  let rest = title.trim();
  const first = (r ? /^([A-Za-z.][\w.]*|`[^`]+`)/ : /^([A-Za-z_]\w*)/).exec(rest);
  if (!first) return undefined;
  const unquote = (s: string) => (/^[`"']/.test(s) ? s.slice(1, -1) : s);
  const path = [unquote(first[1])];
  rest = rest.slice(first[0].length).trim();
  while (rest) {
    const part = (r ? R_PART : PY_PART).exec(rest);
    if (!part) return undefined;
    const [, name, index] = part;
    if (name !== undefined) path.push(unquote(name));
    else if (/^\d+$/.test(index)) path.push(r ? `[[${index}]]` : `[${index}]`);
    else path.push(unquote(index));
    rest = rest.slice(part[0].length).trim();
  }
  return path;
}
