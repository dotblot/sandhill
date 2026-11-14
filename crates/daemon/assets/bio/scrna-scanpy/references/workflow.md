# Visible omicverse-first Python workflow

Revision 2026-11-10.1; Sandhill contributors (AI-assisted); **awaiting human domain review; reviewer/date none**. These are user-visible analysis cells, not code that guide discovery runs. Not executed during authoring. Use actual project paths/session IDs; obtain authorization before executing analyses, creating raw snapshots or downloading anything. No data-download or environment-install call is included.

## 1. Input and record

For an existing raw AnnData `.h5ad`, read it with `anndata.read_h5ad` and inspect its documented counts layer. Do not assume `X` or `.raw` is counts. The example requires a real `counts` layer and human-symbol human PBMC data. For local authorized 10x input, use `scanpy.read_10x_mtx` or `read_10x_h5` in a visible import cell, explicitly declare the source provenance, put the raw sparse matrix in `layers['counts']`, and save a **new** raw `.h5ad` before the input checkpoint. Retain the original 10x files as source files too. This is a local format conversion, not independent proof of upstream provenance. Preserve metadata before writing the raw snapshot; source correspondence compares it with the live object.

10x symbols can be duplicated. If symbols are the chosen labels, import with `make_unique=False`, preserve `adata.var['original_symbol'] = adata.var_names.astype(str)` and the supplied `gene_ids`, then explicitly approve and record `adata.var_names_make_unique()` as **label disambiguation**, not new genes. Keep the mapping in the raw snapshot before baseline creation. Alternatively retain stable gene IDs as the index and use a reviewed symbol mapping for markers/mitochondrial genes. Never silently deduplicate, aggregate or discard features.

```python
from pathlib import Path
from importlib.metadata import version
import anndata as ad
import numpy as np
import pandas as pd
from scipy import sparse
import omicverse as ov
import scanpy as sc

# Use the current project's actual authorized raw_file path, not a tutorial download.
adata = ad.read_h5ad(raw_file)
if 'counts' not in adata.layers:
    raise ValueError('Declare and preserve the actual raw count layer before proceeding')
if not sparse.issparse(adata.layers['counts']):
    raise ValueError('This example requires sparse counts; inspect storage without densifying')
if not adata.obs_names.is_unique or not adata.var_names.is_unique:
    raise ValueError('Resolve identifier correspondence with source; do not silently rename IDs')
versions = {p: version(p) for p in ('omicverse', 'scanpy', 'anndata', 'numpy', 'scipy')}
print(versions)
print(adata.shape, list(adata.layers), list(adata.obs.columns))
```

`raw_file`, `project`, `session`, `documents` and `goal` below are actual values obtained from the current project. `design` contains donor/sample/condition declarations and biological replication; use the study-design guide to draft it. No expression is accepted as the `object`: it is exactly `adata`. Omicverse 2.3.2 is the API target; inspect differences without silently changing packages.

```text
guide = get_bio_guide({"id":"scrna-scanpy"})
spec = {
  "workflow":"scrna", "guide_id":"scrna-scanpy",
  "guide_digest":guide.digest, "checker_digest":guide.checker_digest,
  "goal":goal, "session":session, "object":"adata",
  "source_files":[raw_file], "documents":documents,
  "species":"human", "gene_id_type":"symbol",
  "counts_source":{"kind":"raw_counts","processing_state":"raw","layer":"counts",
                   "provenance":raw_count_provenance},
  "design":design,
  "parameters":{"seed":0}
}
# For these actual human PBMC data, design must explicitly include tissue:'PBMC'.
# Do not add that declaration for other tissues merely to enable named annotations.
record = begin_analysis({"project":project,"spec":spec}).analysis
check_analysis({"project":project,"analysis":record.id,"checkpoint":"input","details":{}})
```

If source counts live in `X`, declare `details.source_counts_layer: "X"` for the input check; do not use this to hide a mismatch. For Ensembl IDs supply actual input `details.mito_gene_ids` with nonempty `details.mito_source` from a verified species-specific mapping and preserve versioned IDs. This mapping overrides prefixes and is persisted in the authoritative baseline for QC/result. Absence of mapping is a stop/unknown condition for mitochondrial QC.

