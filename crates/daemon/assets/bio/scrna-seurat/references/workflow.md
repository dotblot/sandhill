# Visible Seurat exploratory workflow

Revision 2026-11-10.1; author Sandhill contributors (AI-assisted); **awaiting human domain review; reviewer/date none**. Original example cells, not executed during authoring. Requires actual existing R session/project paths and authorized execution. No installation/download/environment setup is included. Seurat v5 APIs and the official PBMC tutorial inform this example; there is no universal correct cluster numbering or retained cell count.

## 1. Read raw data and preserve a source baseline

Use `Read10X(data.dir=...)` for an already-authorized local 10x matrix or `Read10X_h5` for local H5. Inspect multi-modal returns and deliberately select gene expression; do not automatically pick the first matrix. Start without silent input filtering (`min.cells=0,min.features=0`) so exclusion counts are explicit. Alternatively `readRDS` an existing raw Seurat object after verifying provenance, class, assay/layer and processing history.

```r
library(Seurat)
library(Matrix)
# tenx_dir is the actual absolute local authorized gene-expression directory.
# Deliberately disambiguate duplicate SYMBOL LABELS while preserving original IDs/symbols below.
counts <- Read10X(data.dir = tenx_dir, gene.column = 2, unique.features = TRUE)
if (is.list(counts)) stop('Select the declared Gene Expression matrix explicitly')
if (!inherits(counts, 'sparseMatrix')) stop('Inspect storage; do not densify expression data')
pbmc <- CreateSeuratObject(counts = counts, project = 'exploratory',
                           min.cells = 0, min.features = 0)
# feature_file is the actual absolute 10x genes.tsv/features.tsv (or .gz) input.
feature_map <- read.delim(feature_file, header = FALSE, stringsAsFactors = FALSE)
if (nrow(feature_map) != nrow(pbmc)) stop('Feature metadata/source correspondence unresolved')
pbmc[['RNA']] <- AddMetaData(pbmc[['RNA']], metadata = data.frame(
  gene_id = feature_map[[1]], original_symbol = feature_map[[2]], row.names = rownames(pbmc)))
# Keep this label->gene_id/original_symbol map in the raw snapshot and source_files.
rm(counts)
if (anyDuplicated(Cells(pbmc)) || anyDuplicated(rownames(pbmc))) stop('Resolve source ID correspondence')
# This example assumes verified human gene symbols, not Ensembl IDs.
mito_genes <- grep('^MT-', rownames(pbmc), value = TRUE)
if (!length(mito_genes)) stop('Mitochondrial mapping unknown; do not claim 0 percent')
pbmc[['percent.mt']] <- PercentageFeatureSet(pbmc, features = mito_genes, assay = 'RNA')
print(pbmc)
print(sessionInfo())
# raw_file is a NEW absolute .rds path, distinct from every input and prior output.
if (file.exists(raw_file)) stop('Choose a new raw snapshot path')
saveRDS(pbmc, raw_file)
```

Use original metadata including actual donor/sample/group IDs where available. Do not invent donors from cell barcodes. For Ensembl genes, retain IDs and supply verified mitochondrial IDs through input `details.mito_gene_ids` with nonempty `details.mito_source`; document source/version/mapping ambiguity. This explicit mapping overrides prefix inference and is persisted in the authoritative baseline for QC/result. The source `.rds` snapshot preserves raw counts/IDs before QC; original 10x files remain read-only. Declared raw provenance still needs human review.

```text
guide = get_bio_guide({"id":"scrna-seurat"})
spec = {
  "workflow":"scrna", "guide_id":"scrna-seurat",
  "guide_digest":guide.digest, "checker_digest":guide.checker_digest,
  "goal":goal, "session":session, "object":"pbmc",
  "source_files":[raw_file], "documents":documents,
  "species":"human", "gene_id_type":"symbol",
  "counts_source":{"kind":"raw_counts","processing_state":"raw",
                   "assay":"RNA","seurat_layer":"counts","provenance":raw_count_provenance},
  "design":design, "parameters":{"seed":0}
}
# For these actual human PBMC data, design must explicitly include tissue:'PBMC'.
# Other species/tissues retain unknown/pending labels under this reference.
record = begin_analysis({"project":project,"spec":spec}).analysis
check_analysis({"project":project,"analysis":record.id,"checkpoint":"input","details":{}})
```

