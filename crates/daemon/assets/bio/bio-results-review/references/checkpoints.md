# Analysis/checkpoint contract and evidence limits

Revision 2026-11-10.1; author Sandhill contributors (AI-assisted). **Awaiting human domain review; reviewer/date none.** Original operational guidance. Sources: Sandhill's approved bioinformatics-assistance plan; official omicverse 2.3.2 https://github.com/omicverse/omicverse/tree/v2.3.2; Seurat https://satijalab.org/seurat/articles/pbmc3k_tutorial; Scanpy markers https://scanpy.readthedocs.io/en/stable/generated/scanpy.tl.rank_genes_groups.html. No source endorses a scientific-correctness claim.

## Discovery and permissions

`get_bio_guide({})` returns `{guides:[...],recommended_guide:null}`. Each catalog entry includes ID, description, version, guide digest, checker digest/language where applicable, resource names, `review_status: awaiting_human_domain_review`, `recommended:false`, author and null human reviewer/date. `get_bio_guide({id})` adds main `SKILL.md` content; `{id,resource}` reads exactly one listed resource. It never reads arbitrary filesystem paths, starts a kernel, installs software or downloads data.

IDs/resources:

- `bio-study-design`: `SKILL.md`, `references/study-design.md`, `references/checkpoints.md`; checker `design`.
- `scrna-scanpy`: `SKILL.md`, `references/workflow.md`, `references/pbmc-markers.md`, `references/checkpoints.md`; checker `python` (omicverse first).
- `scrna-seurat`: `SKILL.md`, `references/workflow.md`, `references/pbmc-markers.md`, `references/checkpoints.md`; checker `r`.
- `bio-results-review`: `SKILL.md`, `references/pbmc-markers.md`, `references/checkpoints.md`; review companion only, no runnable checker and no valid `begin_analysis` workflow mapping.

The shared PBMC/checkpoint resources are embedded from a single original asset but exposed under each applicable guide's exact resource allowlist. Their content therefore participates in each applicable guide digest; changing the shared annotation/checkpoint contract invalidates old workflow digests too. Read them through discovery, not arbitrary relative filesystem traversal.

Guide digest is `sha256:` plus lowercase hexadecimal. Hash input: domain bytes `sandhill-bio-guide-v1` followed by a zero byte; guide ID; then every listed resource name and content, in fixed catalog order, main skill first. Each ID/name/content field is independently prefixed with its UTF-8 byte length as an unsigned 64-bit big-endian integer. Reference content is included: editing a reference invalidates the old digest. Bounds: 128 KiB per resource and 512 KiB per guide; only compile-time embedded allowlisted names are accepted. Traversal, absolute paths, unlisted resources and resource-without-ID are rejected. Package checker digest is separate, obtained from current discovery; never invent it or reuse a stale value.

Reading guides/history is permitted in Ask/Plan; `begin_analysis` writes records and `check_analysis` executes checks/writes reports, so neither is an unconditional read-only operation. A missing old-server capability or stale digest requires an explicit unavailable/restart response, not a synthetic pass. Guide access records only provision of content, not guaranteed model compliance. General Agent shell/tools are not an OS sandbox.

## Begin request

Call `begin_analysis({project,spec})`; project is the actual absolute project root. `spec` requires:

- `workflow`: `study_design` or `scrna`.
- `guide_id`, current `guide_digest`, current `checker_digest`, nonempty `goal`.
- `source_files`: actual absolute read-only input file paths; `documents`: actual absolute code/narrative document paths.
- `design`: declared study/replication facts, not measured evidence; `parameters`: recorded intended choices.
- Optional `output_dir`: explicit absolute output directory; otherwise results use project `results/<analysis-id>/`.
- For `scrna`: actual existing `session`, plain-identifier `object`, `species`, `gene_id_type`, and `counts_source`.

Python counts source: `{kind:'raw_counts',processing_state:'raw',layer:'counts',provenance:nonempty_declaration}`. R: `{kind:'raw_counts',processing_state:'raw',assay:'RNA',seurat_layer:'counts',provenance:nonempty_declaration}`. Object/layer/assay names are explicit, not executable expressions. Input integer-likeness is a consistency check, not upstream provenance proof. Python/R examples record `parameters.seed: 0`; method-specific actual evidence belongs in result details and visible code.

Workflow/guide mapping is strict: `study_design` → `bio-study-design`; Python `scrna` → `scrna-scanpy`; R `scrna` → `scrna-seurat`. A record binds the selected actual session/object, not a guessed substitute environment. See the study-design reference for complete design keys and language references for visible code. `begin_analysis` returns `{analysis:{id,...},latest_check,...}`: the **nested `response.analysis.id`**, not a top-level `response.id`, is the `analysis` argument for later calls. The examples bind `record = begin_analysis(...).analysis` before using `record.id`.