## 2. QC with explicit human decisions

Human-symbol example only; mitochondrial matching must find real genes. Use counts for QC, not normalized `X`. Scanpy's `calculate_qc_metrics` is used specifically for explicit `layer='counts'` and exact checker column semantics; no automatic omicverse filtering/default doublet method is invoked. The original source remains unmodified. Select thresholds after reviewing distributions; tutorial numbers are not general defaults.

```python
adata.var['mt'] = adata.var_names.str.startswith('MT-')
if not adata.var['mt'].any():
    raise ValueError('Mitochondrial mapping is unresolved; do not report 0 percent')
sc.pp.calculate_qc_metrics(adata, qc_vars=['mt'], layer='counts',
                           percent_top=None, log1p=False, inplace=True)
ov.pl.qc(adata, metrics=['total_counts', 'n_genes_by_counts', 'pct_counts_mt'], kind='violin')
# ov.pl.qc is distribution-only; a plain metadata scatter supplies the bivariate diagnostic.
import matplotlib.pyplot as plt
fig, ax = plt.subplots()
points = ax.scatter(adata.obs['total_counts'], adata.obs['n_genes_by_counts'],
                    c=adata.obs['pct_counts_mt'], s=3)
ax.set(xlabel='Total raw counts', ylabel='Detected genes')
fig.colorbar(points, ax=ax, label='Mitochondrial counts (%)')
```

Use a reviewed threshold list with canonical metrics `total_counts`, `n_genes`, `pct_mito`, numeric `min`/`max` bounds and nonempty `reason`. Values are project decisions, not generated to reproduce a tutorial cell count. The function keeps exact exclusion identities and reasons; it performs no gene filtering beyond the explicit zero-detection rule.

```python
def apply_qc(adata, thresholds, exclusion_file):
    cols = {'total_counts':'total_counts', 'n_genes':'n_genes_by_counts',
            'pct_mito':'pct_counts_mt'}
    keep = np.ones(adata.n_obs, dtype=bool)
    rows = []
    reasons = [[] for _ in range(adata.n_obs)]
    for rule in thresholds:
        values = adata.obs[cols[rule['metric']]].to_numpy()
        ok = np.isfinite(values)
        if 'min' in rule:
            ok &= values >= rule['min']
        if 'max' in rule:
            ok &= values <= rule['max']
        for i in np.flatnonzero(~ok):
            reasons[i].append(rule['reason'])
        keep &= ok
    for i in np.flatnonzero(~keep):
        rows.append({'axis':'cell', 'id':str(adata.obs_names[i]), 'reason':'; '.join(reasons[i])})
    adata = adata[keep].copy()
    keep_gene = np.asarray((adata.layers['counts'] > 0).sum(axis=0)).ravel() > 0
    rows.extend({'axis':'gene', 'id':str(g), 'reason':'No detected counts after cell QC'}
                for g in adata.var_names[~keep_gene])
    adata = adata[:, keep_gene].copy()
    # Recompute after gene filtering: denominators now refer to retained genes.
    sc.pp.calculate_qc_metrics(adata, qc_vars=['mt'], layer='counts',
                               percent_top=None, log1p=False, inplace=True)
    pd.DataFrame(rows, columns=['axis','id','reason']).to_csv(exclusion_file, index=False, mode='x')
    return adata

# thresholds are the explicit reviewed rules; exclusion_file is a new absolute CSV path.
adata = apply_qc(adata, thresholds, exclusion_file)
```

```text
qc_details = {
  "qc_metrics":{"total_counts":"total_counts","n_genes":"n_genes_by_counts","pct_mito":"pct_counts_mt"},
  "qc_thresholds":thresholds, "qc_exclusions_file":exclusion_file,
  "doublets":{"status":"not_performed","reason":"No authorized doublet method has been executed; high-count filtering does not exclude all doublets."}
}
check_analysis({"project":project,"analysis":record.id,"checkpoint":"qc","details":qc_details})
```

If a doublet method is actually run, replace the declaration with its method, parameters and evidence; do not mark doublets excluded just to silence a warning. Stop on failed retained-count/source correspondence.

