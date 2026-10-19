import { describe, expect, it } from "vitest";
import type { TableColumn } from "../backend/types";
import { columnWidth, exportName, filterError, filterText, formatCell, formatNumber, nextSort, opsFor, sortText, tableCode } from "./table";

const COLUMNS: TableColumn[] = [
  { name: "nCount_RNA", kind: "number", type: "dbl" },
  { name: "cell type", kind: "category", type: "fct" },
  { name: "barcode", kind: "text", type: "chr" },
  { name: "percent.mt", kind: "number", type: "dbl" },
];

describe("filters", () => {
  it("offers the conditions a column's kind takes", () => {
    expect(opsFor("number").map((o) => o.op)).toEqual(["<", "<=", ">", ">=", "between", "=", "!=", "missing", "present"]);
    expect(opsFor("category").map((o) => o.label)).toEqual(["is one of", "is none of", "is missing", "is not missing"]);
    expect(opsFor("text")[0].op).toBe("contains");
  });

  it("reads like a sentence on its chip", () => {
    expect(filterText({ column: 3, op: "<", values: ["5"] }, COLUMNS)).toBe("percent.mt < 5");
    expect(filterText({ column: 0, op: "between", values: ["500", "2500"] }, COLUMNS)).toBe("nCount_RNA between 500 and 2500");
    expect(filterText({ column: 1, op: "in", values: ["B", "NK"] }, COLUMNS)).toBe("cell type is one of B, NK");
    expect(filterText({ column: 2, op: "contains", values: ["AAAC"] }, COLUMNS)).toBe('barcode contains "AAAC"');
    expect(filterText({ column: 0, op: "missing" }, COLUMNS)).toBe("nCount_RNA is missing");
  });

  it("asks for numbers where numbers go", () => {
    expect(filterError({ column: 3, op: "<", values: ["5"] }, COLUMNS[3])).toBeUndefined();
    expect(filterError({ column: 3, op: "<", values: ["five"] }, COLUMNS[3])).toBe("Give a number.");
    expect(filterError({ column: 0, op: "between", values: ["1", ""] }, COLUMNS[0])).toBe("Give a value.");
    expect(filterError({ column: 1, op: "in", values: [] }, COLUMNS[1])).toBe("Give a value.");
    expect(filterError({ column: 1, op: "missing" }, COLUMNS[1])).toBeUndefined();
    expect(filterError({ column: 1, op: "<", values: ["1"] }, COLUMNS[1])).toBe("Pick a condition.");
  });
});

describe("cells", () => {
  it("groups digits, keeps four decimals, and shows missing the language's way", () => {
    expect(formatNumber(15818)).toBe("15,818");
    expect(formatNumber(2.123456)).toBe("2.1235");
    expect(formatNumber(0.00001234)).toBe("1.234e-5");
    expect(formatNumber(3.2e12)).toBe("3,200,000,000,000");
    expect(formatNumber(3.25e12)).toBe("3,250,000,000,000");
    expect(formatNumber(1.5e15)).toBe("1.500e+15");
    expect(formatCell(null, "r")).toBe("NA");
    expect(formatCell(null, "python")).toBe("NaN");
    expect(formatCell("CD14+ Mono")).toBe("CD14+ Mono");
  });

  it("sizes a column to its longest cell, within limits", () => {
    expect(columnWidth(COLUMNS[0], ["15,818"])).toBe(108);
    expect(columnWidth({ name: "x", kind: "number", type: "int" }, ["1"])).toBe(64);
    expect(columnWidth(COLUMNS[2], ["a".repeat(200)])).toBe(320);
  });
});