`project`, `session`, `documents`, `goal`, `design`, and `raw_count_provenance` are actual project values, not literal strings to copy. `design` describes biological replication and inference limits. Input checks can establish source correspondence, not prove undocumented upstream processing. Do not continue a raw-count workflow after a required input failure.

## 2. Human-reviewed QC

```r
VlnPlot(pbmc, features = c('nCount_RNA', 'nFeature_RNA', 'percent.mt'), ncol = 3)
FeatureScatter(pbmc, feature1 = 'nCount_RNA', feature2 = 'nFeature_RNA')
FeatureScatter(pbmc, feature1 = 'nCount_RNA', feature2 = 'percent.mt')
```

Set `thresholds` to the actual reviewed list of rules `{metric,min,max,reason}` using canonical `total_counts`, `n_genes`, `pct_mito`. Set bounds after examining distributions and sample strata; tutorial 200/2500/5 values are not defaults. Preserve exact exclusions with this visible function:

```r
apply_qc <- function(object, thresholds, exclusion_file) {
  columns <- c(total_counts = 'nCount_RNA', n_genes = 'nFeature_RNA', pct_mito = 'percent.mt')
  metadata <- object[[]]
  keep <- rep(TRUE, ncol(object))
  reasons <- rep('', ncol(object))
  for (rule in thresholds) {
    values <- metadata[[columns[[rule$metric]]]]
    ok <- is.finite(values)
    if (!is.null(rule$min)) ok <- ok & values >= rule$min
    if (!is.null(rule$max)) ok <- ok & values <= rule$max
    reasons[!ok] <- paste(reasons[!ok], rule$reason, sep = '; ')
    keep <- keep & ok
  }
  exclusions <- data.frame(axis = rep('cell', sum(!keep)), id = Cells(object)[!keep],
                            reason = reasons[!keep], stringsAsFactors = FALSE)
  object <- subset(object, cells = Cells(object)[keep])
  counts <- LayerData(object, assay = 'RNA', layer = 'counts')
  keep_gene <- Matrix::rowSums(counts > 0) > 0
  gene_exclusions <- data.frame(axis = rep('gene', sum(!keep_gene)), id = rownames(counts)[!keep_gene],
                                reason = rep('No detected counts after cell QC', sum(!keep_gene)))
  object <- subset(object, features = rownames(counts)[keep_gene])
  counts <- LayerData(object, assay = 'RNA', layer = 'counts')
  object$nCount_RNA <- Matrix::colSums(counts)
  object$nFeature_RNA <- Matrix::colSums(counts > 0)
  mt <- grep('^MT-', rownames(counts), value = TRUE)
  if (!length(mt)) stop('Retained mitochondrial gene mapping unresolved')
  object[['percent.mt']] <- PercentageFeatureSet(object, features = mt, assay = 'RNA')
  if (file.exists(exclusion_file)) stop('Choose a new exclusions output path')
  write.csv(rbind(exclusions, gene_exclusions), exclusion_file, row.names = FALSE)
  object
}
pbmc <- apply_qc(pbmc, thresholds, exclusion_file)
```

```text
qc_details = {
  "qc_metrics":{"total_counts":"nCount_RNA","n_genes":"nFeature_RNA","pct_mito":"percent.mt"},
  "qc_thresholds":thresholds,"qc_exclusions_file":exclusion_file,
  "doublets":{"status":"not_performed","reason":"No doublet method executed; count thresholds do not exclude all doublets."}
}
check_analysis({"project":project,"analysis":record.id,"checkpoint":"qc","details":qc_details})
```

If a doublet method is authorized and run, record its method/evidence rather than declaring unassessed; doublet analysis is not silently installed or substituted. Stop on failed subset/count correspondence. The saved metrics use retained-gene denominators after filtering.

## 3. Normalization and exploratory graph