For the human PBMC annotation reference, explicitly declare actual `design.tissue` (`PBMC`, `blood`, or `peripheral blood`, case-insensitive) and `species: human`. Confident named PBMC annotations are unsupported outside that context; retain unknown/pending rather than change a declaration to bypass the boundary. This context declaration is not measured proof of tissue identity. Treatment/disease/group differential-expression claims remain out of scope even within human PBMC.

## Check request and baseline

`check_analysis({project,analysis,checkpoint,details})` accepts `design` for design records; `input`, `qc`, `result` for scRNA. `get_analysis({project,analysis})` reads authoritative history without rerunning checks. Backend supplies fixed checker scripts with authoritative spec and prior measured baseline; the Agent cannot submit a pass JSON as evidence.

Checkpoint `details` are retained as declared inputs in history, separate from measured checker facts. The private measured baseline is journal-owned and not exposed by public responses. The backend supplies the top-level declared `spec.goal` to the checker; callers need not duplicate it under design.

`design` details `{}` checks the declared design stored in the journal, with no kernel. Required keys: `research_question,evidence,hypothesis,experimental_unit,groups,pairing,confounders,primary_outcome,analysis_method,sample_size_rationale,validation,stopping_rules,sources,unresolved_questions,biological_replicates`. Sources need URL/DOI/PMID. Placeholders do not establish completeness; format-valid identifiers do not verify truth/support/novelty. Human design/statistical review remains pending.

`input` measures identities/counts properties, metadata alignment, source hashes, package versions/required interfaces and raw-count declaration consistency. Use an already-authorized pre-filter `.h5ad`/`.rds` source with counts/IDs preserved. Source-backed baseline enables sparse/blockwise retained-cell/gene count correspondence without duplicating a full count matrix in the journal. Files remain read-only; no reliable supported source → correspondence is not checked, never fabricated. Python `details.source_counts_layer` selects source `X` or actual source layer if it differs from the live declared count layer. Explicit `details.mito_gene_ids` is required where symbol matching is inappropriate; unmapped mitochondria is unknown, not 0%.

For both languages, explicit mitochondrial mapping includes `details.mito_gene_ids` and nonempty `details.mito_source` at input (or `parameters.mito_genes`/`mito_source`). It overrides symbol-prefix inference and is persisted in the authoritative baseline for QC/result, including correspondence after mapped genes are filtered out. A gene list without mapping provenance is insufficient.

## QC details (both languages)

```json
{
  "qc_metrics": {"total_counts":"total_counts", "n_genes":"n_genes_by_counts", "pct_mito":"pct_counts_mt"},
  "qc_thresholds": [
    {"metric":"n_genes", "min":200, "reason":"Illustrative only: use the real reviewed lower bound and its rationale"}
  ],
  "qc_exclusions_file":"/absolute/new-output/qc-exclusions.csv",
  "doublets":{"status":"not_performed", "reason":"No doublet method has been executed; not equivalent to absence of doublets"}
}
```

The number in this format example is **not a default recommendation**; replace with the actual reviewed rules. Bounds are inclusive in the visible examples. Canonical metric keys are `total_counts`, `n_genes`, `pct_mito`; values map to saved metadata. For R map them to `nCount_RNA,nFeature_RNA,percent.mt`. Mitochondrial values use **percentage 0–100**, unlike expression fractions below. Exclusion CSV header is `axis,id,reason`, axis exactly `cell` or `gene`, one row per excluded ID with actual nonempty reason. Header-only CSV is appropriate only when nothing was excluded. Keep exact sets, not only aggregate counts. QC must recompute from retained counts and verify source subsets/counts; excluded-count reasons and selected thresholds are declarations for human review. Do not label an unperformed doublet assessment complete.

`qc_thresholds` may cover a scientifically justified subset of these metrics; it need not impose arbitrary cutoffs on all three. All three QC metrics must still be measured and correctly mapped. The selected normalized-expression layer is checked for negative values across its contents regardless of its name; naming a scaled matrix `data` does not make it valid marker expression.

## Result details and real execution evidence

Both languages require actual saved `result_file` (`.h5ad`/`.rds`), `cluster_key`, `embedding_key` (`X_umap`/`umap` examples), `marker_file`, `annotation_file`, declared marker method/correction, actual `code_files`, `parameter_evidence`, and `execution_run_ids`. Python additionally `expression_layer` (`lognorm`), `marker_uns_key` (`markers` in the example); R `expression_assay: RNA`, `expression_layer: data`.

