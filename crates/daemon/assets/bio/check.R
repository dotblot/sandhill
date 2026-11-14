# Sandhill Seurat checker. Source this file in a private environment, then call
# sandhill_bio_check(request, namespace=.GlobalEnv). No analysis is auto-run.
# Required details:
# qc_metrics: {total_counts:<column>, n_genes:<column>, pct_mito:<column>}
# qc_thresholds: [{metric:"total_counts"|"n_genes"|"pct_mito",min/max,reason}]
# qc_exclusions_file: CSV axis (cell|gene),id,reason; one row per removed ID.
# doublets: {status:"performed"|"not_performed",reason,method (if performed)}.
# Marker CSV: cluster,gene,p_val,p_val_adj,pct_expr,mean_expr.
# Annotation CSV (or JSON array): cluster,label,gene,pct_expr,mean_expr,
# evidence_type (support|conflict),source,status (confirmed|pending|unknown),reason.
# pct_expr is fraction >0 (0..1); mean_expr is arithmetic mean of the explicitly
# selected expression_assay/expression_layer within the indicated cluster.
# Every cluster needs >=2 distinct support genes and conflict rows or an explicit
# no-conflict assessment in reason. These are evidence checks, NOT label approval.
# parameter_evidence: pca/neighbors/clustering/umap objects with command (actual
# Seurat @commands key), parameters (actual @params keys/values), seed. Seeds are
# explicit records; if a command stores its seed it must agree. Missing package
# seed metadata is not fabricated; daemon-validated execution_evidence links the
# analysis code/runs, without claiming a declaration proves every runtime value.
# counts_source requires kind="raw_counts", assay, seurat_layer, provenance,
# processing_state="raw". Source correspondence requires an unchanged raw Seurat
# .rds in source_files, not an arbitrary processed result or unverified declaration.
# No full matrix is copied/densified. All values and comparisons traverse bounded
# blocks; source-backed baselines contain no full cell/gene vectors or counts.
sandhill_bio_check <- local({
  REQUIRED_ITEMS <- list(
    input=c("object", "counts_source", "counts_values", "identifiers", "metadata", "environment", "seed", "processing_state", "mitochondrial_genes", "source_correspondence", "study_design"),
    qc=c("object", "counts_source", "counts_values", "identifiers", "metadata", "environment", "seed", "processing_state", "mitochondrial_genes", "qc_metrics", "qc_thresholds", "qc_exclusions", "retained_counts", "doublets"),
    result=c("object", "counts_source", "counts_values", "identifiers", "metadata", "environment", "seed", "processing_state", "mitochondrial_genes", "result_reload", "result_metadata", "result_embeddings", "execution_provenance", "parameter_evidence", "markers", "annotation_evidence", "human_review")
  )
  function(request, namespace=.GlobalEnv) {
    items <- list()
    facts <- list(checker="sandhill-seurat-v1", coverage="complete bounded-block inspection; never sampled")
    baseline_out <- NULL
    add <- function(id, status, message, evidence=NULL) {
      item <- list(id=id, status=status, message=message)
      if (!is.null(evidence)) item$evidence <- evidence
      items[[length(items)+1L]] <<- item
      invisible(status)
    }
    text <- function(x) is.character(x) && length(x)==1L && !is.na(x) && nzchar(trimws(x))
    strings <- function(x) as.character(unlist(x, use.names=FALSE))
    scalar_number <- function(x) is.numeric(x) && length(x)==1L && is.finite(x)
    int_seed <- function(x) scalar_number(x) && x>=0 && x<=.Machine$integer.max && x==floor(x)
    checkpoint <- if (text(request$checkpoint)) request$checkpoint else "input"
    finish <- function() {
      expected <- REQUIRED_ITEMS[[checkpoint]]
      present <- vapply(items, function(x) x$id, character(1))
      for (id in setdiff(expected, present)) add(id, "not_checked", "Prerequisite missing; this required check was not executed.")
      out <- list(items=items, facts=facts)
      if (!is.null(baseline_out)) out$baseline <- baseline_out
      out
    }
    prerequisite <- function(message) stop(structure(list(message=message,call=NULL),class=c("sandhill_missing","error","condition")))
    attempt <- function(fun) tryCatch(list(ok=TRUE, value=fun()), error=function(e) list(ok=FALSE, status=if(inherits(e,"sandhill_missing")) "not_checked" else "fail", error=substr(conditionMessage(e),1L,600L)))
    propagate <- function(result) { if(identical(result$status,"fail")) stop(result$error) else prerequisite(result$error) }
    # Relative paths are deliberately rejected; no changes to working directory.
    file_info <- function(path, extension=NULL) {
      if (!text(path) || !startsWith(path,"/") || !file.exists(path) || dir.exists(path)) prerequisite("An existing absolute file path is required.")
      if (!is.null(extension) && !grepl(extension,path,ignore.case=TRUE)) stop("Unexpected file extension.")
      if(file.access(path,mode=4L)!=0L) prerequisite("File is not readable in the selected kernel.")
      list(path=normalizePath(path,mustWork=TRUE), md5=unname(tools::md5sum(path)))
    }
    valid_names <- function(x,n) !is.null(x) && length(x)==n && !anyNA(x) && all(nzchar(x)) && !anyDuplicated(x)
    # At most 256 x 256 values per dense block. Sparse blocks stay sparse.
    blocks <- function(n) if(n>0L) seq.int(1L,n,by=256L) else integer()
    values <- function(block) {
      if (inherits(block,"sparseMatrix")) {
        if (!("x" %in% methods::slotNames(block))) return(1)
        return(block@x)
      }
      as.vector(block)
    }
    scan_matrix <- function(m, counts=FALSE, nonnegative=counts) {
      if (!(is.matrix(m) || inherits(m,"sparseMatrix"))) prerequisite("Only in-memory base or Matrix sparse matrices support complete bounded inspection.")
      if(length(dim(m))!=2L || any(dim(m)==0L)) stop("Matrix has an empty or invalid dimension.")
      noninteger <- FALSE
      for (r in blocks(nrow(m))) for (c in blocks(ncol(m))) {
        v <- values(m[r:min(r+255L,nrow(m)),c:min(c+255L,ncol(m)),drop=FALSE])
        if (!is.numeric(v) && !is.logical(v)) stop("Matrix values are not numeric.")
        if (any(!is.finite(v))) stop("Non-finite matrix values found in full scan.")
        if (nonnegative && any(v<0)) stop("Negative values found in a nonnegative counts/expression layer during full scan.")
        if (counts && any(abs(v-round(v))>1e-8)) noninteger <- TRUE
      }
      if(noninteger) stop("Fractional values contradict the declared raw integer-count workflow.")
      TRUE
    }
    compare_matrix <- function(a,b,rows_a=seq_len(nrow(a)),cols_a=seq_len(ncol(a)),rows_b=seq_len(nrow(b)),cols_b=seq_len(ncol(b))) {
      if(length(rows_a)!=length(rows_b) || length(cols_a)!=length(cols_b)) return(FALSE)
      for(r in blocks(length(rows_a))) for(c in blocks(length(cols_a))) {
        rr <- r:min(r+255L,length(rows_a)); cc <- c:min(c+255L,length(cols_a))
        # Only bounded blocks become dense, avoiding sparse representation differences.
        aa <- as.matrix(a[rows_a[rr],cols_a[cc],drop=FALSE])
        bb <- as.matrix(b[rows_b[rr],cols_b[cc],drop=FALSE])
        if(!isTRUE(all.equal(unname(aa),unname(bb),tolerance=0,check.attributes=FALSE))) return(FALSE)
      }
      TRUE
    }
    get_layer <- function(obj,assay,layer) {
      if(!inherits(obj,"Seurat")) stop("Object is not a Seurat object.")
      if(!text(assay) || !text(layer)) stop("Explicit assay and seurat_layer are required.")
      if(!(assay %in% names(obj@assays))) stop("Requested assay is absent.")
      available <- SeuratObject::Layers(obj[[assay]],search=NA)
      if(!(layer %in% available)) stop("Requested exact layer is absent; split layers must be explicitly joined by the analysis.")
      SeuratObject::LayerData(obj,assay=assay,layer=layer,fast=FALSE)
    }
    metrics <- function(m,mito) {
      totals <- numeric(ncol(m)); genes <- numeric(ncol(m)); mt <- numeric(ncol(m))
      for(r in blocks(nrow(m))) for(c in blocks(ncol(m))) {
        rr <- r:min(r+255L,nrow(m)); cc <- c:min(c+255L,ncol(m))
        b <- m[rr,cc,drop=FALSE]
        totals[cc] <- totals[cc]+as.numeric(Matrix::colSums(b))
        genes[cc] <- genes[cc]+as.numeric(Matrix::colSums(b>0))
        sel <- which(rr %in% mito)
        if(length(sel)) mt[cc] <- mt[cc]+as.numeric(Matrix::colSums(b[sel,,drop=FALSE]))
      }
      pct <- rep(NA_real_,length(totals)); good <- totals>0
      pct[good] <- 100*mt[good]/totals[good]
      list(total_counts=totals,n_genes=genes,pct_mito=pct)
    }
    read_table <- function(path,annotation=FALSE) {
      f <- file_info(path)
      if(file.info(f$path)$size>16*1024^2) stop("Evidence table exceeds the 16 MiB bounded evidence limit; no sampled pass is possible.")
      if(annotation && grepl("\\.json$",f$path,ignore.case=TRUE)) {
        tab <- jsonlite::fromJSON(f$path,simplifyDataFrame=TRUE)
        if(!is.data.frame(tab)) stop("Annotation JSON must be an array of row objects.")
      } else tab <- utils::read.csv(f$path,stringsAsFactors=FALSE,check.names=FALSE,colClasses="character")
      if(nrow(tab)>100000L) stop("Evidence table exceeds the row limit; no sampled pass is possible.")
      list(table=tab,file=f)
    }
    study <- function() {
      d <- request$design
      if(!is.list(d) || !text(d$goal) || is.null(d$biological_replicates)) {
        add("study_design","not_checked","Explicit study goal and biological_replicates are required; no inferential design approval.")
      } else add("study_design","warning","Goal/replication are declarations, not verified experimental design. Single-donor tutorials cannot support treatment/disease effects.",list(goal=substr(d$goal,1,500),biological_replicates=d$biological_replicates))
    }
    if(!(checkpoint %in% names(REQUIRED_ITEMS))) {
      add("checkpoint","fail","Unsupported checkpoint; only input, qc and result are implemented.")
      return(finish())
    }
    versions <- list(R=R.version.string)
    missing <- character()
    for(pkg in c("Seurat","SeuratObject","Matrix","jsonlite")) {
      if(!requireNamespace(pkg,quietly=TRUE)) missing <- c(missing,pkg)
      else versions[[pkg]] <- as.character(utils::packageVersion(pkg))
    }
    facts$versions <- versions
    if(length(missing)) {
      add("environment","not_checked","Missing required packages; installation requires separate permission.",list(missing=missing))
      return(finish())
    }
    interfaces <- c("LayerData","Layers","Embeddings")
    absent <- interfaces[!vapply(interfaces,function(n) exists(n,envir=asNamespace("SeuratObject"),inherits=FALSE),logical(1))]
    seurat_interfaces <- c("NormalizeData","FindVariableFeatures","ScaleData","RunPCA","FindNeighbors","FindClusters","RunUMAP","FindAllMarkers")
    absent <- c(absent,seurat_interfaces[!vapply(seurat_interfaces,function(n) exists(n,envir=asNamespace("Seurat"),inherits=FALSE),logical(1))])
    if(length(absent)) {
      add("environment","not_checked","Installed Seurat/SeuratObject lacks required interfaces.",list(missing_interfaces=absent))
      return(finish())
    }
    add("environment","pass","Versions and required interfaces measured in the selected R session.",versions)
    seed <- request$parameters$seed
    rng <- get0(".Random.seed",envir=.GlobalEnv,inherits=FALSE)
    if(int_seed(seed)) add("seed","pass","Explicit analysis seed recorded; current RNG presence/kind measured, not equated to the initial seed.",list(declared_seed=seed,rng_present=!is.null(rng),rng_kind=as.list(RNGkind())))
    else add("seed","not_checked","parameters.seed must explicitly record a nonnegative integer; current RNG state cannot recover the original seed.")
    objname <- request$object
    if(!text(objname) || !grepl("^[A-Za-z.][A-Za-z0-9._]*$",objname) || grepl("^\\.[0-9]",objname) || !is.environment(namespace) || !exists(objname,envir=namespace,inherits=FALSE)) {
      add("object","not_checked","Object must name an existing plain identifier in the supplied live namespace.")
      return(finish())
    }
    if(bindingIsActive(objname,namespace)) {
      add("object","not_checked","Active bindings are not evaluated by the checker.")
      return(finish())
    }
    obj <- get(objname,envir=namespace,inherits=FALSE)
    if(!inherits(obj,"Seurat")) {
      add("object","fail","Named live object is not a Seurat object.")
      return(finish())
    }
    add("object","pass","Live object has Seurat class.")
    cs <- request$counts_source
    assay <- cs$assay; layer <- cs$seurat_layer
    declared <- identical(cs$kind,"raw_counts") && text(cs$provenance) && text(assay) && text(layer)
    if(!declared) add("counts_source","not_checked","Explicit raw_counts kind, provenance, assay and seurat_layer are required; integer appearance is not provenance.")
    got <- attempt(function() get_layer(obj,assay,layer))
    if(!got$ok) {
      add("counts_source","fail",got$error)
      return(finish())
    }
    m <- got$value
    if(declared) add("counts_source","pass","Exact explicitly named counts layer exists; provenance remains a declaration.",list(assay=assay,layer=layer,provenance=substr(cs$provenance,1,1000)))
    good_counts <- attempt(function() scan_matrix(m,TRUE))
    add("counts_values",if(good_counts$ok) "pass" else good_counts$status,if(good_counts$ok) "Every count is finite, nonnegative and integer-valued (full bounded scan)." else good_counts$error)
    ids_ok <- valid_names(rownames(m),nrow(m)) && valid_names(colnames(m),ncol(m))
    gene_type <- request$gene_id_type
    ensembl_ids <- grepl("^ENS[A-Z]*G[0-9]+(\\.[0-9]+)?$",rownames(m))
    if(identical(gene_type,"ensembl") && !all(ensembl_ids)) ids_ok <- FALSE
    if(identical(gene_type,"symbol") && any(ensembl_ids)) ids_ok <- FALSE
    add("identifiers",if(ids_ok) "pass" else "fail","Counts require nonempty unique gene/cell IDs; the declared gene-ID type must agree with observed Ensembl ID structure. Species is a declaration, not proved by a prefix.")
    meta <- obj@meta.data
    meta_ok <- is.data.frame(meta) && identical(rownames(meta),colnames(m)) && identical(colnames(obj),colnames(m))
    add("metadata",if(meta_ok) "pass" else "fail","All live object cells and metadata rows must exactly match counts cell order; partial-assay coverage is unsupported.")
    facts$dimensions <- list(genes=nrow(m),cells=ncol(m))
    facts$counts <- list(assay=assay,layer=layer,layers=as.list(SeuratObject::Layers(obj[[assay]],search=NA)))
    # Counts can coexist with normalized data; only writing a preprocessing output
    # into the selected raw layer is contradictory, not normal Seurat processing.
    raw_state <- identical(cs$processing_state,"raw")
    commands <- obj@commands
    overwritten <- character()
    for(n in names(commands)) {
      p <- commands[[n]]@params
      if(grepl("NormalizeData|ScaleData|SCTransform|LogNormalize",n) &&
         (identical(p$layer,layer) || identical(p$save.layer,layer) || identical(p$new.assay.name,assay))) overwritten <- c(overwritten,n)
    }
    if(length(overwritten) || (raw_state && !good_counts$ok)) add("processing_state","fail","Observed counts or command output contradict the raw-count declaration.",list(commands=as.list(overwritten)))
    else if(!raw_state) add("processing_state","not_checked","counts_source.processing_state='raw' is required; no raw status inferred from values.")
    else add("processing_state","pass","Explicit raw-state declaration has no observed conflicting output command; source correspondence is checked separately.",list(normalized_layer_present="data" %in% SeuratObject::Layers(obj[[assay]],search=NA)))
    species <- request$species; gene_type <- request$gene_id_type
    mito <- integer(); mito_source <- NULL
    mapping <- NULL
    if(!is.null(request$details$mito_gene_ids)) {
      mapping <- list(genes=strings(request$details$mito_gene_ids),source=request$details$mito_source)
    } else if(!is.null(request$parameters$mito_genes)) {
      mapping <- list(genes=strings(request$parameters$mito_genes),source=request$parameters$mito_source)
    } else {
      previous <- request$baseline$mito_mapping
      if(identical(previous$species,species) && identical(previous$gene_id_type,gene_type)) mapping <- previous
    }
    if(!is.null(mapping)) {
      if(text(species) && text(gene_type) && gene_type %in% c("symbol","ensembl") && length(mapping$genes) && text(mapping$source)) {
        mito <- which(rownames(m) %in% strings(mapping$genes))
        mito_source <- substr(mapping$source,1,1000)
      }
    } else if(text(species) && species %in% c("human","mouse") && identical(gene_type,"symbol")) {
      mito <- which(grepl(if(species=="human") "^MT-" else "^mt-",rownames(m)))
      mito_source <- paste(species,"gene-symbol mitochondrial prefix convention")
    }
    if(!length(mito)) add("mitochondrial_genes","not_checked","Species/ID-aware mitochondrial genes could not be established. A missing match is unknown, never 0% mitochondrial counts. Explicit mapping needs details.mito_gene_ids and details.mito_source (or persistent parameters.mito_genes/mito_source).")
    else add("mitochondrial_genes","pass","Mitochondrial genes identified with explicit species/ID context; annotation source is not independently biologically validated.",list(species=species,gene_id_type=gene_type,n_genes=length(mito),source=mito_source))
    if(!ids_ok || !meta_ok || !good_counts$ok) return(finish())
    # A trusted input baseline is anchored to read-only source bytes. Subsequent
    # QC reads these same bytes and compares retained rows/columns exactly.
    source <- NULL
    source_error <- "No existing raw Seurat .rds source file supplied."
    source_error_status <- "not_checked"
    if(checkpoint=="input") {
      for(path in strings(request$source_files)) {
        if(!grepl("\\.rds$",path,ignore.case=TRUE)) next
        candidate <- attempt(function() {
          f <- file_info(path,"\\.rds$")
          s <- readRDS(f$path); sm <- get_layer(s,assay,layer)
          if(!identical(dim(sm),dim(m)) || !identical(rownames(sm),rownames(m)) || !identical(colnames(sm),colnames(m)) || !compare_matrix(sm,m)) stop("Source counts/identities are not identical to the live input.")
          if(!identical(unname(tools::md5sum(f$path)),f$md5)) stop("Source file changed while being read.")
          list(file=f,object=s,counts=sm)
        })
        if(candidate$ok) { source <- candidate$value; break }
        source_error <- candidate$error
        source_error_status <- candidate$status
      }
      if(is.null(source)) add("source_correspondence",source_error_status,source_error)
      else {
        add("source_correspondence","pass","All live input counts and IDs match the reopened raw source; file content digest recorded.",source$file)
        if(raw_state && declared) {
          baseline_out <- list(format="sandhill-seurat-counts-v1",source=source$file,assay=assay,layer=layer,genes=nrow(m),cells=ncol(m),versions=versions)
          if(length(mito)) baseline_out$mito_mapping <- list(genes=as.list(rownames(m)[mito]),source=mito_source,species=species,gene_id_type=gene_type)
        }
      }
      study()
      return(finish())
    }
    if(checkpoint=="qc") {
      b <- request$baseline
      loaded <- attempt(function() {
        if(!identical(b$format,"sandhill-seurat-counts-v1")) prerequisite("A program-measured Seurat input baseline is required.")
        if(!identical(b$assay,assay) || !identical(b$layer,layer)) stop("Counts assay/layer differs from the input baseline.")
        f <- file_info(b$source$path,"\\.rds$")
        if(!identical(f$md5,b$source$md5)) stop("Input source content changed since input checkpoint.")
        s <- readRDS(f$path); sm <- get_layer(s,assay,layer)
        if(nrow(sm)!=b$genes || ncol(sm)!=b$cells || !valid_names(rownames(sm),nrow(sm)) || !valid_names(colnames(sm),ncol(sm))) stop("Baseline source identities/dimensions are invalid.")
        if(!identical(unname(tools::md5sum(f$path)),f$md5)) stop("Source changed during the check.")
        list(file=f,object=s,counts=sm)
      })
      if(loaded$ok) {
        source <- loaded$value; sm <- source$counts
        rr <- match(rownames(m),rownames(sm)); cc <- match(colnames(m),colnames(sm))
        if(anyNA(rr) || anyNA(cc)) add("retained_counts","fail","Filtered cell/gene IDs are not subsets of the measured input source.")
        else add("retained_counts",if(compare_matrix(sm,m,rr,cc,seq_len(nrow(m)),seq_len(ncol(m)))) "pass" else "fail","Every retained raw count compared exactly to the source subset, preserving cell/gene correspondence.",list(before=list(cells=ncol(sm),genes=nrow(sm)),after=facts$dimensions))
      } else add("retained_counts",loaded$status,loaded$error)
      calculated <- if(length(mito)) attempt(function() metrics(m,mito)) else list(ok=FALSE,error="Mitochondrial genes are unresolved; all required QC metrics cannot be checked.")
      if(!calculated$ok) add("qc_metrics","not_checked",calculated$error)
      else {
        qm <- request$details$qc_metrics
        discrepancies <- character(); unavailable <- character()
        for(key in c("total_counts","n_genes","pct_mito")) {
          column <- qm[[key]]
          if(!text(column) || !(column %in% names(meta))) { unavailable <- c(unavailable,key); next }
          saved <- meta[[column]]; expected <- calculated$value[[key]]
          if(!is.numeric(saved) || any(!is.finite(saved)) || any(!is.finite(expected)) || !isTRUE(all.equal(as.numeric(saved),expected,tolerance=1e-7,check.attributes=FALSE))) discrepancies <- c(discrepancies,key)
        }
        if(length(discrepancies)) add("qc_metrics","fail","Saved QC metrics differ from full counts recalculation (zero-library mitochondrial fractions are undefined).",list(metrics=as.list(discrepancies)))
        else if(length(unavailable)) add("qc_metrics","not_checked","Explicit saved QC metadata columns are missing.",list(metrics=as.list(unavailable)))
        else add("qc_metrics","pass","All saved QC metrics match full raw-count recalculation; mitochondrial percentages are 0..100.")
      }
      threshold <- attempt(function() {
        rules <- request$details$qc_thresholds
        if(!is.list(rules) || !length(rules)) prerequisite("qc_thresholds must record metric bounds and a reason for each selected threshold.")
        if(!calculated$ok) prerequisite("Recomputed metrics are unavailable.")
        for(rule in rules) {
          key <- rule$metric
          if(!text(key) || !(key %in% names(calculated$value)) || !text(rule$reason)) prerequisite("Each threshold needs a supported metric and nonempty justification.")
          if(is.null(rule$min) && is.null(rule$max)) prerequisite("Each threshold requires at least one numeric bound.")
          v <- calculated$value[[key]]
          if(any(!is.finite(v))) stop("Undefined QC values cannot satisfy thresholds.")
          if(!is.null(rule$min) && (!scalar_number(rule$min) || any(v<rule$min))) stop(paste("Retained cells violate minimum",key))
          if(!is.null(rule$max) && (!scalar_number(rule$max) || any(v>rule$max))) stop(paste("Retained cells violate maximum",key))
          if(!is.null(rule$min) && !is.null(rule$max) && rule$min>rule$max) stop("Threshold minimum exceeds maximum.")
        }
        TRUE
      })
      add("qc_thresholds",if(threshold$ok) "pass" else threshold$status,if(threshold$ok) "All retained cells satisfy documented thresholds; justification remains a human scientific judgment." else threshold$error)
      exclusion <- attempt(function() {
        if(is.null(source)) prerequisite("The measured input source is required to verify every removed ID.")
        tab <- read_table(request$details$qc_exclusions_file)$table
        if(!all(c("axis","id","reason") %in% names(tab))) stop("Exclusions CSV requires axis,id,reason columns (header-only is valid only when no IDs were removed).")
        if(anyNA(tab[,c("axis","id","reason"),drop=FALSE]) || any(!nzchar(trimws(tab$reason))) || any(!(tab$axis %in% c("cell","gene"))) || anyDuplicated(paste(tab$axis,tab$id,sep="\r"))) stop("Exclusions contain missing/duplicate IDs, unknown axes or empty reasons.")
        removed_cells <- setdiff(colnames(source$counts),colnames(m)); removed_genes <- setdiff(rownames(source$counts),rownames(m))
        if(!setequal(tab$id[tab$axis=="cell"],removed_cells) || !setequal(tab$id[tab$axis=="gene"],removed_genes)) stop("Exclusions do not account exactly for all removed cells and genes.")
        list(cells=length(removed_cells),genes=length(removed_genes))
      })
      add("qc_exclusions",if(exclusion$ok) "pass" else exclusion$status,if(exclusion$ok) "Every removed cell/gene has an explicit exclusion reason; reasons are recorded declarations." else exclusion$error,if(exclusion$ok) exclusion$value else NULL)
      d <- request$details$doublets
      if(is.list(d) && identical(d$status,"performed") && text(d$method) && text(d$reason) && text(d$metadata_column) && d$metadata_column %in% names(meta)) add("doublets","warning","Doublet method and saved classifications exist; their biological validity and removal decisions require human review.",list(method=d$method,column=d$metadata_column))
      else if(is.list(d) && identical(d$status,"not_performed") && text(d$reason)) add("doublets","warning",paste("Doublet assessment explicitly not performed; doublets have NOT been excluded.",substr(d$reason,1,500)))
      else add("doublets","not_checked","No explicit doublet assessment or reasoned omission recorded; doublets are not automatically excluded.")
      return(finish())
    }
    # Result checks reopen the actual deliverable and compare every metadata field
    # and reduction, not just a handful of sampled cells or UMAP dimensions.
    details <- request$details
    reopened <- attempt(function() {
      f <- file_info(details$result_file,"\\.rds$")
      for(input in strings(request$source_files)) if(file.exists(input) && identical(normalizePath(input,mustWork=TRUE),f$path)) stop("Result path aliases an input source.")
      x <- readRDS(f$path); xm <- get_layer(x,assay,layer)
      if(!identical(dim(m),dim(xm)) || !identical(dimnames(m),dimnames(xm)) || !compare_matrix(m,xm)) stop("Reopened result counts/IDs differ from live counts/IDs.")
      if(!identical(unname(tools::md5sum(f$path)),f$md5)) stop("Result file changed during reread.")
      list(object=x,file=f)
    })
    if(!reopened$ok) add("result_reload",reopened$status,reopened$error)
    else {
      add("result_reload","pass","Actual saved .rds reopened; all raw counts and IDs equal the live object.",reopened$value$file)
      saved <- reopened$value$object
      cluster_key <- details$cluster_key
      cluster_valid <- text(cluster_key) && cluster_key %in% names(meta) && !anyNA(meta[[cluster_key]]) && all(nzchar(as.character(meta[[cluster_key]])))
      metadata_equal <- identical(meta,saved@meta.data)
      add("result_metadata",if(metadata_equal && cluster_valid) "pass" else "fail","All reopened metadata fields, types, cell order and cluster assignments must exactly equal the live result; cluster_key must be complete.")
      emb <- attempt(function() {
        key <- details$embedding_key
        if(!text(key) || !(key %in% names(obj@reductions)) || !(key %in% names(saved@reductions))) stop("Explicit embedding_key is absent from live or saved result.")
        if(!setequal(names(obj@reductions),names(saved@reductions))) stop("Saved/live reductions differ.")
        for(red in names(obj@reductions)) {
          a <- SeuratObject::Embeddings(obj[[red]]); b <- SeuratObject::Embeddings(saved[[red]])
          scan_matrix(a); scan_matrix(b)
          if(!identical(rownames(a),colnames(m)) || !identical(dimnames(a),dimnames(b)) || !compare_matrix(a,b)) stop(paste("Nonfinite, misaligned or changed embedding:",red))
        }
        TRUE
      })
      add("result_embeddings",if(emb$ok) "pass" else "fail",if(emb$ok) "Every coordinate of every saved/live reduction is finite, aligned and identical." else emb$error)
    }
    trace <- request$execution_evidence
    trace_ok <- is.list(trace) && length(trace)>0L && all(vapply(trace,function(run) is.list(run) && text(run$run_id) && identical(run$status,"ok") && text(run$code_digest) && text(run$code),logical(1)))
    add("execution_provenance",if(trace_ok) "pass" else "not_checked",if(trace_ok) "Analysis execution IDs/code digests supplied from daemon-validated completed same-session journal runs; this is traceability, not proof of scientific correctness." else "Supply execution_run_ids for completed successful analysis runs; declarations and stored commands alone are not execution traceability.")
    if(trace_ok) facts$analysis_runs <- lapply(trace,function(run) list(run_id=run$run_id,code_digest=run$code_digest))
    param <- attempt(function() {
      required <- list(pca=c("npcs"),neighbors=c("dims","reduction","k.param","nn.method"),clustering=c("resolution","algorithm"),umap=c("dims","reduction","n.neighbors","min.dist"))
      for(stage in names(required)) {
        e <- details$parameter_evidence[[stage]]
        if(!is.list(e) || !text(e$command) || !(e$command %in% names(commands))) prerequisite(paste("Missing actual Seurat command evidence for",stage))
        expected_prefix <- c(pca="RunPCA",neighbors="FindNeighbors",clustering="FindClusters",umap="RunUMAP")[[stage]]
        if(!startsWith(e$command,expected_prefix)) stop(paste("Command does not correspond to",stage))
        actual <- commands[[e$command]]@params
        if(!all(required[[stage]] %in% names(e$parameters))) prerequisite(paste("Incomplete explicit parameter evidence for",stage))
        for(key in names(e$parameters)) {
          wanted <- e$parameters[[key]]; observed <- actual[[key]]
          if(is.list(wanted) && !is.list(observed)) wanted <- unlist(wanted,use.names=FALSE)
          if(!isTRUE(all.equal(wanted,observed,tolerance=0,check.attributes=FALSE))) stop(paste("Declared parameter differs from stored command:",stage,key))
        }
        seed_key <- if(stage=="clustering") "random.seed" else "seed.use"
        if(!int_seed(e$seed)) prerequisite(paste("Explicit recorded seed required for",stage))
        if(!is.null(actual[[seed_key]]) && !identical(as.numeric(e$seed),as.numeric(actual[[seed_key]]))) stop(paste("Contradictory stored stochastic seed for",stage))
      }
      TRUE
    })
    add("parameter_evidence",if(param$ok) "pass" else param$status,if(param$ok) "PCA/neighbors/clustering/UMAP parameters match actual stored commands. Seeds are recorded declarations, checked against stored seeds where available; execution provenance is reported separately." else param$error)
    # Expression summaries are computed gene-by-gene, in bounded cell chunks.
    expression <- attempt(function() {
      if(!text(details$expression_assay) || !text(details$expression_layer)) prerequisite("Explicit marker expression_assay and expression_layer are required.")
      if(identical(details$expression_layer,"scale.data")) prerequisite("Centered/scaled scale.data is not an expression-abundance layer for detection fractions; select raw counts or normalized expression explicitly.")
      ex <- get_layer(obj,details$expression_assay,details$expression_layer)
      if(!identical(colnames(ex),colnames(m)) || !valid_names(rownames(ex),nrow(ex))) stop("Expression layer IDs do not align with result cells.")
      scan_matrix(ex,nonnegative=TRUE)
      if(!reopened$ok) prerequisite("The actual result must reopen before expression evidence can be verified.")
      saved_ex <- get_layer(reopened$value$object,details$expression_assay,details$expression_layer)
      if(!identical(dimnames(ex),dimnames(saved_ex)) || !compare_matrix(ex,saved_ex)) stop("Saved marker expression layer differs from the live layer.")
      if(!text(details$cluster_key) || !(details$cluster_key %in% names(meta))) prerequisite("Explicit cluster_key is missing.")
      cl <- as.character(meta[[details$cluster_key]])
      if(anyNA(cl) || any(!nzchar(cl))) stop("Missing cluster labels.")
      list(matrix=ex,clusters=cl,levels=unique(cl))
    })
    summary_for <- function(gene,cluster) {
      ex <- expression$value$matrix; cells <- which(expression$value$clusters==cluster)
      idx <- match(gene,rownames(ex))
      if(is.na(idx) || !length(cells)) stop("Evidence gene/cluster is absent from the expression layer.")
      total <- 0; positive <- 0
      for(c in blocks(length(cells))) {
        ids <- cells[c:min(c+255L,length(cells))]
        v <- as.numeric(ex[idx,ids,drop=TRUE])
        if(any(!is.finite(v))) stop("Non-finite expression summary values.")
        total <- total+sum(v); positive <- positive+sum(v>0)
      }
      c(pct_expr=positive/length(cells),mean_expr=total/length(cells))
    }
    verify_summaries <- function(tab) {
      for(key in c("cluster","gene","pct_expr","mean_expr")) if(!(key %in% names(tab))) stop(paste("Evidence table missing",key))
      if(!nrow(tab) || anyNA(tab[,c("cluster","gene","pct_expr","mean_expr"),drop=FALSE])) stop("Empty/missing evidence rows.")
      for(i in seq_len(nrow(tab))) {
        pct <- suppressWarnings(as.numeric(tab$pct_expr[i])); avg <- suppressWarnings(as.numeric(tab$mean_expr[i]))
        if(!is.finite(pct) || pct<0 || pct>1 || !is.finite(avg)) stop("Expression summaries must be finite and fractions in 0..1.")
        actual <- summary_for(as.character(tab$gene[i]),as.character(tab$cluster[i]))
        if(abs(pct-actual[["pct_expr"]])>1e-6 || abs(avg-actual[["mean_expr"]])>1e-6*max(1,abs(actual[["mean_expr"]]))) stop(paste("Expression summaries disagree with recomputed layer for row",i))
      }
      TRUE
    }
    markers <- attempt(function() {
      if(!expression$ok) propagate(expression)
      if(!text(details$marker_method) || !text(details$p_adjust_method)) prerequisite("Explicit marker_method and p_adjust_method required; adjusted p-values cannot be inferred from column names alone.")
      if(!(tolower(details$p_adjust_method) %in% c("bonferroni","bh","benjamini-hochberg","holm","hochberg","hommel","by","fdr"))) stop("Unsupported/uncorrected marker multiplicity method.")
      data <- read_table(details$marker_file); tab <- data$table
      if(!("p_val_adj" %in% names(tab))) stop("Marker table lacks adjusted p_val_adj.")
      p <- suppressWarnings(as.numeric(tab$p_val_adj))
      if(!length(p) || any(!is.finite(p)) || any(p<0 | p>1)) stop("Adjusted marker p-values must be finite in [0,1].")
      if(!("p_val" %in% names(tab))) prerequisite("Raw p_val values are required to recompute the declared multiple-testing adjustment.")
      raw_p <- suppressWarnings(as.numeric(tab$p_val))
      if(any(!is.finite(raw_p)) || any(raw_p<0 | raw_p>1)) stop("Raw marker p-values must be finite in [0,1].")
      verify_summaries(tab)
      if(anyDuplicated(paste(tab$cluster,tab$gene,sep="\r"))) stop("Duplicate cluster/gene marker rows.")
      if(!setequal(as.character(tab$cluster),expression$value$levels)) stop("Markers must cover all and only current clusters.")
      adjustment <- tolower(details$p_adjust_method)
      if(adjustment=="benjamini-hochberg") adjustment <- "BH"
      if(adjustment=="bh") adjustment <- "BH"
      if(adjustment=="by") adjustment <- "BY"
      for(cluster in expression$value$levels) {
        idx <- which(as.character(tab$cluster)==cluster)
        if(adjustment=="bonferroni") expected <- pmin(1,raw_p[idx]*nrow(expression$value$matrix))
        else {
          if(!setequal(as.character(tab$gene[idx]),rownames(expression$value$matrix))) prerequisite("Non-Bonferroni adjustment needs the complete per-cluster tested-gene p-value universe, not a filtered marker table.")
          expected <- stats::p.adjust(raw_p[idx],method=adjustment)
        }
        if(!isTRUE(all.equal(p[idx],expected,tolerance=1e-8,check.attributes=FALSE))) stop("Adjusted marker p-values contradict recomputed multiple-testing correction.")
      }
      list(file=data$file,rows=nrow(tab),method=details$marker_method,adjustment=details$p_adjust_method)
    })
    add("markers",if(markers$ok) "pass" else markers$status,if(markers$ok) "Marker adjustment and every cited expression summary recomputed. Test method/source remain declarations; post-clustering p-values are exploratory, not independent validation." else markers$error,if(markers$ok) markers$value else NULL)
    annotation <- attempt(function() {
      if(!expression$ok) propagate(expression)
      data <- read_table(details$annotation_file,TRUE); tab <- data$table
      required <- c("label","evidence_type","source","status","reason")
      if(!all(required %in% names(tab))) stop("Annotation table is missing required evidence/status/source columns.")
      verify_summaries(tab)
      for(k in required) if(anyNA(tab[[k]]) || any(!nzchar(trimws(as.character(tab[[k]]))))) stop(paste("Missing annotation field",k))
      if(any(!(tab$evidence_type %in% c("support","conflict"))) || any(!(tab$status %in% c("confirmed","pending","unknown")))) stop("Unknown annotation evidence type/status.")
      if(!setequal(as.character(tab$cluster),expression$value$levels)) stop("Annotation table must cover exactly all current clusters.")
      context_supported <- identical(request$species,"human") && text(request$parameters$tissue) && tolower(request$parameters$tissue) %in% c("pbmc","blood","peripheral blood")
      for(cluster in expression$value$levels) {
        rows <- tab[as.character(tab$cluster)==cluster,,drop=FALSE]
        if(length(unique(rows$label))!=1L || length(unique(rows$status))!=1L) stop("A cluster has conflicting label/status records; use one pending/unknown label and explicit conflict evidence.")
        support <- rows$gene[rows$evidence_type=="support"]
        if(length(unique(support))<2L) stop(paste("Cluster needs at least two distinct support marker genes:",cluster))
        positive_support <- rows$gene[rows$evidence_type=="support" & as.numeric(rows$pct_expr)>0 & as.numeric(rows$mean_expr)>0]
        if(length(unique(positive_support))<2L && identical(rows$status[1L],"confirmed")) stop("Insufficient positive multi-marker expression requires pending/unknown status, not confirmation.")
        if(!any(rows$evidence_type=="conflict") && !any(grepl("no[ -]conflict|none[ -]detected|未发现冲突",rows$reason,ignore.case=TRUE))) stop("Each cluster needs conflict evidence or an explicit no-conflict assessment reason.")
        if(any(rows$evidence_type=="conflict") && identical(rows$status[1L],"confirmed")) stop("Conflict evidence requires pending/unknown annotation status, not automatic confirmation.")
        if(!context_supported && (!all(rows$status %in% c("pending","unknown")) || !all(tolower(rows$label) %in% c("unknown","pending","待审","未知")))) stop("Unsupported species/tissue requires unknown/pending label and status; PBMC references are not universally applicable.")
        if(identical(rows$status[1L],"unknown") && !(tolower(rows$label[1L]) %in% c("unknown","未知"))) stop("Unknown status must use an unknown label.")
      }
      list(file=data$file,rows=nrow(tab),clusters=length(expression$value$levels),context_supported=context_supported)
    })
    add("annotation_evidence",if(annotation$ok) "pass" else annotation$status,if(annotation$ok) "Multi-marker summaries, cluster coverage, conflicts, sources and review states checked. No marker combination or source string proves biological identity." else annotation$error,if(annotation$ok) annotation$value else NULL)
    unsupported_comparison <- any(vapply(c("disease_comparison","treatment_comparison","group_differential_expression"),function(key) identical(request$design[[key]],TRUE),logical(1)))
    if(unsupported_comparison) add("human_review","fail","Explicit disease/treatment/group differential-expression analysis is outside this exploratory workflow and cannot receive technical approval. Independent design and biological review remain required.")
    else add("human_review","warning","Scientific judgment awaiting independent human review: design, QC tradeoffs, annotation conflicts and conclusions. A confirmed table status is only a submitted assertion; no treatment/disease inference is approved.")
    finish()
  }
})
