# Explore a Table: gene symbols for Ensembl and Entrez IDs

Huan, 2026-11-19. Status: approved by Huan (2026-11-19) as written:
org.Hs.eg.db and org.Mm.eg.db, human and mouse now.

Huan asked (2026-11-19) for ID conversion in Explore a Table: tables from
GEO often name genes only by Ensembl ID (`ENSG00000141510`), and people
read gene symbols (`TP53`).

## What the user sees
- On the What we know card, when a table's gene column holds Ensembl or
  Entrez IDs, the gene fact gets **Add Gene Symbols** (for example
  "Genes: gene_ID · Ensembl IDs · Add Gene Symbols").
- It needs the organism. Ensembl prefixes already tell human (`ENSG`) or
  mouse (`ENSMUSG`); Entrez IDs, or anything else, use the card's organism
  fact, which the user can set.
- Clicking it adds a `symbol` column beside the ID column in the session's
  copy of the table. The file on disk is never changed. The Data Explorer
  shows the new column, and the card says what happened in plain words:
  "15,890 of 16,525 genes got a symbol; 635 have none and keep their
  Ensembl ID".
- From then on, plots label genes by symbol (volcano labels, heatmap rows,
  Gene by Group's search, the top genes table), falling back to the ID for
  genes without one. The ID column stays, so nothing is lost.
- Version suffixes (`ENSG00000141510.17`) are dropped before matching.
  When one ID matches several symbols, the first is used, as
  `AnnotationDbi::mapIds` does by default; the count of such IDs is shown.
- Undo: the gene fact gets **Use IDs Instead**, which goes back to labelling
  by ID (the column stays in the table).

## Where symbols come from
Bioconductor's organism packages, through `AnnotationDbi::mapIds`:
`org.Hs.eg.db` for human, `org.Mm.eg.db` for mouse. They work offline once
installed and are the usual way R users do this, so the session's
`steps.R` recipe stays plain, standard R that runs anywhere.

They are large (about 90 MB each), so they are not part of "Install
Missing Packages". When the needed one is missing, the button says
"Needs org.Hs.eg.db" and offers **Install org.Hs.eg.db…**, which asks
which R library (the rule from 2026-11-19) and installs it, with
AnnotationDbi, as a recorded job. Getting Ready lists them as "for gene
symbols" with the same install.

## How it is built
- A template action in `assets/quick/quick.R`, `symbols`: reads the ID
  column, strips versions, maps with `mapIds(org, keys, "SYMBOL",
  keytype = "ENSEMBL" or "ENTREZID", multiVals = "first")`, adds the
  column (`symbol`, or `symbol_2`... if taken) to the table, and returns
  the counts. It is a new `QuickRequest::Symbols { table, column, ids,
  organism }` and goes into `steps.R` like every run.
- The card table gains `labels: Option<Known<String>>`: the column plots
  label genes by. Templates use it for labels where they use the gene
  column today; matching and the gene column itself stay on the IDs.
- A session reopened later runs the symbols step again after loading the
  table, so the column is there whenever the label fact points at it.
- Organisms: human and mouse now, the ones Sandhill already recognises.
  Others (rat, zebrafish, fly, worm, yeast have org packages too) come
  when Sandhill learns to tell them apart.

## Not doing
- No online lookup (biomaRt, Ensembl's servers): slower, needs a network,
  and fails when Ensembl is busy.
- No symbol to ID conversion, and no ID column replaced in place.

## Settled rules this changes
- `docs/decisions/window.md` (Explore a Table): the card can add a gene
  symbol column to the session's copy of a table; the file stays as it is.

## Checks
- Template tests with real R: Ensembl with and without versions, Entrez,
  mouse, IDs with no symbol, a taken `symbol` column; labels in volcano
  and top genes use symbols.
- App tests: the card offers Add Gene Symbols only for Ensembl or Entrez
  columns with a known organism; the label fact feeds the plot settings;
  reopening replays the step.
- On this Mac: `sandhill-sample-real`'s `gbm_vs_control.csv` gets symbols
  and its volcano shows them.
