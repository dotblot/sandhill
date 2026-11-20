# Explore a Table: Find Changed Genes, designs, batch effects, compressed files

Huan, 2026-11-19. Status: approved by Huan (2026-11-19) with these
choices: batch in the model, SVA, RUVSeq, ComBat-seq and ComBat-ref (not
random effects); archives as well as gzipped tables; and, his addition,
designs with two factors of two or more levels, ready-made formulas to
choose from and the user's own formula (section 1.1, confirmed the same
day, with the dose trend).

Huan asked (2026-11-19) for differential expression in Explore a Table with
a choice of method (DESeq2, edgeR, limma) and of batch handling (in the
design formula, SVA, ComBat-seq, ComBat-ref, limma's
`removeBatchEffect()`), and for compressed tables to open. This is phase 3
of `docs/design/quick-tools-plan.md` (which had DESeq2 only), widened.

## 1. Find Changed Genes
From a raw counts table with sample factors on the card (typed, or read
from a sample sheet) and what to compare.

### 1.1 Sample factors, and which question to ask
The card's groups become **sample factors**: every column of a sample
sheet besides the sample names (for example `genotype`, `dose`, `batch`,
`donor`) is offered as a factor, each with its levels in an order the user
can change (the first is the reference: wild type, dose 0). Typed groups
are one factor. Plots colour by the factor chosen.

Most people don't know which formula to write, so Find Changed Genes asks
**what do you want to find?** and offers ready-made designs, each showing
the formula and test it runs and one line on when to use it. For one
factor (say `condition`):

| Question | Formula and test |
|---|---|
| Compare two groups (B vs A) | `~ condition`, Wald test of B vs A |
| Each group against the reference | `~ condition`, one Wald test per level against the first; one results table each |
| Any difference among 3 or more groups | `~ condition` against `~ 1`, likelihood ratio test (LRT; F-test in edgeR and limma) |

For two factors (say `genotype`: WT, KO and `dose`: 0, 1, 10):