## 3. Normalization, HVGs, PCA and graph

The following visible cells intentionally select CPU mode for sparse-aware behavior. This changes an omicverse package setting explicitly; no hidden checker does so. No default `ov.pp.preprocess`, `ov.pp.scale` or `ov.pp.red` is used because their default layer/subsetting/scaling choices are not this preservation contract. The tagged PCA source accepts `layer`, `mask_var`, `svd_solver`, `zero_center`; `arpack` on CPU avoids full expression densification. The PCA is centered log-expression PCA without per-gene unit-variance scaling; record this choice and evaluate sensitivity if scaling is scientifically important.

```python
ov.settings.mode = 'cpu'
adata.X = adata.layers['counts'].astype(np.float32, copy=True)
ov.pp.normalize_total(adata, target_sum=10000, exclude_highly_expressed=False)
ov.pp.log1p(adata)
adata.layers['lognorm'] = adata.X.copy()
# Scanpy is used here for an explicit log-expression HVG flavor/layer interface.
sc.pp.highly_variable_genes(adata, layer='lognorm', flavor='seurat',
                            n_top_genes=min(2000, adata.n_vars), subset=False)
n_pcs = min(30, int(adata.var['highly_variable'].sum()) - 1, adata.n_obs - 1)
if n_pcs < 2:
    raise ValueError('Insufficient retained variable features/cells for this workflow')
ov.pp.pca(adata, layer='lognorm', n_pcs=n_pcs, mask_var='highly_variable',
          zero_center=True, svd_solver='arpack', random_state=0)
ov.pp.neighbors(adata, use_rep='X_pca', n_pcs=n_pcs,
                n_neighbors=min(15, adata.n_obs - 1), random_state=0)
ov.pp.leiden(adata, resolution=0.5, key_added='leiden', random_state=0,
             flavor='igraph', n_iterations=2, directed=False)
ov.pp.umap(adata, min_dist=0.5, spread=1.0, random_state=0)
ov.pl.umap(adata, color=['leiden', 'total_counts', 'pct_counts_mt'])
```

This explicitly selects the installed igraph Leiden implementation rather than the optional `leidenalg` backend. Record `flavor='igraph'`, `n_iterations=2`, `directed=False`, resolution and seed in visible code/parameter evidence. Require igraph for this choice, not leidenalg; if this selected backend is unavailable, report the prerequisite instead of installing or silently switching algorithms.

These choices are illustrative, not universal optimal parameters. Inspect PCA variance/marker/QC sensitivity and donor/sample mixing before settling them. UMAP coordinates and cluster IDs need not match R or tutorials. Save actual settings, versions and code/output in the project document. Never construct fake `uns` provenance. When a library omits a seed, the checker can attribute a recorded seed to backend-fetched successful executed code via `execution_run_ids`; this does not prove an unrecorded internal RNG state or scientific correctness. Missing trustworthy evidence remains `not_checked`.

## 4. Markers and annotation evidence

```python
sc.tl.rank_genes_groups(adata, groupby='leiden', layer='lognorm', use_raw=False,
                        method='wilcoxon', corr_method='benjamini-hochberg',
                        tie_correct=True, n_genes=adata.n_vars, key_added='markers')
markers = sc.get.rank_genes_groups_df(adata, group=None, key='markers')
markers = markers.rename(columns={'group':'cluster','names':'gene','pvals':'p_val','pvals_adj':'p_val_adj'})
# Retain the full per-cluster test family in adata.uns['markers'] for BH recomputation.
markers = markers.groupby('cluster', observed=True).head(50).copy()

def expression_summary(adata, cluster, gene, cluster_key='leiden', layer='lognorm'):
    mask = adata.obs[cluster_key].astype(str).to_numpy() == str(cluster)
    if not mask.any() or gene not in adata.var_names:
        raise ValueError('Unknown cluster or gene in evidence')
    column = adata.layers[layer][mask, adata.var_names.get_loc(gene)]
    return float((column > 0).mean()), float(column.mean())

summary = [expression_summary(adata, r.cluster, r.gene) for r in markers.itertuples()]
markers['pct_expr'] = [v[0] for v in summary]
markers['mean_expr'] = [v[1] for v in summary]
markers[['cluster','gene','p_val','p_val_adj','pct_expr','mean_expr']].to_csv(marker_file, index=False, mode='x')
```

