---
name: bio-results-review
description: Review biological study designs and exploratory scRNA-seq outputs by separating successful execution, measured technical checks, statistical appropriateness, and independent human biological interpretation.
license: GPL-3.0-or-later
metadata:
  author: Sandhill contributors (AI-assisted original text)
  revision: "2026-11-10.1"
  review-status: awaiting_human_domain_review
  reviewer: none
  review-date: none
---

# Biological results review

**Awaiting human domain review. Not a default recommended guide.** No human reviewer/date is recorded. This AI-assisted guide and AI agreement do not constitute independent domain review. This is a review companion, not a separate runnable analysis workflow or valid `begin_analysis` guide.

## Scope and prerequisites

Use to review the study-design and routine exploratory Python/R scRNA workflows. Requires the actual question/design, input provenance, visible code and outputs, saved result object, current journal-backed checkpoint record and scientific evidence. Missing materials remain missing; do not infer successful execution from a polished report. There is no special-check pass for treatment/disease differential expression, bulk/spatial analyses, trajectory or cell communication.

## Questions before accepting a result

What claim is being made, and what independent experimental units support it? Which exact source/object/file/code/package version was checked, when, and through which run? Did later code execute, the object/file change, or the kernel restart? Which required items failed or were not checked? Which choices are human declarations rather than measured facts? What annotations conflict, what genes were unavailable, and who reviewed QC/biology? Does the proposed inference exceed the design?

## Review workflow

1. **Execution:** Inspect real recorded code and outputs. A command finishing, an embedding rendering or a file existing proves neither intended method nor biological correctness. Missing packages are explicit prerequisites, not invitations to install automatically. Ask/Plan may read guides/history, not create records or run checks.
2. **Technical evidence:** Read `get_analysis`, not an Agent-written export as authoritative truth. Check `input`, `qc`, `result` or `design` as appropriate using [references/checkpoints.md](references/checkpoints.md). Bind claims to source hashes, guide/checker digests, actual session/run and saved result. Reject stale evidence; recheck after changes. Busy/unsupported/disconnected environments do not equal pass.
3. **Statistical applicability:** Confirm donor/sample units, replication/pairing, confounding, hypothesis/outcome, multiplicity and uncertainty. Cells nested in one donor are not independent donor replicates. Post-clustering cell markers are exploratory, not an independently validated disease effect. No single-donor PBMC3k treatment/disease claim is permitted. No universal sample-size or QC threshold follows from a tutorial.
4. **QC:** Review distributions and reasons, exact exclusions, retained raw-count correspondence and mitochondrial mapping. Missing doublet/ambient-RNA assessment stays visible. A filtering rule can remove real biology; program consistency cannot approve the choice.
5. **Biology:** Review multiple concordant markers, expression fractions/means including zeros, source relevance and competing explanations for each cluster. Read [references/pbmc-markers.md](references/pbmc-markers.md). NKG7/FCGR3A/LYZ each have nonunique interpretations; include conflict evidence. Dropout/ambient RNA/doublets/mixed clusters can explain weak or mixed programs. Keep unknown/pending for inadequate evidence; no cluster-number lookup.
6. **Reproducibility:** Verify visible code, versions, actual parameters/seeds, input provenance/hashes, exclusion tables, plots, normalized-expression layer, adjusted marker table, annotation evidence, distinct `.h5ad`/`.rds` result and current program report. Do not densify full matrices, copy them into prompts or overwrite inputs to simplify inspection. There is no automatic download/environment step.
7. **Human handoff:** List technical facts, failures/not_checked, scientific limitations and decisions requiring Huan/designated domain reviewer. Record actual reviewer/date/scope only when a human supplies them. Distinguish guide approval from approval of this project's design, QC, annotations and conclusions.

## Checkpoints and acceptance language

Statuses are per item: `pass`, `warning`, `fail`, `not_checked`. Required unmeasured items cannot become pass by declaration; human review can remain `not_checked` even when technical items pass. Use “technical consistency checked for this snapshot; biological interpretation awaits human review,” not “scientifically correct,” “100% accurate” or “independently validated.” Format-correct DOI/URL is not source verification; listing two markers is not correct cell typing. A deterministic check of the same data is not an independent biological replicate.

## Sources, revision and authorship

Original Sandhill text, revision **2026-11-10.1**, author **Sandhill contributors (AI-assisted)**; reviewer/date **none; awaiting human domain review**. Sources: [Seurat PBMC tutorial](https://satijalab.org/seurat/articles/pbmc3k_tutorial); [Scanpy marker API and replication warning](https://scanpy.readthedocs.io/en/stable/generated/scanpy.tl.rank_genes_groups.html); [Squair et al., 2021](https://doi.org/10.1038/s41467-021-25960-2); [Human Protein Atlas NKG7](https://www.proteinatlas.org/ENSG00000105374-NKG7/single+cell). Official tutorial/API/atlas pages accessed 2026-11-10; no copying of third-party skill text. These references do not independently validate the current project.