| Question | Formula and test |
|---|---|
| The drug's effect within each genotype (recommended start) | `~ group`, where group is genotype and dose together (WT_0, WT_1...); Wald tests of each dose against dose 0, separately in WT and in KO |
| The genotype's effect at each dose | `~ group`; Wald tests of KO against WT at each dose |
| The genotype's effect, the same at every dose | `~ dose + genotype`, Wald test of KO vs WT (assumes the genotype changes genes the same way at every dose) |
| Does the drug act differently in KO than in WT? | `~ genotype + dose + genotype:dose`; with two doses a Wald test of the interaction; with three or more an LRT against `~ genotype + dose` |
| Genes that respond to the drug at all, in either genotype | LRT of `~ genotype + dose + genotype:dose` against `~ genotype` |
| Genes that rise or fall with dose (offered when a factor's levels are all numbers) | `~ genotype + dose`, with dose as a number; Wald test of the dose slope (log2 fold change per unit of dose) |

When to use an LRT, in the help beside the choices: a Wald test compares
exactly two things (KO vs WT, dose 10 vs dose 0) and gives a fold change
for them. An LRT asks whether a whole set of terms matters at once (any
of several doses, an interaction with three or more levels, a time
course) and gives one p-value per gene; the fold change shown is for one
comparison the user picks.

Batch, donors or pairs, when chosen (section 1.2), are added as a term
first (`~ batch + ...`), and to the reduced formula of an LRT too.

**Write my own formula**, for those who know: a design formula
(`~ batch + genotype * dose`), then either a Wald test of a contrast
(a factor and two of its levels, or a coefficient by name, listed from
the design) or an LRT with a reduced formula. Sandhill checks the formula
in R before running (every term is a factor on the card, the design has
full rank, no group without samples) and says what is wrong in plain
words.

Each comparison makes its own results table, named for it.

### 1.2 Method and batch

**Method** (one choice, DESeq2 first):

| Method | What runs |
|---|---|
| DESeq2 | `DESeq()` then `results()` for the comparison, with `lfcShrink(type = "apeglm")` offered as a tick for the fold changes shown |
| edgeR | `filterByExpr`, `normLibSizes`, `glmQLFit`, `glmQLFTest` (the quasi-likelihood pipeline edgeR's authors recommend) |
| limma | `voom` (or `voomWithQualityWeights`, a tick), `lmFit`, `eBayes`, `topTable` |

Each gives the same results table shape (gene, average expression, log2
fold change, p-value, adjusted p-value, plus the method's own columns), so
every Quick Plot works on it, and normalized values (DESeq2's
`vst`, edgeR's and limma's log2 CPM) for PCA and heatmaps.

**Batch** (one choice, "None" first). The batch is one of the sample
factors (section 1.1):

| Choice | What it does | Notes shown |
|---|---|---|
| None | `~ group` | |
| Batch in the model (recommended) | `~ batch + group` for the chosen method | The usual, statistically sound way |
| Pairs or donors in the model | `~ donor + group` | For before/after or matched samples |
| Unknown batches: SVA | `sva::svaseq` estimates hidden factors, added to the model | When batches weren't recorded |
| Unknown batches: RUVSeq | `RUVSeq::RUVg` with empirical control genes (least changed in a first pass), factors added to the model | Alternative to SVA |
| ComBat-seq | `sva::ComBat_seq` corrects the counts, then the method runs on them | Changes the counts; the model choice is usually preferred |
| ComBat-ref | ComBat-seq with the batch of least dispersion as the reference, which is kept as is | Not on CRAN or Bioconductor: its MIT-licensed R code is copied into Sandhill's templates with its notice |

`limma::removeBatchEffect()` is not offered for testing: limma's own help
says it is for plots and clustering, not for the data a test is run on.
It is used, when a batch is set, for the batch-corrected values that PCA
and heatmaps draw, and the plot says so.

Other approaches people use, not built now:
- Random effects for donors or batches: limma's `duplicateCorrelation`,
  or `dream` from variancePartition. Worth adding for repeated measures.
- `variancePartition` to measure how much batch explains, and PCA coloured
  by batch, as a check before choosing (the PCA plot gets a "colour by
  batch" choice now).
- ComBat (not -seq), for log-scale or microarray data, and Harmony, fastMNN
  and others, which are for single-cell data.

**Checks before it runs, in plain words:** whole-number counts only
("These look normalized: DESeq2, edgeR and limma-voom need raw counts");
at least two samples in each group being compared; a batch that isn't the
same as the groups (confounded: "Every control is batch 1, so batch and
group can't be told apart"); at least two samples per batch for
ComBat-seq/ComBat-ref.

**Results:** a new table on the card, `<counts>_<method>_<higher>_vs_<lower>`,
recognised as results with the comparison already filled in, shown in the
Data Explorer, and the Make Plots choices for it. The sentence says the
method, the design and how many genes changed. Gene symbols carry over.
`steps.R` records it, and the methods text names the method, design,
batch handling and package versions.

**Packages:** DESeq2, edgeR, limma, apeglm, sva and RUVSeq install on
demand from Getting Ready, as the gene symbol packages do (one row, "For
finding changed genes", asking which R library).

## 2. Compressed files
- Gzipped tables (`.csv.gz`, `.tsv.gz`, `.txt.gz`, `.tab.gz`, as GEO
  gives them) open like their uncompressed forms: dropped, chosen,
  imported as a sample sheet, or added to a session. R reads them
  directly; nothing is unpacked on disk.
- Archives (`.zip`, `.tar`, `.tar.gz`, `.tgz`, like GEO's `_RAW.tar`):
  opening one lists the tables inside (gzipped ones too) with ticks; the
  ticked ones are copied out into the session folder and opened. The
  archive itself is never changed.
- When the ticked files are one per sample, each a gene column and a
  count column (as in many `_RAW.tar` files), Sandhill offers **Combine
  into One Counts Table**: one column per sample, named from the file
  names, genes matched by ID, and says how many genes every file had.

## Settled rules this changes
- `docs/decisions/window.md`: Find Changed Genes, its methods and batch
  choices; gzipped tables.
- `docs/decisions/licences.md`: ComBat-ref's R code (MIT, xiaoyu12/Combat-ref)
  is copied into the templates with its licence notice.

## Checks
- Template tests with real R on `sandhill-sample-real`'s GSE159851 counts:
  each method gives a results table whose top genes agree with running the
  package directly; batch choices change the design as described; the
  refusals above.
- A gzipped GEO table loads and is recognised the same as unzipped.
- App tests: the method and batch choices reach the request; the new
  results table appears on the card with its comparison.