Fractions are 0–1, means are arithmetic means of log-expression including zero cells, not exp-transformed means. Retain original statistics and correction method in the visible document; do not call BH-adjusted values independent post-clustering validation.

Read the PBMC marker reference. Review dot/violin plots of several candidate lineages, not just the top significant gene. Prepare `annotation_rows` as actual scientific review decisions, one row per evidence gene, with `cluster,label,gene,evidence_type,source,status,reason`. Each cluster needs at least two support genes and conflict evidence or an explicit assessed-no-conflict reason. Use `label: unknown`, `status: pending` or `unknown` when uncertain; never generate `confirmed` labels from this example. Support/conflict are human-declared interpretations, not program-measured truth. Missing genes cannot be invented. Fill measured fields from the object:

```python
annotations = pd.DataFrame(annotation_rows)
values = [expression_summary(adata, r.cluster, r.gene) for r in annotations.itertuples()]
annotations['pct_expr'] = [v[0] for v in values]
annotations['mean_expr'] = [v[1] for v in values]
annotations[['cluster','label','gene','pct_expr','mean_expr','evidence_type',
             'source','status','reason']].to_csv(annotation_file, index=False, mode='x')
if Path(result_file).exists():
    raise FileExistsError('Choose a new result filename; never overwrite the input or prior output')
adata.write_h5ad(result_file)
```

All output variables are actual new absolute paths under the selected output directory. Refuse aliases/symlinks/hardlinks to inputs as well as name equality; backend managed-output protection does not sandbox arbitrary notebook writes.

## 5. Result request and handoff

```text
result_details = {
  "result_file":result_file,"cluster_key":"leiden","embedding_key":"X_umap",
  "marker_file":marker_file,"annotation_file":annotation_file,"expression_layer":"lognorm",
  "marker_method":"wilcoxon","p_adjust_method":"benjamini-hochberg","marker_uns_key":"markers",
  "code_files":documents,"execution_run_ids":execution_run_ids,
  "parameter_evidence":parameter_evidence
}
check_analysis({"project":project,"analysis":record.id,"checkpoint":"result","details":result_details})
get_analysis({"project":project,"analysis":record.id})
```

`parameter_evidence` maps `pca`, `neighbors`, `clustering`, `umap` to `{uns_key,parameters,seed}`; use actual keys `pca`, `neighbors`, `leiden`, `umap` and actual parameter names/values emitted by those calls. Never add `uns` fields to make them match. `execution_run_ids` is a list of at most 32 actual successful completed analysis runs in the bound session, obtained from Sandhill execution results. The backend fetches their authoritative code/status; never send an Agent-authored `execution_evidence` object as proof. See the shared checkpoint reference for limitations. Deliver visible code/figures, result object, QC exclusions, marker/annotation tables and the current program report. Required `not_checked` is not pass; human review remains outstanding after technical success.

Sources: official omicverse v2.3.2 `_normalization.py`, `_preprocess.py`, `_pca.py` at https://github.com/omicverse/omicverse/tree/v2.3.2/omicverse/pp (accessed 2026-11-10); Scanpy marker API https://scanpy.readthedocs.io/en/stable/generated/scanpy.tl.rank_genes_groups.html. APIs were source-inspected, not runtime-verified here.

Plot API sources inspected at the same official tag: https://github.com/omicverse/omicverse/blob/v2.3.2/omicverse/pl/_qc.py (`ov.pl.qc` accepts metrics/kind/batch_key) and https://github.com/omicverse/omicverse/blob/v2.3.2/omicverse/pl/_single.py (`ov.pl.umap` and explicit `ov.pl.embedding`). `ov.pl.umap` creates an `obsm['UMAP']` alias from `X_umap`; include it in saved-object correspondence rather than hide the change. For marker expression plots prefer `ov.pl.embedding(adata,basis='X_umap',color=genes,layer='lognorm',use_raw=False)` and `ov.pl.violin`/`ov.pl.dotplot` when their installed interfaces fit.
