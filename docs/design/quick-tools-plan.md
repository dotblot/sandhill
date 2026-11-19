# Quick Tools

Huan, 2026-11-19. Status: plan, waiting for Huan's approval.

Huan often sends a table (DESeq2 results, raw counts, Gene Ontology
results) to colleagues who aren't bioinformaticians. They want a quick
overview and a few standard plots without making a folder or a project.
Quick Tools gives them that: a page in Welcome, a simplified window with
only the Data Explorer, Plots and an AI helper, and Sandhill's own tested
plots and analyses.

Decided by Huan (2026-11-19):
- Welcome gets Quick Tools, with Quick Analysis (bulk RNA-seq
  differential expression, gene enrichment) and Quick Plot (volcano,
  heatmap and the other usual bulk RNA-seq plots from CSV, TSV or Excel).
  Other omics come later in the same design.
- A simplified window with only the Data Explorer, Plots and AI.
- Everything works without an AI account; the AI is a helper.
- Sandhill doesn't carry R, Python or their packages. It checks what the
  computer has and guides the user to install what is missing.
- Delimited files and Excel first.

## 1. Welcome → Quick Tools
- Quick Tools is the first row of Welcome's sidebar, above Devices.
  Welcome opens on it when Sandhill knows no projects (a first launch);
  otherwise it opens on This Mac's page as now.
- The page, top to bottom:
  1. Getting ready (section 2). When everything is there it shrinks to
     one green line: "Ready: R 4.6.2, Quick Plot and Quick Analysis".
  2. A drop area: "Drop a table here" and Choose a File… (`.csv`,
     `.tsv`, `.txt`, `.xlsx`, `.xls`).
  3. Quick Plot and Quick Analysis, each a short list of what it makes
     (section 5). Clicking one asks for the file and opens the window
     with that goal chosen; dropping a file lets Sandhill suggest goals
     from the table.
  4. Recent sessions: name, date and tables, with a ⋯ menu: Open, Show
     in Finder, Open as Project, Delete… (asked in the row).

## 2. Getting ready
Sandhill is dragged into Applications from its disk image, so there is
no install step to run a check in. The check runs whenever the Quick Tools
page or a Quick Tools window opens, and on Check Again. A first launch opens
Welcome on Quick Tools, so in practice it runs right after installing.

| Item | Needed for | How Sandhill helps |
|---|---|---|
| R 4.4 or newer | everything | Get R… opens CRAN's Mac page in the browser and says which file fits this Mac: the one ending in `arm64.pkg` on Apple silicon, `x86_64.pkg` on Intel. Sandhill checks again when its window becomes active. |
| IRkernel | everything | The existing Install IRkernel action, run as a job. |
| ggplot2, ggrepel, pheatmap, readxl, writexl, svglite | Quick Plot | Install Missing Packages |
| BiocManager, DESeq2 | Quick Analysis: differential expression | the same button |
| clusterProfiler, and org.Hs.eg.db (human) or org.Mm.eg.db (mouse) | Quick Analysis: enrichment | the same button; an organism's package is offered when first needed |
| Claude Code or Codex, signed in | the AI helper (optional) | a link to Welcome → Agents; never blocks anything |

- R is found on the login `PATH`, else where CRAN's installer puts it
  (`/Library/Frameworks/R.framework/Resources/bin/R`).
- What is installed is read by one fixed R script, without starting a
  kernel.
