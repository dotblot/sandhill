# ComBat-ref, Xiaoyu Zhang.
# Upstream: https://github.com/xiaoyu12/Combat-ref/tree/f0c6d3f313e9f1f29de2c8c8f12b98113443be55
# Combat_ref.R and its two used functions from helper_seq.R. Sandhill adaptations below.
#
# MIT License
#
# Copyright (c) 2026 Xiaoyu Zhang
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all
# copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
# SOFTWARE.

combat_ref <- local({
  # Correct batch effects for RNA-seq count data using a negative binomial distribution
  # Adjust count towards the reference batch with minimum estimated dispersion

  #' @param counts Raw count matrix from genomic studies (dimensions gene x sample)
  #' @param batch Batch covariate (only one batch allowed)
  #' @param mod The biological design matrix to preserve (with intercept).
  #' @param genewise.disp Compute dispersion parameter for each gene = FALSE
  #' @return data A probe x sample count matrix, adjusted for batch effects.
  #'
  #' Sandhill passes the full model matrix instead of a single group factor.
  #' @export
  #'

  ComBat_ref <- function(counts, batch, mod, genewise.disp=FALSE)
  {
    ########  Preparation  ########
    counts <- as.matrix(counts)

    ## Does not support 1 sample per batch yet
    batch <- as.factor(batch)
    if(any(table(batch)<=1)){
      stop("ComBat-seq doesn't support 1 sample per batch yet")
    }

    ## Remove genes with only 0 counts in any batch
    keep_lst <- lapply(levels(batch), function(b){
      which(rowSums(counts[, batch==b, drop=FALSE]) > 0)
    })
    keep <- Reduce(intersect, keep_lst)
    if(!length(keep)) stop("ComBat-ref needs at least one gene expressed in every batch.", call.=FALSE)
    rm <- setdiff(1:nrow(counts), keep)
    countsOri <- counts
    counts <- counts[keep, , drop=FALSE]

    # require bioconductor 3.7, edgeR 3.22.1
    dge_obj <- edgeR::DGEList(counts=counts)

    ## Prepare characteristics on batches
    n_batch <- nlevels(batch)  # number of batches
    batches_ind <- lapply(1:n_batch, function(i){which(batch==levels(batch)[i])}) # list of samples in each batch
    n_batches <- sapply(batches_ind, length)
    cat("Found",n_batch,'batches\n')

    ## Helper: build design matrix from current batch factor
    build_design <- function(batch, mod) {
      batchmod <- model.matrix(~batch)
      # Keep the batch offsets first for gamma_hat; mod supplies the intercept.
      design <- cbind(batchmod[, -1L, drop=FALSE], mod)
      list(design=design, batchmod=batchmod)
    }

    dm <- build_design(batch, mod)
    design <- dm$design
    batchmod <- dm$batchmod

    if(qr(design)$rank<ncol(design)) stop("Batch and the comparison cannot be told apart.", call.=FALSE)

    cat("Estimating dispersions\n")
    ## Estimate common dispersion within each batch
    disp_common <- sapply(1:n_batch, function(i){
      if((n_batches[i] <= ncol(mod)) | qr(mod[batches_ind[[i]], , drop=FALSE])$rank < ncol(mod)){
        # not enough residual degree of freedom
        return(edgeR::estimateGLMCommonDisp(counts[, batches_ind[[i]], drop=FALSE], design=NULL, subset=nrow(counts)))
      }else{
        return(edgeR::estimateGLMCommonDisp(counts[, batches_ind[[i]], drop=FALSE], design=mod[batches_ind[[i]], , drop=FALSE], subset=nrow(counts)))
      }
    })
    for(i in 1:n_batch) {
      cat("Batch", levels(batch)[i], "dispersion =", disp_common[i], "\n")
    }

    # Choose the batch with the smallest dispersion as the reference batch
    ref_batch <- which.min(disp_common)
    cat("Reference batch: ", levels(batch)[ref_batch], "\n")

    # Reorder factor levels so the reference batch becomes the first level
    if(ref_batch != 1) {
      # Swap dispersion values
      disp_common[c(1, ref_batch)] <- disp_common[c(ref_batch, 1)]
      # Reorder factor levels (not rename) so ref_batch level comes first
      new_order <- seq_along(levels(batch))
      new_order[c(1, ref_batch)] <- new_order[c(ref_batch, 1)]
      batch <- factor(batch, levels = levels(batch)[new_order])
      # Rebuild batch indices and design matrix
      batches_ind <- lapply(1:n_batch, function(i){which(batch==levels(batch)[i])})
      n_batches <- sapply(batches_ind, length)
      dm <- build_design(batch, mod)
      design <- dm$design
      batchmod <- dm$batchmod
    }

    if(genewise.disp) {
      genewise_disp_lst <- lapply(1:n_batch, function(j){
        if((n_batches[j] <= ncol(mod)) | qr(mod[batches_ind[[j]], , drop=FALSE])$rank < ncol(mod)){
          # not enough residual degrees of freedom - use the common dispersion
          return(rep(disp_common[j], nrow(counts)))
        }else{
          return(edgeR::estimateGLMTagwiseDisp(counts[, batches_ind[[j]], drop=FALSE], design=mod[batches_ind[[j]], , drop=FALSE],
                                             dispersion=disp_common[j], prior.df=0))
        }
      })
    } else {
      genewise_disp_lst <- lapply(1:n_batch, function(j) {
        return(rep(disp_common[j], nrow(counts)))
      })
    }

    names(genewise_disp_lst) <- paste0('batch', levels(batch))
    ## construct dispersion matrix
    phi_matrix <- matrix(NA, nrow=nrow(counts), ncol=ncol(counts))
    for(k in 1:n_batch){
      phi_matrix[, batches_ind[[k]]] <- vec2mat(genewise_disp_lst[[k]], n_batches[k])
    }
    ########  Estimate parameters from NB GLM  ########
    cat("Fitting the GLM model\n")
    glm_f <- edgeR::glmFit(dge_obj, design=design, dispersion=phi_matrix, prior.count=1e-4)

    gamma_hat <- as.matrix(glm_f$coefficients[, 1:(n_batch-1)])
    mu_hat <- glm_f$fitted.values
    phi_hat <- do.call(cbind, genewise_disp_lst)

    ########  Adjust the data  ########
    cat("Adjusting the data\n")
    adjust_counts <- counts
    # adjust batches except for the first one (reference batch)
    for(kk in 2:n_batch) {
      counts_sub <- counts[, batches_ind[[kk]], drop=FALSE]
      old_mu <- pmax(mu_hat[, batches_ind[[kk]], drop=FALSE], 1e-4)      #numerical stability
      old_phi <- phi_hat[, kk]
      new_mu <- exp(log(old_mu)-vec2mat(gamma_hat[, kk-1], n_batches[kk]))
      # avoid exploding count (mu), new_mu shouldn't increase to more than the ref batch if gamma_hat < 0
      increased_genes <- which(gamma_hat[, kk-1] < -0.2)
      n_cols <- ncol(new_mu)
      ref_max <- vec2mat(matrixStats::rowMaxs(mu_hat[, batches_ind[[1]], drop=FALSE]), n_cols)
      new_mu[increased_genes, ] <- pmin(new_mu[increased_genes, , drop=FALSE],
                                       ref_max[increased_genes, , drop=FALSE])
      new_phi <- phi_hat[, 1]
      adjust_counts[, batches_ind[[kk]]] <- match_quantiles(counts_sub=counts_sub,
                                                            old_mu=old_mu, old_phi=old_phi,
                                                            new_mu=new_mu, new_phi=new_phi, keep_zero=FALSE)
    }

    ## Add back genes with only 0 counts in any batch (so that dimensions won't change)
    adjust_counts_whole <- matrix(NA, nrow=nrow(countsOri), ncol=ncol(countsOri))
    dimnames(adjust_counts_whole) <- dimnames(countsOri)
    adjust_counts_whole[keep, ] <- adjust_counts
    adjust_counts_whole[rm, ] <- countsOri[rm, ]
    attr(adjust_counts_whole, "reference_batch") <- levels(batch)[1L]
    return(adjust_counts_whole)
  }
  ####  Expand a vector into matrix (columns as the original vector)
  vec2mat <- function(vec, n_times){
    return(matrix(rep(vec, n_times), ncol=n_times, byrow=FALSE))
  }
  ####  Match quantiles (vectorized)
  # keep_zero: zero values in the original counts don't change
  match_quantiles <- function(counts_sub, old_mu, old_phi, new_mu, new_phi, keep_zero=TRUE){
    nr <- nrow(counts_sub)
    nc <- ncol(counts_sub)
    # Expand per-gene phi vectors into matrices matching counts_sub dimensions
    old_phi_mat <- vec2mat(old_phi, nc)
    new_phi_mat <- vec2mat(new_phi, nc)

    if(keep_zero) {
      # Compute CDF for counts-1 (shift for keep_zero mode)
      tmp_p <- pnbinom(counts_sub - 1, mu=old_mu, size=1/old_phi_mat)
      # Map through new distribution, adding 1 back
      new_counts_sub <- 1 + qnbinom(tmp_p, mu=new_mu, size=1/new_phi_mat)
      # Keep original values where count <= 1 or CDF is essentially 1 (outliers)
      keep_mask <- (counts_sub <= 1) | (abs(tmp_p - 1) < 1e-4)
      new_counts_sub[keep_mask] <- counts_sub[keep_mask]
    } else {
      tmp_p <- pnbinom(counts_sub, mu=old_mu, size=1/old_phi_mat)
      new_counts_sub <- qnbinom(tmp_p, mu=new_mu, size=1/new_phi_mat)
      # Keep original for outlier counts where CDF is essentially 1
      outlier_mask <- abs(tmp_p - 1) < 1e-6
      new_counts_sub[outlier_mask] <- counts_sub[outlier_mask]
    }

    return(new_counts_sub)
  }
  ComBat_ref
})