This recipe uses LogNormalize, not SCTransform. Do not mix assays/layers or treat integrated expression as original counts. Scaling writes a dense **HVG-only derived slice**, never a full dense count/expression conversion. The memory estimate below must fit the actual environment with workspace overhead; if it does not, stop and design an authorized sparse/out-of-core alternative rather than silently densify. Do not run `as.matrix(counts)` or scale all genes.

```r
DefaultAssay(pbmc) <- 'RNA'
pbmc <- NormalizeData(pbmc, assay = 'RNA', normalization.method = 'LogNormalize', scale.factor = 10000)
pbmc <- FindVariableFeatures(pbmc, assay = 'RNA', selection.method = 'vst',
                             nfeatures = min(2000L, nrow(pbmc)))
hvg <- VariableFeatures(pbmc)
print(c(cells = ncol(pbmc), HVGs = length(hvg), scaled_min_bytes = 8 * ncol(pbmc) * length(hvg)))
# Review this estimate against available memory before executing ScaleData.
pbmc <- ScaleData(pbmc, assay = 'RNA', features = hvg, vars.to.regress = NULL)
npcs <- min(30L, length(hvg) - 1L, ncol(pbmc) - 1L)
if (npcs < 2L) stop('Insufficient cells/variable features')
pbmc <- RunPCA(pbmc, assay = 'RNA', features = hvg, npcs = npcs, seed.use = 0)
ElbowPlot(pbmc, ndims = npcs)
pbmc <- FindNeighbors(pbmc, reduction = 'pca', dims = seq_len(npcs),
                       k.param = min(20L, ncol(pbmc) - 1L), nn.method = 'rann')
pbmc <- FindClusters(pbmc, resolution = 0.5, algorithm = 1, random.seed = 0)
pbmc <- RunUMAP(pbmc, reduction = 'pca', dims = seq_len(npcs),
                 n.neighbors = min(30L, ncol(pbmc) - 1L), min.dist = 0.3, seed.use = 0)
DimPlot(pbmc, reduction = 'umap', group.by = 'seurat_clusters', label = TRUE)
```

RANN neighbor search is selected explicitly so the deterministic-neighbor rationale is real; it is not a claim that all graph methods have deterministic seeds. Capture command names with `names(pbmc@commands)` and actual parameters with each command's `@params`, without editing them. The R checker expects `parameter_evidence`:

- `pca`: command key from `RunPCA`, parameters including `npcs`, seed matching actual `seed.use`.
- `neighbors`: command key from `FindNeighbors`, parameters including `dims`, `reduction`, `k.param`, `nn.method`; record the intended seed explicitly and explain that RANN itself is deterministic for this chosen search.
- `clustering`: command key from `FindClusters`, parameters including `resolution`, `algorithm`, seed matching actual `random.seed`.
- `umap`: command key from `RunUMAP`, parameters including `dims`, `reduction`, `n.neighbors`, `min.dist`, seed matching actual `seed.use`.

Each entry is `{command,parameters,seed}`. Use exact emitted key names, not guessed tutorial names; inspected `@commands` and backend-fetched successful runs provide runtime evidence, Agent-written metadata does not. Where a package omits a stored seed, a recorded seed plus actual executed code establishes traceable attribution, not proof of an internal RNG state. Keep code/output visible. A seed does not guarantee bit-identical results across platforms/versions.

## 4. Markers, evidence and save