- Installing is one job. The card shows each package as it goes and Show
  details has the full log.
  - Packages go into the user's own R library (`R_LIBS_USER`, made if
    missing), so no administrator password is asked.
  - Binary packages only, never compiled, so Apple's developer tools
    aren't needed. When a binary isn't available (an R newer than
    Bioconductor's builds), the card says so and offers the command to
    copy instead.
  - CRAN through its cloud mirror, Bioconductor through BiocManager.
    Packages the user already has are never updated or removed.

## 3. The Quick Tools window
- Opens when a table is dropped or chosen, or a recent session opened;
  one window per session. Closing it keeps the session in the list. If no
  other Sandhill window is open, Welcome comes back.
- Three columns: the Data Explorer and Plots side by side, about three
  eighths of the width each, and the Guide on the right, a quarter, like
  the AI pane in the main window.
- The title bar: the session's name (click to rename), one capsule per
  table (an Excel workbook has one per sheet, under a sheet menu), Export…
  and Open as Project.
- Dropping more files onto the window adds tables, such as a count table
  and its sample sheet.
- The Data Explorer and Plots are the existing panes, unchanged.
- The Guide column: the What we know card, the buttons for what can be
  made, then the AI conversation. Without AI it says "The AI helper isn't
  set up. Everything here works without it." with Set Up AI….
- Not in this window: Files, the editor, Console, Terminal, Jobs, Git,
  Variables and kernel pickers. Errors show in the Guide in plain words,
  with Show details.

## 4. The What we know card
Each item shows where it came from: Found in the table, You told us,
Suggested by AI (waiting for a click), or Still needed. Clicking an item
changes it.

- What each table is: differential expression results (DESeq2, edgeR,
  limma), raw counts, normalized expression, a sample sheet, enrichment
  results, or not recognised. Recognition never guesses silently: whole
  numbers in sample columns read as counts, decimals as normalized values,
  and anything unclear becomes "What are these numbers?" with choices.
- The gene column and its kind (Ensembl, symbol, Entrez) and the organism.
  Ensembl prefixes suggest one (`ENSG` human, `ENSMUSG` mouse); symbols
  need the user.
- Gene names Excel turned into dates (`1-Mar`, `2-Sep`) are found and
  shown as a warning on the table.
- For results: the fold change, adjusted p-value and average expression
  columns, and the comparison, "higher in ___ than in ___". DESeq2
  tables don't record it, so it is usually Still needed.
- For counts and expression: the sample columns, and which samples belong
  to which group (sample chips dragged into groups, or read from a sample
  sheet). Batch or pairs are optional.
- Cutoffs: adjusted p below 0.05 and at least a two-fold change
  (|log2 fold change| ≥ 1) by default, changeable.

Each plot and analysis lists the items it needs. Its button works once they
are known; hovering a button that doesn't work yet says what is missing
("Needs: which samples are controls"). An AI suggestion takes one click to
accept (Accept All accepts every one), so a wrong guess about the direction
of a comparison can't reach a plot unseen.

## 5. What can be made

### Quick Plot (phase 1)
| From | Plots and tables | Needs |
|---|---|---|
| Differential expression results | volcano; MA plot (with an average expression column); top genes table; genes up and down; heatmap when the table has a value per sample | fold change and adjusted p (the comparison only for the labels) |
| Raw counts | library sizes; PCA (log2 counts per million, the 500 most variable genes); sample correlation heatmap; heatmap of the most variable genes; one gene by group | groups only for colours |
| Normalized expression | the same, without library sizes | as above |
| Enrichment results (clusterProfiler, g:Profiler, DAVID tables) | dot plot; bar plot of the top terms | term name and adjusted p (gene count or ratio for the dot plot) |

Every plot also writes a plain sentence of what it shows, from the numbers
("1,234 genes have adjusted p below 0.05: 610 higher in treated, 624
lower"). This comes from the template, not from AI.

### Quick Analysis
- Differential expression (phase 3): counts and groups to DESeq2, with
  `~ group`, or `~ batch + group` and pairs when given. It makes the full
  results table (every tested gene) and normalized counts, then offers
  the Quick Plot choices for the result. It refuses, saying why, when the
  values aren't whole numbers ("These look normalized; DESeq2 needs raw
  counts") or a group has fewer than two samples.
- Enrichment (phase 4): a results table, or a pasted gene list, and the
  organism, to GO over-representation with clusterProfiler's `enrichGO`
  for all three ontologies. Up and down genes are tested separately, and
  the background is every gene tested in the results table, not the whole
  genome. Works offline (GO comes from the organism's package). Its table
  and dot plot follow.

## 6. Export
- Plots: the existing save window (PNG, PDF, SVG) with Save recipe
  beside it.
- Tables: Excel with a Read_me sheet (what each column means, the
  comparison, cutoffs, and the template and package versions) or CSV.
- Export…: a chosen folder gets the figures, the workbook, `methods.txt`
  (a methods paragraph and figure captions made from the card and versions)
  and `steps.R` (the code that ran).
- Open as Project opens the session folder as an ordinary project in the
  main window, for a bioinformatician to check or continue.

## 7. The AI helper (phase 2)
- Claude Code or Codex, as turned on in Welcome → Agents.
- Before a session's first message, a line above the message box names
  where things go: "Your messages, and any part of your tables the AI
  reads, are sent to Anthropic (Claude Code)." Continue Without AI hides
  the conversation for that session.
- A conversation starts with the session's tables as @ chips (shown, and
  removable) and Sandhill's Quick Tools guide as its instruction: plain
  words, say what the data can't show, never name a direction the card
  doesn't know.
- Tools through Sandhill's tool server: `quick_state` (the card, tables
  and what they were recognised as), `quick_suggest` (fills card items as
  Suggested by AI) and `quick_make` (runs a template with accepted items
  only). The AI explains the result from the template's numbers.
- The AI may still write its own code when asked for something no template
  makes. Such plots are labelled "Made by AI code" in Plots and in exports.
- Its options for the user are the suggestion chips agents already end
  replies with.
- Agents work in the Quick Tools folder (section 8), so Claude's folder
  question is asked once, not per session.

## 8. Where things are kept
- `~/Documents/Sandhill Quick Tools/<date> <first table's name>/`: copies
  of the dropped files (the originals stay as they are), `results/` for
  exports, `steps.R`, and `.sandhill/quick.json` with the card and the
  tables.
- The daemon trusts `~/Documents/Sandhill Quick Tools/` when it makes it:
  Sandhill made the folder and only copies tables into it. Every other
  folder keeps the trust question.
- A session is a project the daemon knows, marked as quick, so kernels,
  the journal and file history work as in any project. It is listed on the
  Quick Tools page, and under Projects only after Open as Project.
- This Mac only.

## 9. How it is built
- Daemon, `crates/daemon/src/quick/`:
  - `readiness.rs` finds R, reads what is installed and makes the install
    job.
  - `recognise.rs` names each table from its column names, types and the
    Data Explorer's column summaries. Plain Rust, no kernel.
  - `templates.rs` runs fixed R scripts from `crates/daemon/assets/quick/`
    (load, volcano, MA, PCA, heatmaps, library sizes, gene by group,
    enrichment plots, DESeq2, `enrichGO`, workbook). Settings go in as
    JSON, never as code, as `bio/checks.rs` already does.
  - `card.rs` reads and writes `quick.json`.
  - Agent tools in `crates/daemon/src/mcp/quick.rs`.
  - New calls in `crates/proto/src/lib.rs`: `QuickReadiness`,
    `QuickInstall`, `QuickOpen`, `QuickState`, `QuickSet`, `QuickMake`,
    `QuickExport`.
- App, `app/src/quick/`: the window (opened from
  `app/src/shell/app_window.rs`), the Guide column and card, and the
  Welcome page (`app/src/welcome/start_screen`). It reuses
  `app/src/panes/explorer/`, `app/src/panes/plots/` and
  `app/src/panes/irkernel.rs`.
- This touches more than eight files and adds a module to both the daemon
  and the app, which is why it is built in phases.

## 10. Phases
Each phase ships on its own and is useful without the next.
1. Quick Plot without AI: the Welcome page with Getting ready and
   sessions, the window, loading CSV, TSV and Excel, recognition, the card
   with answers by hand, the Quick Plot plots with their sentences, and
   export.
2. The AI helper.
3. Quick Analysis: differential expression with DESeq2.
4. Quick Analysis: GO enrichment.

## 11. Settled rules this changes
Written into the decision files, with the date, once approved.
- `docs/decisions/window.md`: Quick Tools first in Welcome's sidebar,
  which opens on it when Sandhill knows no projects; a third kind of
  window, one per Quick Tools session, beside the main window and Welcome.
- `docs/decisions/devices.md`: the Quick Tools folder Sandhill makes is
  trusted without asking.
- `docs/decisions/agents.md`: Quick Tools conversations start with their
  tables attached as visible @ chips. Elsewhere messages still carry only
  what the user adds.

## 12. Not included
- Python templates (section 13), servers, and other omics (single-cell,
  proteomics); these come later in the same design.
- KEGG, Reactome and ranked enrichment (GSEA). KEGG needs the internet
  and has its own licence terms.
- Opening `.rds` and `.qs2` files, and Finder's Open With.
- Editing values in a table, and plots of tables that weren't recognised
  beyond what the AI makes on request.

## 13. Choices made in this plan
- R only for the templates: DESeq2 is R's own package, GO enrichment
  works offline with clusterProfiler and the organism packages, and users
  install one language. Python was the alternative: PyDESeq2 is a
  re-implementation of DESeq2, and gseapy's usual Enrichr route sends gene
  lists to a web server.
- The window's three columns and their widths (section 3).
- AI suggestions need a click before a plot uses them (section 4).
- The folder `~/Documents/Sandhill Quick Tools/` (section 8).

## 14. Checks
- Daemon tests without a kernel: recognition cases in
  `tests/fixtures/quick_tables.json`: DESeq2, edgeR and limma results,
  featureCounts counts (with `Chr`, `Start`, `End`, `Strand`, `Length`),
  non-whole estimated counts (asked about), TPM, a sample sheet,
  clusterProfiler and g:Profiler results, and an Excel column with gene
  names turned into dates.
- Daemon tests with a real R kernel, skipped with a message when R or the
  packages aren't installed: every template on the sample project's data;
  the DESeq2 template gives the same results as DESeq2 run directly; it
  refuses decimals and single-sample groups; the enrichment background is
  the tested genes.
- Getting ready: a package missing from a temporary R library shows as
  missing; installing into a temporary `R_LIBS_USER` works (skipped
  without internet).
- App tests: a button stays off with the right "Needs:" text until its
  items are known; an AI suggestion that hasn't been accepted doesn't
  reach a plot.
- Huan's acceptance, on a Mac user account without R: from Sandhill's
  first launch to an exported volcano plot of a DESeq2 table, following
  only what Sandhill shows. Then a colleague: given a DESeq2 table, they
  find their gene and send back a figure without help.
