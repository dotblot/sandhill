---
name: scrna-seurat
description: Perform reproducible exploratory scRNA-seq analysis in an existing R/Seurat session with explicit raw-count assay/layer provenance, checkpoint evidence, and provisional multi-marker annotations.
license: GPL-3.0-or-later
metadata:
  author: Sandhill contributors (AI-assisted original text)
  revision: "2026-11-10.1"
  review-status: awaiting_human_domain_review
  reviewer: none
  review-date: none
---

# R scRNA-seq with Seurat

**Awaiting human domain review. Not a default recommended guide.** No human reviewer/date is recorded. AI self-review or agreement between models does not meet independent biological review.

## Scope and prerequisites

Routine exploratory UMI scRNA-seq from input through QC, normalization, variable features, PCA, graph clustering, UMAP, markers and broad provisional labels. Not a validated treatment/disease differential-analysis, integration, trajectory, spatial or cell-communication workflow. The PBMC marker reference is not transferable automatically to other tissues/species.

Use the explicitly selected existing R session with Seurat/SeuratObject, Matrix and jsonlite available. Record actual versions and required method interfaces; Seurat v5 assay layers differ from v4 slots. Never install, upgrade, download data, switch kernels or build an environment automatically. Use one explicit raw-count assay/layer (`RNA`/`counts` for the example); do not silently join layers or select among multiple assays. Requires unique cell/gene IDs, clear species/ID mapping, read-only inputs and a distinct output path. A saved pre-filter raw `.rds` source with original IDs/metadata/counts is required for source-backed retained-count checks.

## Required questions

Identify goal, source files, prior processing, selected session/object, actual counts assay/layer, species/gene IDs, donor/sample/condition columns, pairing and biological replication, batch structure, QC rationale, mitochondrial mapping, doublet assessment, existing validation and resource constraints. A `counts` label or integer-like matrix alone does not establish raw-count provenance. Inspect existing files before asking again; unresolved evidence stays unresolved.

## Workflow

Use [references/workflow.md](references/workflow.md) for visible R code and tool calls. Retrieve `bio-results-review` resources `references/pbmc-markers.md` and `references/checkpoints.md` for common tables and interpretation limits.

1. Declare raw-count provenance and `processing_state: raw`. Inspect class, assays/layers, sparse count dimensions/identities and metadata. Record study design; single-donor PBMC3k permits descriptive exploration, not treatment/disease inference. Create the analysis with this guide/current guide+checker digests, exact session/object, documents and source `.rds`.
2. Request `input` before filtering. Stop if raw counts are missing, transformed, ambiguous, negative/nonfinite or misaligned. Do not rename normalized data as counts. Compute QC from the declared count layer; identify mitochondrial genes through validated symbols or explicit mapping, never interpret unmatched IDs as zero mitochondrial percentage.
3. Show count/gene/mitochondrial distributions and batch-level plots. Set threshold reasons from the data and study aim, not tutorial constants. Record exact excluded cell/gene IDs and reasons, before/after sizes, and doublet/ambient-RNA limitations. Preserve the counts layer and retained-ID correspondence. Request `qc` before normalization.
4. Use an explicit normalization recipe once. The example uses `NormalizeData`, `FindVariableFeatures` and sparse-aware dimensional reduction; do not mix log normalization and SCTransform/integrated assays accidentally. If scaling is chosen, scale only a bounded HVG subset after checking memory, never the full count matrix or all genes by default. No full `as.matrix()` conversion. Record variable features, PCA, neighbor representation/PCs, clustering method/resolution and UMAP settings/seeds. Do not change global RNG/options invisibly; seed changes in user analysis must be visible and intentional.
5. Set identities from the recorded cluster column. Use normalized RNA `data` for exploratory Wilcoxon markers with documented Bonferroni adjustment; do not relabel adjusted p-values as FDR. Preserve test/correction/filtering details and export independently recomputable expression fractions/means. Multiple-testing correction does not make post-clustering marker tests independent biological confirmation.
6. Evaluate multiple markers and conflicting evidence per cluster. NKG7 is shared by NK/cytotoxic T; FCGR3A also occurs outside monocytes; LYZ is not monocyte-specific. Preserve pending/unknown labels rather than force subtype assignments. Human review owns scientific interpretation.
7. Save new `.rds`, marker/annotation tables, QC and embedding/marker plots, versions (`sessionInfo()`), command parameters/seeds and visible analysis code. Request `result` to reopen and compare the actual saved object; read all required failures/not_checked items and human-review limitations.

## Checkpoints and prohibited claims

`input`, `qc`, `result` execute only through an explicit check in the selected idle session. Ask/Plan can read guides/history, not begin records or execute new checks. A failed/unknown required item is not a technical pass. No single-donor treatment conclusions, pseudoreplication, universal QC thresholds, automatic tutorial cluster labels, UMAP-as-causality, fabricated command logs or independent-review claim. Do not modify `@commands` to satisfy provenance checks. A technical pass cannot prove annotation correctness. Changes to objects/files/runs/kernel versions require fresh checks; old exports remain historical only.

## Sources, revision and authorship

Original Sandhill text, revision **2026-11-10.1**, author **Sandhill contributors (AI-assisted)**; reviewer/date **none; awaiting human domain review**. Official [Seurat PBMC3k tutorial](https://satijalab.org/seurat/articles/pbmc3k_tutorial), [Seurat essential commands](https://satijalab.org/seurat/articles/essential_commands), [FindAllMarkers API](https://satijalab.org/seurat/reference/findallmarkers), [SeuratObject LayerData](https://satijalab.github.io/seurat-object/reference/Layers.html); [Squair et al.](https://doi.org/10.1038/s41467-021-25960-2). Tutorial/marker sources accessed 2026-11-10; examples here have not been executed as part of authoring. No unlicensed skill is copied.
