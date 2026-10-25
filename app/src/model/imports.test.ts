import { describe, expect, it } from "vitest";
import { codePath, importable, importsFor, validVariable, variableName } from "./imports";

const r = (id: string) => importsFor("R").find((k) => k.id === id)!;
const py = (id: string) => importsFor("python").find((k) => k.id === id)!;

describe("imports", () => {
  it("lists RStudio's imports for R, with the function each uses", () => {
    expect(importsFor("R").map((k) => k.fn)).toEqual([
      "read.csv()",
      "readr::read_csv()",
      "readxl::read_excel()",
      "haven::read_sav()",
      "haven::read_sas()",
      "haven::read_dta()",
      "readRDS()",
      "Seurat::Read10X()",
    ]);
    expect(importsFor("julia")).toEqual([]);
  });

  it("writes the code it will run", () => {
    expect(r("readr").code("data/counts.tsv", "counts")).toBe('counts <- readr::read_tsv("data/counts.tsv")');
    expect(r("base").code("donors.csv", "donors")).toBe('donors <- read.csv("donors.csv")');
    expect(r("rdata").code("obj.rds", "obj")).toBe('obj <- readRDS("obj.rds")');
    expect(r("rdata").code("all.RData", "x")).toBe('load("all.RData")');
    expect(r("rdata").named("all.RData")).toBe(false);
    expect(r("10x").code("filtered_feature_bc_matrix", "pbmc")).toBe('pbmc <- Seurat::Read10X(data.dir = "filtered_feature_bc_matrix")');
    expect(py("pandas").code("a b.tsv.gz", "ab")).toBe('import pandas as pd\nab = pd.read_csv("a b.tsv.gz", sep="\\t")');
    expect(py("anndata").code("pbmc3k.h5ad", "pbmc3k")).toBe('import anndata as ad\npbmc3k = ad.read_h5ad("pbmc3k.h5ad")');
  });

  it("knows which files each reads", () => {
    expect(importable(r("readr"), "reads.csv.gz")).toBe(true);
    expect(importable(r("readr"), "reads.fastq.gz")).toBe(false);
    expect(importable(r("excel"), "Sheet.XLSX")).toBe(true);
    expect(importable(r("rdata"), "x.rds")).toBe(true);
  });

  it("names the variable after the file", () => {
    expect(variableName("/w/pbmc3k_counts.csv")).toBe("pbmc3k_counts");
    expect(variableName("/w/10x out/")).toBe("x10x_out");
    expect(variableName("/w/counts.tsv.gz")).toBe("counts");
    expect(validVariable("my.data", "R")).toBe(true);
    expect(validVariable("my.data", "python")).toBe(false);
    expect(validVariable("2x", "R")).toBe(false);
  });

  it("writes paths inside the session's folder relative to it", () => {
    expect(codePath("/w/p/data/a.csv", "/w/p")).toBe("data/a.csv");
    expect(codePath("/w/other/a.csv", "/w/p")).toBe("/w/other/a.csv");
  });
});