describe("sorting", () => {
  it("goes ascending, descending, then off", () => {
    expect(nextSort([], 0, false)).toEqual([{ column: 0, descending: false }]);
    expect(nextSort([{ column: 0, descending: false }], 0, false)).toEqual([{ column: 0, descending: true }]);
    expect(nextSort([{ column: 0, descending: true }], 0, false)).toEqual([]);
    expect(nextSort([{ column: 0, descending: true }], 1, false)).toEqual([{ column: 1, descending: false }]);
  });

  it("adds a key with shift, keeping the others", () => {
    const sort = [{ column: 0, descending: true }];
    expect(nextSort(sort, 1, true)).toEqual([{ column: 0, descending: true }, { column: 1, descending: false }]);
    expect(nextSort([...sort, { column: 1, descending: false }], 1, true)).toEqual([{ column: 0, descending: true }, { column: 1, descending: true }]);
    expect(nextSort([...sort, { column: 1, descending: true }], 0, true)).toEqual([{ column: 1, descending: true }]);
  });

  it("says how the rows are sorted", () => {
    expect(sortText([], COLUMNS)).toMatch(/^Not sorted/);
    expect(sortText([{ column: 0, descending: true }], COLUMNS)).toBe("Sorted by nCount_RNA, descending");
    expect(sortText([{ column: 0 }], COLUMNS)).toBe("Sorted by nCount_RNA, ascending");
    expect(sortText([{ column: 1 }, { column: 0, descending: true }], COLUMNS)).toBe("Sorted by cell type, then nCount_RNA, descending");
  });
});

describe("code for the view", () => {
  const filters = [
    { column: 3, op: "<", values: ["5"] },
    { column: 1, op: "in", values: ["B", "NK"] },
  ];
  const sort = [{ column: 0, descending: true }];
  const table = { code: "seu@meta.data", columns: COLUMNS };

  it("writes R", () => {
    expect(tableCode("R", table, sort, filters)).toBe(
      "x <- seu@meta.data\n" +
        'x <- x[which(x$percent.mt < 5 & x[["cell type"]] %in% c("B", "NK")), , drop = FALSE]\n' +
        "x <- x[order(-xtfrm(x$nCount_RNA)), , drop = FALSE]\n",
    );
    expect(tableCode("r", { ...table, code: "m", matrix: true, columns: [{ name: "[,1]", kind: "number", type: "int" }, { name: "g2", kind: "number", type: "int" }] }, [{ column: 1 }], [{ column: 0, op: "between", values: ["1", "2"] }])).toBe(
      'x <- m\nx <- x[which(x[, 1] >= 1 & x[, 1] <= 2), , drop = FALSE]\nx <- x[order(x[, "g2"]), , drop = FALSE]\n',
    );
    expect(tableCode("r", table, [], [{ column: 2, op: "contains", values: ["AaAC"] }])).toBe('x <- seu@meta.data\nx <- x[which(grepl("aaac", tolower(x$barcode), fixed = TRUE)), , drop = FALSE]\n');
    expect(tableCode("r", table, [], [])).toBe("x <- seu@meta.data\n");
  });

  it("writes Python", () => {
    expect(tableCode("python", { ...table, code: "adata.obs" }, [...sort, { column: 1 }], filters)).toBe(
      "x = adata.obs\n" +
        'x = x[(x["percent.mt"] < 5) & (x["cell type"].astype(str).isin(["B", "NK"]) & x["cell type"].notna())]\n' +
        'x = x.sort_values(["nCount_RNA", "cell type"], ascending=[False, True])\n',
    );
    expect(tableCode("python", { code: "arr", matrix: true, columns: [{ name: "0", kind: "number", type: "float64" }] }, [{ column: 0, descending: true }], [{ column: 0, op: "missing" }])).toBe(
      "import pandas as pd\nx = pd.DataFrame(arr)\nx = x[x.iloc[:, 0].isna()]\nx = x.sort_values(x.columns[0], ascending=False)\n",
    );
    expect(tableCode("python", { ...table, code: "df" }, [], [{ column: 2, op: "=", values: ["AAAC-1"] }])).toBe('x = df\nx = x[x["barcode"].astype(str) == "AAAC-1"]\n');
  });
});

it("names an export after the table", () => {
  expect(exportName(["seu", "meta.data"])).toBe("meta.data.csv");
  expect(exportName(["adata", "obs"])).toBe("obs.csv");
  expect(exportName(["my table!"])).toBe("my-table.csv");
  expect(exportName([])).toBe("table.csv");
});
