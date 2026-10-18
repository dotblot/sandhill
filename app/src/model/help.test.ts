import { describe, expect, it } from "vitest";
import type { Package } from "../backend/types";
import {
  consoleHelp,
  docBlocks,
  exampleCode,
  filterPackages,
  helpLink,
  inlineParts,
  nameAt,
  installCode,
  packageNames,
  packageStatus,
  packageTopic,
  removeCode,
  signatureLines,
} from "./help";

describe("help asked for at the Console", () => {
  it("reads R's ways of asking", () => {
    expect(consoleHelp("?lm", "r")).toEqual({ topic: "lm", package: undefined });
    expect(consoleHelp(" ? read.csv ", "r")).toEqual({ topic: "read.csv", package: undefined });
    expect(consoleHelp("?stats::filter", "r")).toEqual({ topic: "filter", package: "stats" });
    expect(consoleHelp('?"[.data.frame"', "r")).toEqual({ topic: "[.data.frame", package: undefined });
    expect(consoleHelp("??principal components", "r")).toEqual({ topic: "principal components", search: true });
    expect(consoleHelp("help(lm)", "r")).toEqual({ topic: "lm", package: undefined });
    expect(consoleHelp('help("filter", package = "dplyr")', "r")).toEqual({ topic: "filter", package: "dplyr" });
    expect(consoleHelp("help(filter, dplyr)", "r")).toEqual({ topic: "filter", package: "dplyr" });
    expect(consoleHelp('help(package = "Seurat")', "r")).toEqual({ topic: "", package: "Seurat" });
  });

  it("leaves other code to run", () => {
    expect(consoleHelp("help()", "r")).toBeUndefined();
    expect(consoleHelp("x ? y", "r")).toBeUndefined();
    expect(consoleHelp("?lm\nx <- 1", "r")).toBeUndefined();
    expect(consoleHelp("help(lm, try.all.packages = TRUE)", "r")).toBeUndefined();
    expect(consoleHelp("help()", "python")).toBeUndefined();
    expect(consoleHelp("x = a if b else c", "python")).toBeUndefined();
  });

  it("reads Python's ways of asking", () => {
    expect(consoleHelp("?np.mean", "python")).toEqual({ topic: "np.mean" });
    expect(consoleHelp("sc.pp.neighbors?", "python")).toEqual({ topic: "sc.pp.neighbors" });
    expect(consoleHelp("pd.concat??", "python3")).toEqual({ topic: "pd.concat" });
    expect(consoleHelp("help(len)", "python")).toEqual({ topic: "len" });
    expect(consoleHelp('help("json.dumps")', "python")).toEqual({ topic: "json.dumps" });
  });
});

describe("the name under the cursor", () => {
  it("reads R's and Python's names", () => {
    expect(nameAt("x <- read.csv(file)", 9, "r")).toEqual({ topic: "read.csv" });
    expect(nameAt("y <- stats::filter(x, f)", 15, "r")).toEqual({ topic: "filter", package: "stats" });
    expect(nameAt("sc.pp.neighbors(adata)", 3, "python")).toEqual({ topic: "sc.pp.neighbors" });
    expect(nameAt("x = 1.5", 5, "python")).toBeUndefined();
    expect(nameAt("a + b", 2, "r")).toBeUndefined();
  });
});

describe("links in R help pages", () => {
  it("go to topics, examples, the package's index or the web", () => {
    expect(helpLink("../../stats/help/aov", "stats")).toEqual({ kind: "topic", package: "stats", topic: "aov" });
    expect(helpLink("../../base/help/%5B.data.frame")).toEqual({ kind: "topic", package: "base", topic: "[.data.frame" });
    expect(helpLink("../../utils/html/head.html")).toEqual({ kind: "topic", package: "utils", topic: "head" });
    expect(helpLink("../help/lm.fit", "stats")).toEqual({ kind: "topic", package: "stats", topic: "lm.fit" });
    expect(helpLink("../Example/lm", "stats")).toEqual({ kind: "example", package: "stats", topic: "lm" });
    expect(helpLink("00Index.html", "stats")).toEqual({ kind: "index", package: "stats" });
    expect(helpLink("../../Seurat/html/00Index.html")).toEqual({ kind: "index", package: "Seurat" });
    expect(helpLink("https://doi.org/10.2307/2346786")).toEqual({ kind: "web", url: "https://doi.org/10.2307/2346786" });
    expect(helpLink("#method")).toEqual({ kind: "anchor", id: "method" });
    expect(helpLink("javascript:alert(1)")).toBeUndefined();
  });

  it("runs examples in the topic's package", () => {
    expect(exampleCode("lm", "stats")).toBe('example("lm", package = "stats")');
    expect(exampleCode("[.data.frame")).toBe('example("[.data.frame")');
  });
});

