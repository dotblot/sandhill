---
name: scrna-scanpy
description: Perform reproducible exploratory scRNA-seq analysis in a selected Python kernel, prioritizing omicverse 2.3.2 and using Scanpy only for explicitly needed interfaces, with preserved raw counts and human-reviewed annotations.
license: GPL-3.0-or-later
metadata:
  author: Sandhill contributors (AI-assisted original text)
  revision: "2026-11-10.1"
  review-status: awaiting_human_domain_review
  reviewer: none
  review-date: none
---

# Python scRNA-seq: omicverse first

**Awaiting human domain review. Not a default recommended guide.** AI review is not independent biological validation. The stable discovery ID remains `scrna-scanpy`; it does not mean Scanpy takes priority over omicverse.

## Scope and prerequisites

Routine exploratory UMI scRNA-seq: input, QC, normalization, variable features, PCA, graph clustering, UMAP, markers and provisional broad-lineage annotation. Prioritize **omicverse 2.3.2** for supported steps; use Scanpy for explicit matrix-layer QC/HVG/marker interfaces where necessary. No bulk/spatial/trajectory/cell-communication or disease differential-analysis pass is provided. PBMC references apply only to suitable human PBMC data, not arbitrary tissue/species.

Use the user's selected existing Python session and live AnnData object named by a plain identifier. Confirm available omicverse, Scanpy, AnnData, SciPy, NumPy, graph/UMAP dependencies and actual versions in that kernel; missing software requires an explicit installation decision, never automatic environment creation/upgrades/downloads. Requires authorized read-only source data, unique cell/gene IDs, species/ID type, explicit raw-count provenance, output space and a visible code document. A source `.h5ad` with preserved original metadata/counts is needed for source-backed retained-count checks. Importing 10x is supported in visible analysis code, but save a distinct raw `.h5ad` baseline before filtering; do not overwrite inputs.

The reference explicitly selects `ov.pp.leiden(..., flavor='igraph', n_iterations=2, directed=False)` with installed igraph. The separate optional leidenalg package is not required for this selected backend. Record backend and iteration/direction choices; do not install a missing alternative or silently change the algorithm.

## Required questions

What is the descriptive question? Which file/device/session/object? Which counts layer, and how do we know it is raw? Has it been normalized/logged? What species, ID mapping, chemistry, samples, donors, conditions and biological replication? What QC decisions are justified by distributions? Is mitochondrial mapping known? What doublet/ambient-RNA assessment is feasible? What output and independent validation are available? Inspect project facts first; do not infer provenance from integer-looking values.

## Workflow

Read [references/workflow.md](references/workflow.md) for visible code and exact checkpoint calls. Retrieve `bio-results-review` resources `references/pbmc-markers.md` and `references/checkpoints.md` for evidence tables and the shared contract.

1. Record study design, inputs/processing history and limitations; PBMC3k is one healthy donor, not treatment evidence. Inspect source, dimensions, IDs, sparse counts, metadata and actual versions. Declare `counts_source.kind: raw_counts`, `processing_state: raw`, layer and provenance. Keep counts unchanged; `.raw` is not automatically raw counts.
2. Create the analysis with this guide/current digests, actual session, object and absolute document/input paths. Request `input` before filtering. If raw counts are missing/contradictory or matrix/metadata mapping fails, stop this workflow. Technical failure cannot be fixed by relabeling normalized values as counts.
3. Calculate cell/gene QC from counts with explicit mitochondrial genes. Unknown mapping is unknown, not 0% mitochondrial contamination. Show QC distributions/scatter plots, assess batch effects and doublets, choose sample-appropriate thresholds with a reason. Preserve exact excluded IDs/reasons; filtered cells/genes must be source subsets. Request `qc` after filtering and before normalization. A missing doublet assessment remains unassessed.
4. Normalize/log-transform once using omicverse's explicit calls. Retain full retained-gene counts and log-expression for markers; mark HVGs rather than discarding all other genes. Use a sparse-aware centered PCA path, then omicverse neighbors/Leiden/UMAP with explicit representation, parameters and seeds. Never materialize the full expression matrix with `.toarray()`, `.A`, `.to_df()`, or dense all-gene scaling. Dense low-dimensional embeddings are expected; assess memory for feature slices before any intentional bounded transform.
5. Use an explicit normalized expression layer, not counts/scaled/PCA/integrated values, for exploratory markers. Specify test and correction; export adjusted p-values and independently reproducible per-cluster fractions/means. Cluster-discovery and marker tests on the same cells are not independent confirmation.
6. Compare multiple concordant lineage markers and conflicts. NKG7 alone cannot distinguish NK from cytotoxic T; LYZ/FCGR3A alone cannot separate all myeloid/NK states. Keep unknown/pending labels when conflicted. Save evidence for every cluster, including contradictory markers, missing genes and source limits. Human review owns the biological label.
7. Save a new `.h5ad`, marker and annotation tables, QC/UMAP/marker figures, visible code, versions, parameters, seeds and rationale. Request `result`; inspect the actual program report and required failures/not_checked items. Reopened-object consistency is required, not just an in-memory success.

## Checkpoints and prohibited claims

`input`, `qc`, `result` are explicit checks in the selected idle session; reading a guide does not execute them. Ask/Plan may only read guides/history. Keep scientific judgment pending after technical pass. No single-donor disease/treatment inference, cell-as-donor pseudoreplication, UMAP-distance-as-lineage proof, universal QC cutoffs, automatic cluster-number labels, unsupported accuracy claim or fabricated runtime provenance. Do not write fake `uns` metadata to satisfy checks. Missing recorded runtime evidence must remain `not_checked`. Later runs/files/kernel changes require fresh checks; exports are not authoritative pass evidence.

## Sources, revision and authorship

Original text, **2026-11-10.1**, **Sandhill contributors (AI-assisted)**; human reviewer/date **none; awaiting human domain review**. APIs checked against official tagged [omicverse 2.3.2 preprocessing source](https://github.com/omicverse/omicverse/blob/v2.3.2/omicverse/pp/_preprocess.py), [normalization source](https://github.com/omicverse/omicverse/blob/v2.3.2/omicverse/pp/_normalization.py) and [PCA source](https://github.com/omicverse/omicverse/blob/v2.3.2/omicverse/pp/_pca.py), accessed 2026-11-10. Also [Scanpy marker API](https://scanpy.readthedocs.io/en/stable/generated/scanpy.tl.rank_genes_groups.html), [Scanpy legacy PBMC tutorial](https://scanpy.readthedocs.io/en/stable/tutorials/basics/clustering-2017.html), [Squair et al.](https://doi.org/10.1038/s41467-021-25960-2). Source inspection is not an executed validation of these examples. No third-party skill text is copied.
