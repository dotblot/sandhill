# Human PBMC multiple-marker review reference

Revision 2026-11-10.1; original synthesis by Sandhill contributors (AI-assisted). **Awaiting human domain review; reviewer/date none. Not a default recommended reference.** This is a source-linked candidate panel, not a classifier, annotation truth set or proof of cell identity.

## Applicability and limitations

Use only for broad exploratory **human peripheral blood mononuclear cell** annotation with suitable gene-symbol mapping and raw-count/QC provenance. Disease, activation, developmental stage, tissue, species, library chemistry and gene dropout can change expression. Do not automatically apply human PBMC panels to mouse, tumors, tissue macrophages or nuclei. The tutorial PBMC3k dataset comprises 2,700 captured cells from one healthy donor, with no treatment/disease comparison. Retained cell counts and cluster IDs depend on actual analysis choices.

Markers below are candidate evidence for joint review, not mandatory coexpression in every cell. Compare multiple genes, fractions of cells expressing them, mean expression including zeros, within-cluster heterogeneity, and competing lineages. Absence in sparse RNA data is not reliable exclusion; contradictory programs can indicate mixed clusters, doublets or ambient RNA as well as biology. Do not declare a doublet from marker overlap alone. No universal expression/fraction threshold is specified.

| Candidate broad population/program | Concordant candidate genes | Conflicts and caution to actively examine | Source basis |
| --- | --- | --- | --- |
| T lineage | CD3D, CD3E; IL7R/CCR7 may support a naive/helper-like program in context | CD8A alone does not establish all T identities. NKG7/GNLY with CD3D/CD3E can reflect cytotoxic T rather than NK; distinguish lineage from state. No fine subtype claim from this small panel. | S1 marker examples and T marker output; S3 NKG7 expression shared with T cells |
| Cytotoxic program, T versus NK unresolved | NKG7, GNLY; PRF1/KLRD1 are additional context genes | The program is not NK-specific. Review CD3D/CD3E, broad T evidence and cluster heterogeneity. Shared expression must not be hidden as “no conflict.” | S1 NK examples; S3 NKG7/T/NK expression and correlated genes |
| NK-like broad label | GNLY, NKG7 with review of PRF1/KLRD1 and limited coherent T-lineage evidence | NKG7 alone is insufficient. Apparent CD3 absence can be dropout; simultaneous convincing T and NK programs requires pending/unknown, mixture/doublet review or finer analysis. | S1; S3 |
| B lineage | MS4A1, CD79A | Isolated MS4A1 or low-level CD79A is insufficient in a mixed cluster. Review T/myeloid conflicts and dropout; do not claim plasma-cell identity from this panel. | S1 joint MS4A1/CD79A visualization |
| CD14-like monocyte | CD14, LYZ, with CD68 as contextual support | LYZ is not monocyte-specific. Review FCER1A/CST3 dendritic-like evidence and FCGR3A/MS4A7 program; mixed cluster or state may remain unresolved. | S1 CD14/LYZ table and CD68 marker output |
| FCGR3A-like monocyte | FCGR3A, MS4A7, with LYZ/CD68 context | FCGR3A alone does not separate monocytes from NK-like populations. Compare myeloid coherence against GNLY/NKG7 cytotoxic evidence; do not assign all FCGR3A+ cells as monocytes. | S1 monocyte markers; S3 demonstrates cytotoxic lineage context is shared/multigene |
| Dendritic-like population | FCER1A, CST3 | CST3 and myeloid genes are not specific enough alone. Review CD14/LYZ and neighboring myeloid programs; do not infer dendritic subtypes or function without additional sources/markers. | S1 paired DC example |
| Platelet-associated program | PPBP, PF4 | In nucleated immune cells this may reflect platelet RNA contamination, aggregates or doublets, not a new platelet identity. Review cell counts, mixture and expression localization. | S1 PPBP annotation and PF4 plot |

These are **source-informed interpretation cautions**, not claims that the tutorials independently tested every conflict in the table. S1's example cluster labels are specific to its run and must not be transferred by cluster number. S3 explicitly classifies NKG7 expression as shared across several cytotoxic lymphocyte populations; its across-dataset values are not thresholds for this project's normalized layer.

## Evidence table rules

For each actual cluster, retain candidate label, at least two distinct supporting genes, measured `pct_expr` (fraction 0–1) and arithmetic `mean_expr` on the explicitly selected nonnegative normalized expression layer, and conflict genes or a meaningful assessed-no-conflict reason. Include source URL/identifier, `status` (`pending`, `unknown`, or a genuine human-declared `confirmed`) and a narrative reason. The program can verify that reported values match the object and table coverage; it cannot determine whether “support”/“conflict” interpretations are scientifically correct. A `confirmed` string does not create an independent human review record.

Example reasoning without fabricated measurements: “NKG7 and GNLY suggest a cytotoxic program; examine CD3D/CD3E and PRF1/KLRD1 before choosing T versus NK; label pending until those values/heterogeneity and QC are reviewed.” Do not insert arbitrary numeric evidence into the CSV. Missing candidate genes are explicitly missing; preserve unknown rather than silently substituting a single gene or borrowing counts from a different object.

## Sources and verification boundary

All source pages below were accessed 2026-11-10. Original prose/panel synthesis is authored here; no external skill implementation or full source table is vendored.

- **S1:** Seurat official guided PBMC3k tutorial, https://satijalab.org/seurat/articles/pbmc3k_tutorial — input description, example broad markers, CD3D/CD3E marker output, joint MS4A1/CD79A visualization, PF4/PPBP examples and dimensional-reduction limitations. It is a tutorial, not independent biological truth for new projects.
- **S2:** Scanpy official *legacy* PBMC3k tutorial, https://scanpy.readthedocs.io/en/stable/tutorials/basics/clustering-2017.html — healthy-donor data context and reproducible exploratory workflow. Legacy choices do not define universal best practice.
- **S3:** Human Protein Atlas NKG7 single-cell/immune-cell expression, https://www.proteinatlas.org/ENSG00000105374-NKG7/single+cell — NKG7 is enriched across NK and several T-cell populations, with cytotoxic expression-cluster context including GNLY, PRF1 and KLRD1. Atlas normalizations/cell summaries differ from this project's assay.
- **S4:** Squair et al., 2021, https://doi.org/10.1038/s41467-021-25960-2 — biological replication and single-cell differential-expression false-discovery concerns. No disease/treatment inference check is implemented here.

Human reviewer must assess source relevance, panel sufficiency, conflicts and final labels. AI re-reading these references is not that independent review.