describe("docstrings", () => {
  it("splits numpydoc into sections, entries and examples", () => {
    const doc = [
      "Concatenate pandas objects along a particular axis.",
      "",
      "Can also add a layer of hierarchical indexing on the concatenation axis,",
      "which may be useful.",
      "",
      "Parameters",
      "----------",
      "objs : an iterable or mapping of Series or DataFrame objects",
      "    If a mapping is passed, the keys will be used as the `keys`",
      "    argument.",
      "",
      "axis : {0/'index', 1/'columns'}, default 0",
      "    The axis to concatenate along.",
      "",
      "Returns",
      "-------",
      "DataFrame",
      "",
      "Examples",
      "--------",
      ">>> s1 = pd.Series(['a', 'b'])",
      ">>> pd.concat([s1, s1])",
      "0    a",
      "",
    ].join("\n");
    expect(docBlocks(doc)).toEqual([
      { kind: "text", text: "Concatenate pandas objects along a particular axis." },
      { kind: "text", text: "Can also add a layer of hierarchical indexing on the concatenation axis, which may be useful." },
      { kind: "heading", text: "Parameters" },
      {
        kind: "defs",
        section: "Parameters",
        items: [
          { term: "objs : an iterable or mapping of Series or DataFrame objects", text: "If a mapping is passed, the keys will be used as the `keys` argument." },
          { term: "axis : {0/'index', 1/'columns'}, default 0", text: "The axis to concatenate along." },
        ],
      },
      { kind: "heading", text: "Returns" },
      { kind: "defs", section: "Returns", items: [{ term: "DataFrame", text: "" }] },
      { kind: "heading", text: "Examples" },
      { kind: "code", text: ">>> s1 = pd.Series(['a', 'b'])\n>>> pd.concat([s1, s1])\n0    a" },
    ]);
  });

  it("reads Google style and reStructuredText code blocks", () => {
    const doc = "Fetch rows.\n\nArgs:\n    table (str): The table.\n    keys: Which rows,\n        and more.\n\nExample::\n\n    fetch('t', [1])\n\n- one\n- two";
    expect(docBlocks(doc)).toEqual([
      { kind: "text", text: "Fetch rows." },
      { kind: "heading", text: "Args" },
      { kind: "defs", section: "Args", items: [{ term: "table (str)", text: "The table." }, { term: "keys", text: "Which rows, and more." }] },
      { kind: "text", text: "Example:" },
      { kind: "code", text: "fetch('t', [1])" },
      { kind: "text", text: "- one\n- two" },
    ]);
  });

  it("reads directives inside a description as words", () => {
    const doc = "Parameters\n----------\ncopy : bool\n    Ignored.\n\n    .. deprecated:: 3.0.0\n       Will be removed.\n";
    expect(docBlocks(doc)).toEqual([
      { kind: "heading", text: "Parameters" },
      { kind: "defs", section: "Parameters", items: [{ term: "copy : bool", text: "Ignored. Deprecated since version 3.0.0: Will be removed." }] },
    ]);
  });

  it("shows reStructuredText's notes as notes, and its code as code", () => {
    const doc = "Merge.\n\n.. warning::\n   If both key columns contain nulls,\n   rows match.\n\n.. versionadded:: 1.2\n\n.. code-block:: python\n   :linenos:\n\n   merge(a, b)\n";
    expect(docBlocks(doc)).toEqual([
      { kind: "text", text: "Merge." },
      { kind: "note", title: "Warning", text: "If both key columns contain nulls, rows match." },
      { kind: "note", title: "Added in version 1.2", text: "" },
      { kind: "code", text: "merge(a, b)" },
    ]);
  });

  it("marks code and follows references", () => {
    expect(inlineParts("Use ``obj`` or `x`, see :func:`pandas.concat` and :class:`~anndata.AnnData`.")).toEqual([
      { text: "Use " },
      { code: "obj" },
      { text: " or " },
      { code: "x" },
      { text: ", see " },
      { ref: "pandas.concat", label: "pandas.concat" },
      { text: " and " },
      { ref: "anndata.AnnData", label: "AnnData" },
      { text: "." },
    ]);
    expect(inlineParts("the indexes *will be ignored* (2 * 3 * 4), **always**")).toEqual([
      { text: "the indexes " },
      { em: "will be ignored" },
      { text: " (2 * 3 * 4), " },
      { em: "always" },
    ]);
    expect(inlineParts("See the `user guide <https://pandas.pydata.org/guide.html>`__ for more.")).toEqual([
      { text: "See the " },
      { url: "https://pandas.pydata.org/guide.html", label: "user guide" },
      { text: " for more." },
    ]);
    // Labels in the docs aren't topics.
    expect(inlineParts(":ref:`the guide <guide-io>`")).toEqual([{ code: "the guide" }]);
  });

  it("puts a long signature's arguments on lines of their own", () => {
    expect(signatureLines("numpy.mean", "(a, axis=None)")).toBe("mean(a, axis=None)");
    expect(signatureLines("pandas.concat", "(objs, *, axis=0, join: str = 'outer', keys=(1, 2)) -> 'DataFrame'", 30)).toBe(
      "concat(\n    objs,\n    *,\n    axis=0,\n    join: str = 'outer',\n    keys=(1, 2),\n) -> 'DataFrame'",
    );
  });
});