```r
Idents(pbmc) <- 'seurat_clusters'
markers <- FindAllMarkers(pbmc, assay = 'RNA', slot = 'data', test.use = 'wilcox',
                           only.pos = TRUE, min.pct = 0.1, logfc.threshold = 0.25,
                           return.thresh = 1, densify = FALSE, random.seed = 0)
# Seurat p_val_adj is Bonferroni-adjusted over all genes in the dataset, not BH FDR.
expression_summary <- function(object, cluster, gene) {
  selected <- as.character(object$seurat_clusters) == as.character(cluster)
  expression <- LayerData(object, assay = 'RNA', layer = 'data')
  if (!any(selected) || !gene %in% rownames(expression)) stop('Unknown cluster/gene')
  values <- expression[gene, selected, drop = FALSE]
  c(pct_expr = sum(values > 0) / sum(selected), mean_expr = sum(values) / sum(selected))
}
if (!nrow(markers)) stop('No marker evidence; retain unknown and report missing evidence')
measured <- t(vapply(seq_len(nrow(markers)), function(i)
  expression_summary(pbmc, markers$cluster[i], markers$gene[i]), numeric(2)))
markers$pct_expr <- measured[, 'pct_expr']
markers$mean_expr <- measured[, 'mean_expr']
if (file.exists(marker_file)) stop('Choose a new marker output path')
write.csv(markers[, c('cluster','gene','p_val','p_val_adj','pct_expr','mean_expr')], marker_file, row.names = FALSE)
```

`pct_expr` is a fraction 0–1; `mean_expr` is mean normalized log-expression including zeros, not exponentiated average expression. Preserve full original marker statistics and method/filtering parameters in the analysis document too.

Use the shared PBMC multi-marker reference to examine candidate and conflicting programs with `DotPlot`/`VlnPlot`/`FeaturePlot`. Prepare `annotation_rows` as a data frame with actual review decisions: `cluster,label,gene,evidence_type,source,status,reason`. Include at least two support genes/cluster plus conflict rows, or a genuine explicit assessed-no-conflict reason. Keep label `unknown` and status `pending`/`unknown` when uncertain. A “confirmed” string is a human assertion, not independent program approval. No recipe can responsibly fill this biological table without observing data and reviewing conflicting evidence.

```r
annotations <- annotation_rows
measured <- t(vapply(seq_len(nrow(annotations)), function(i)
  expression_summary(pbmc, annotations$cluster[i], annotations$gene[i]), numeric(2)))
annotations$pct_expr <- measured[, 'pct_expr']
annotations$mean_expr <- measured[, 'mean_expr']
if (file.exists(annotation_file) || file.exists(result_file)) stop('Choose new output filenames')
write.csv(annotations[, c('cluster','label','gene','pct_expr','mean_expr','evidence_type',
                          'source','status','reason')], annotation_file, row.names = FALSE)
saveRDS(pbmc, result_file)
print(sessionInfo())
```

All file variables are actual new absolute paths under the authorized output directory. Check resolved paths/hardlinks/symlinks against inputs as well as names. Arbitrary notebook writes are not sandboxed by managed-export safeguards. Save plots through existing notebook outputs or explicit new plot files; keep QC, PCA diagnostic, UMAP and multi-marker plots.

```text
result_details = {
  "result_file":result_file,"cluster_key":"seurat_clusters","embedding_key":"umap",
  "marker_file":marker_file,"annotation_file":annotation_file,
  "expression_assay":"RNA","expression_layer":"data",
  "marker_method":"wilcox","p_adjust_method":"bonferroni",
  "code_files":documents,"execution_run_ids":execution_run_ids,
  "parameter_evidence":parameter_evidence
}
check_analysis({"project":project,"analysis":record.id,"checkpoint":"result","details":result_details})
get_analysis({"project":project,"analysis":record.id})
```

`execution_run_ids` contains at most 32 actual completed successful analysis runs from this bound session, returned by Sandhill execution tools. Backend fetches actual source/status from the journal; never provide an Agent-written `execution_evidence` object as proof. The raw marker `p_val` column allows recomputing Seurat's Bonferroni `pmin(1,p_val*nrow(RNA))`; adjusted values alone are insufficient to verify correction.

Read the real report, including all failures/not_checked items. Deliver source hashes, versions, exact parameters/seeds, visible code, QC decisions/exclusions, figures, object and evidence tables. Recheck after modifications/restarts. Technical consistency does not validate single-donor treatment claims or biological labels; independent human review remains required.

Sources: official Seurat PBMC3k tutorial https://satijalab.org/seurat/articles/pbmc3k_tutorial (accessed 2026-11-10); API https://satijalab.org/seurat/reference/findallmarkers; SeuratObject layers https://satijalab.github.io/seurat-object/reference/Layers.html. This source-informed example is not an executed compatibility test.
