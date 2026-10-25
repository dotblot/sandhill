// Variables' Import menu: RStudio's list of ways to read a dataset into R,
// plus R data files and 10x Genomics output, and the same for Python. Each
// writes the code it will run, which the Import dialog shows first.

import { normalizeLanguage } from "./workspace";

export interface ImportKind {
  id: string;
  label: string;
  /** The function, shown at the right of the menu. */
  fn: string;
  /** File extensions it reads, lower case; a folder for 10x output. */
  exts: string[];
  folder?: boolean;
  /** The menu draws a line before it. */
  group?: boolean;
  /** The code for a file and the variable to read it into. */
  code(path: string, name: string): string;
  /** False when the code names its own variables (R's load()). */
  named(path: string): boolean;
}

const q = (path: string) => JSON.stringify(path);
const ext = (path: string) => path.slice(path.lastIndexOf(".") + 1).toLowerCase();
const tabbed = (path: string) => ["tsv", "tab", "txt"].includes(ext(path.replace(/\.gz$/i, "")));
const always = () => true;

const R_IMPORTS: ImportKind[] = [
  {
    id: "base",
    label: "From text (base)…",
    fn: "read.csv()",
    exts: ["csv", "tsv", "txt", "tab", "gz"],
    code: (p, n) => `${n} <- ${tabbed(p) ? "read.delim" : "read.csv"}(${q(p)})`,
    named: always,
  },
  {
    id: "readr",
    label: "From text (readr)…",
    fn: "readr::read_csv()",
    exts: ["csv", "tsv", "txt", "tab", "gz"],
    code: (p, n) => `${n} <- readr::${tabbed(p) ? "read_tsv" : "read_csv"}(${q(p)})`,
    named: always,
  },
  {
    id: "excel",
    label: "From Excel…",
    fn: "readxl::read_excel()",
    exts: ["xlsx", "xls"],
    group: true,
    code: (p, n) => `${n} <- readxl::read_excel(${q(p)})`,
    named: always,
  },
  {
    id: "spss",
    label: "From SPSS…",
    fn: "haven::read_sav()",
    exts: ["sav", "zsav", "por"],
    group: true,
    code: (p, n) => `${n} <- haven::read_sav(${q(p)})`,
    named: always,
  },
  { id: "sas", label: "From SAS…", fn: "haven::read_sas()", exts: ["sas7bdat"], code: (p, n) => `${n} <- haven::read_sas(${q(p)})`, named: always },
  { id: "stata", label: "From Stata…", fn: "haven::read_dta()", exts: ["dta"], code: (p, n) => `${n} <- haven::read_dta(${q(p)})`, named: always },
  {
    id: "rdata",
    label: "From R data (.rds, .RData)…",
    fn: "readRDS()",
    exts: ["rds", "rdata", "rda"],
    group: true,
    code: (p, n) => (ext(p) === "rds" ? `${n} <- readRDS(${q(p)})` : `load(${q(p)})`),
    named: (p) => ext(p) === "rds",
  },
  {
    id: "10x",
    label: "From 10x Genomics…",
    fn: "Seurat::Read10X()",
    exts: ["h5"],
    folder: true,
    code: (p, n) => (ext(p) === "h5" ? `${n} <- Seurat::Read10X_h5(${q(p)})` : `${n} <- Seurat::Read10X(data.dir = ${q(p)})`),
    named: always,
  },
];

const PYTHON_IMPORTS: ImportKind[] = [
  {
    id: "pandas",
    label: "From text (pandas)…",
    fn: "pd.read_csv()",
    exts: ["csv", "tsv", "txt", "tab", "gz"],
    code: (p, n) => `import pandas as pd\n${n} = pd.read_csv(${q(p)}${tabbed(p) ? ', sep="\\t"' : ""})`,
    named: always,
  },
  {
    id: "excel",
    label: "From Excel…",
    fn: "pd.read_excel()",
    exts: ["xlsx", "xls"],
    code: (p, n) => `import pandas as pd\n${n} = pd.read_excel(${q(p)})`,
    named: always,
  },
  {
    id: "parquet",
    label: "From Parquet…",
    fn: "pd.read_parquet()",
    exts: ["parquet", "pq"],
    code: (p, n) => `import pandas as pd\n${n} = pd.read_parquet(${q(p)})`,
    named: always,
  },
  {
    id: "anndata",
    label: "From AnnData (.h5ad)…",
    fn: "ad.read_h5ad()",
    exts: ["h5ad"],
    group: true,
    code: (p, n) => `import anndata as ad\n${n} = ad.read_h5ad(${q(p)})`,
    named: always,
  },
  {
    id: "10x",
    label: "From 10x Genomics…",
    fn: "sc.read_10x_mtx()",
    exts: ["h5"],
    folder: true,
    code: (p, n) => `import scanpy as sc\n${n} = sc.${ext(p) === "h5" ? "read_10x_h5" : "read_10x_mtx"}(${q(p)})`,
    named: always,
  },
];

export function importsFor(language: string): ImportKind[] {
  return normalizeLanguage(language) === "r" ? R_IMPORTS : normalizeLanguage(language) === "python" ? PYTHON_IMPORTS : [];
}

/** Whether a file is one the import reads (compressed text counts as text). */
export function importable(kind: ImportKind, name: string): boolean {
  const lower = name.toLowerCase();
  if (lower.endsWith(".gz")) return kind.exts.includes("gz") && kind.exts.includes(ext(lower.slice(0, -3)));
  return kind.exts.includes(ext(lower));
}

/** A variable name from a file's name: "pbmc3k_counts.csv" is pbmc3k_counts, "10x out" is x10x_out. */
export function variableName(path: string): string {
  const base = path.replace(/\/+$/, "").split("/").pop() ?? "";
  const stem = base.replace(/(\.(gz|bz2|xz|zst))$/i, "").replace(/\.[^.]+$/, "") || base;
  let name = stem.replace(/[^A-Za-z0-9_]+/g, "_").replace(/^_+|_+$/g, "");
  if (!name) return "data";
  if (/^[0-9]/.test(name)) name = `x${name}`;
  return name;
}

/** Whether `name` can be assigned to as is. */
export function validVariable(name: string, language: string): boolean {
  if (normalizeLanguage(language) === "r") return /^([A-Za-z]|\.[A-Za-z_.])[A-Za-z0-9._]*$/.test(name) || name === ".";
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
}

/** The path as the code should say it: relative to the session's folder when inside it. */
export function codePath(path: string, cwd?: string): string {
  if (!cwd) return path;
  const dir = cwd.replace(/\/+$/, "");
  return path.startsWith(`${dir}/`) ? path.slice(dir.length + 1) : path;
}