`execution_run_ids` is a list of at most 32 **actual completed successful analysis execution IDs** from the record's same session. Backend independently retrieves source, code digest, timestamps and status from its journal. Do not send a caller-authored `execution_evidence` object as proof; that reserved evidence is constructed by the backend. A recorded seed plus actual executed source is honest attribution even when a package omits stored seed metadata, not proof of every internal RNG draw. Emitted metadata values must agree; missing verifiable evidence stays unverified. Later object mutations/runs require fresh checking.

Python `parameter_evidence` maps `pca,neighbors,clustering,umap` to entries `{uns_key,parameters,seed}`. Example actual keys are `pca,neighbors,leiden,umap`. Record actual method choices (PCA components/layer/HVG/solver, neighbors representation/PCs/k, clustering resolution/algorithm, UMAP PCs/graph/min_dist/spread) and explicit seed. Compare emitted `uns[key]['params']` where available and executed code, not fabricated metadata. R entries are `{command,parameters,seed}` referencing actual `@commands` keys; capture npcs, dims/reduction/k.param/nn.method, resolution/algorithm, n.neighbors/min.dist and appropriate seed fields. Do not rewrite command history.

Saved-object reopening compares dimensions/IDs/counts, all relevant metadata, clusters and embeddings/reductions with the live object; checking a different unsaved object is insufficient. Finite low-dimensional embeddings do not validate their biological interpretation. Save plot outputs and actual environment versions alongside visible code.

For final results, backend reruns QC using the prior authoritative QC details and input baseline; required `final_qc_*` items therefore detect count/metric changes after the earlier QC checkpoint. A prior QC pass alone cannot cover later mutations. Execution IDs must identify actual completed successful **non-tool analysis runs**, not the checking tool's own executions.

### Marker CSV

Common columns: `cluster,gene,p_val_adj,pct_expr,mean_expr`; **include raw `p_val` too** (required R correction verification, useful Python). Fractions are 0–1 detection on the declared nonnegative expression layer; means are arithmetic expression means across **all cluster cells including zeros**, not exponentiated/conditional means. R's Seurat `pct.1`/average-fold-change summaries must not be silently substituted for this defined `mean_expr`.

Python keeps the full actual `rank_genes_groups` raw/adjusted p-value family in the result object's `uns[marker_uns_key]` for independent BH correction recomputation, with `use_raw=False` and explicit layer. CSV can retain a top subset only if the full tested family remains available. R verifies Seurat Bonferroni from raw `p_val` and actual number of genes in the expression assay; other corrections require their full test family, not a truncated significant-only list. Declared correction is not measured correction until checked. Gene/cluster references, reported fractions and means are independently recomputed against saved/live expression.

### Annotation CSV/JSON rows

`cluster,label,gene,pct_expr,mean_expr,evidence_type,source,status,reason`.

- `evidence_type`: `support` or `conflict`.
- `status`: `confirmed`, `pending`, `unknown`. “Confirmed” is a human-declared interpretation, not a program-created independent-review approval.
- At least two distinct support genes per cluster; include conflicting genes or an explicit meaningful assessed-no-conflict reason. Each row references a real gene and real cluster and nonempty source/reason.
- Every cluster must be covered; unknown/pending is appropriate when evidence is insufficient. The checker verifies structure and expression summaries, not that the biological choice of support/conflict/label is correct. Do not invent marker measurements or silently reuse another cluster/object.

Retrieve `references/pbmc-markers.md` for relevant candidate panels/conflicts. Missing tissue/species coverage remains unsupported; adding two gene names does not establish correctness.

## Meaning of a report

Per-item statuses: `pass`, `warning`, `fail`, `not_checked`. Required missing/unmeasured items cannot yield technical pass. Nonperformed doublet assessment and awaiting independent human review remain explicitly visible, never “excluded” or “reviewed.” Journal/checker evidence is authoritative; user-editable managed exports are historical/readable material, not importable approval. Each check has its own ID and actual run where a kernel was used; design checks do not fabricate run IDs. Atomic export failure must be reported, not disguised as saved success.

Checks are snapshot-specific: input/file/code/result digests, selected session state, guide/checker versions and history matter. Later execution, file edits, restarts or removed history can invalidate freshness/traceability. Recheck before final delivery and disclose any required unknowns. Managed output protections reject input aliases and existing-output overwrite; they do not sandbox arbitrary external Agent/notebook tools. Human approval is still needed for study design, QC tradeoffs, annotation and final claims. No total scientific-correctness or “100% accuracy” badge is justified.