const pkg = (name: string, more: Partial<Package> = {}): Package => ({
  name,
  version: "1.0",
  title: "",
  library: "/lib",
  attached: false,
  loaded: false,
  base: false,
  ...more,
});

describe("packages", () => {
  it("says which are in use", () => {
    expect(packageStatus(pkg("Seurat", { attached: true, loaded: true }), "r")).toBe("attached");
    expect(packageStatus(pkg("pandas", { attached: true, loaded: true }), "python")).toBe("imported");
    expect(packageStatus(pkg("Matrix", { loaded: true }), "r")).toBe("loaded");
    expect(packageStatus(pkg("DESeq2"), "r")).toBe("");
  });

  it("filters by name, then title", () => {
    const all = [pkg("ggplot2", { title: "Create Elegant Data Visualisations" }), pkg("DESeq2", { title: "Differential gene expression" }), pkg("plotly")];
    expect(filterPackages(all, "plot").map((p) => p.name)).toEqual(["ggplot2", "plotly"]);
    expect(filterPackages(all, "gene").map((p) => p.name)).toEqual(["DESeq2"]);
    expect(filterPackages(all, " ")).toBe(all);
  });

  it("reads the names typed to install", () => {
    expect(packageNames("DESeq2, harmony  scran", "r")).toEqual(["DESeq2", "harmony", "scran"]);
    expect(packageNames("rm -rf", "r")).toBeUndefined();
    expect(packageNames("scanpy[leiden]>=1.10 anndata", "python")).toEqual(["scanpy[leiden]>=1.10", "anndata"]);
    expect(packageNames("x; import os", "python")).toBeUndefined();
    expect(packageNames("  ", "r")).toBeUndefined();
  });

  it("installs with BiocManager when it's there, and sets a CRAN mirror when none is", () => {
    expect(installCode("r", ["DESeq2"], { cran: true, bioconductor: true })).toBe('BiocManager::install("DESeq2", update = FALSE, ask = FALSE)');
    expect(installCode("r", ["harmony", "scran"], { cran: false, bioconductor: false })).toBe(
      'options(repos = c(CRAN = "https://cloud.r-project.org"))\ninstall.packages(c("harmony", "scran"))',
    );
    expect(installCode("python", ["scanpy", "leidenalg"], { cran: false, bioconductor: false })).toBe("%pip install scanpy leidenalg");
    expect(installCode("python", ["pysam"], { cran: false, bioconductor: false }, true)).toBe("%conda install -y -c conda-forge -c bioconda pysam");
  });

  it("removes with the tool that installed", () => {
    expect(removeCode("r", [pkg("harmony"), pkg("scran")])).toBe('remove.packages(c("harmony", "scran"))');
    expect(removeCode("python", [pkg("leidenalg", { installer: "conda" }), pkg("scanpy", { installer: "pip" }), pkg("x")])).toBe(
      "%conda remove -y leidenalg\n%pip uninstall -y scanpy x",
    );
  });

  it("opens a package's help", () => {
    expect(packageTopic(pkg("Seurat"), "r")).toEqual({ topic: "", package: "Seurat" });
    expect(packageTopic(pkg("scikit-learn", { modules: ["sklearn"] }), "python")).toEqual({ topic: "sklearn" });
    expect(packageTopic(pkg("typing-extensions", { modules: ["typing_extensions"] }), "python")).toEqual({ topic: "typing_extensions" });
    expect(packageTopic(pkg("PyYAML", { modules: ["_yaml", "yaml"] }), "python")).toEqual({ topic: "yaml" });
  });
});
