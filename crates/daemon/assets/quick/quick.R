sandhill_quick <- local({
  fail <- function(message) stop(message, call. = FALSE)

  need_packages <- function(packages) {
    missing <- packages[!vapply(packages, requireNamespace, logical(1), quietly = TRUE)]
    if (length(missing)) fail(paste0("Missing R packages: ", paste(missing, collapse = ", "), "."))
  }

  fix_names <- function(nms) {
    empty <- which(!nzchar(nms) | is.na(nms))
    if (length(empty)) {
      nms[empty] <- paste0("column_", empty)
      nms[empty[1L]] <- "id"
    }
    make.unique(nms)
  }

  text_patterns <- function(x) {
    x <- trimws(x)
    matches <- function(pattern, ignore.case = FALSE) grepl(pattern, x, perl = TRUE, ignore.case = ignore.case)
    human <- sum(matches("^ENSG[0-9]+(?:\\.[0-9]+)?$"))
    mouse <- sum(matches("^ENSMUSG[0-9]+(?:\\.[0-9]+)?$"))
    ensembl <- sum(matches("^ENS[A-Z]*G[0-9]+(?:\\.[0-9]+)?$"))
    month <- paste0("(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|",
                    "Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)")
    day <- "(?:0?[1-9]|[12][0-9]|3[01])"
    datelike <- sum(matches(paste0("^(?:", day, "-", month, "|", month, "-", day, ")$"), TRUE))
    serials <- sum(matches("^(?:[3-5][0-9]{4}|60000)$"))
    if (serials && sum(matches("^[+-]?(?:[0-9]+(?:\\.[0-9]*)?|\\.[0-9]+)(?:[eE][+-]?[0-9]+)?$")) < length(x) / 2) {
      datelike <- datelike + serials
    }
    ratio <- matches("^[0-9]+\\s*/\\s*[0-9]+$")
    gene <- "[A-Za-z0-9][A-Za-z0-9_.-]*"
    list(ensembl_human = human, ensembl_mouse = mouse, ensembl_other = ensembl - human - mouse,
         entrez = sum(matches("^[0-9]+$")), symbol = sum(matches("^[A-Za-z][A-Za-z0-9._-]*$")),
         go = sum(matches("^GO:[0-9]{7}$")),
         pathway = sum(matches("^(?:[a-z]{2,4}[0-9]{5}|R-[A-Z]{3}-[0-9]+|KEGG:[A-Za-z]*[0-9]+|REAC:R-[A-Z]{3}-[0-9]+|WP[0-9]+)$")),
         datelike = datelike,
         gene_lists = sum(!ratio & matches(paste0("^", gene, "(?:\\s*[/,;]\\s*", gene, ")+$"))),
         ratio = sum(ratio))
  }

  describe <- function(name, x) {
    kind <- if (inherits(x, c("Date", "POSIXt"))) "date" else if (is.numeric(x)) "number" else
      if (is.logical(x)) "logical" else "text"
    absent <- is.na(x)
    if (kind == "text") absent <- absent | !nzchar(x)
    present <- x[!absent]
    distinct <- unique(present)
    result <- list(name = name, kind = kind, missing = sum(absent), distinct = length(distinct),
                   examples = I(as.character(utils::head(distinct, 8L))))
    if (kind == "number") {
      finite <- present[is.finite(present)]
      result$whole <- all(finite == trunc(finite))
      if (length(finite)) {
        result$min <- min(finite)
        result$max <- max(finite)
      }
    }
    if (kind == "text") result$patterns <- text_patterns(present)
    result
  }

  load_table <- function(request) {
    file <- request$file
    extension <- tolower(tools::file_ext(file))
    if (extension %in% c("xlsx", "xls")) {
      need_packages("readxl")
      tab <- tryCatch(readxl::read_excel(file, sheet = request$sheet, .name_repair = "minimal"),
                      error = function(e) fail("Could not read the selected sheet."))
      tab <- as.data.frame(tab, check.names = FALSE, stringsAsFactors = FALSE)
    } else {
      tab <- tryCatch({
        sep <- switch(extension, csv = ",", tsv = "\t", tab = "\t", {
          line <- readLines(file, n = 1L, warn = FALSE)
          count <- function(separator) nchar(line) - nchar(gsub(separator, "", line, fixed = TRUE))
          if (length(line) && count("\t") > count(",")) "\t" else ","
        })
        table <- utils::read.table(file, header = TRUE, sep = sep, quote = '"', comment.char = "",
                                   na.strings = c("NA", ""), row.names = NULL, check.names = FALSE,
                                   stringsAsFactors = FALSE)
        # read.table uses this name for an extra leading field when the header is short.
        if (identical(names(table)[1L], "row.names")) {
          header <- scan(file, what = character(), sep = sep, quote = '"', comment.char = "",
                         nlines = 1L, quiet = TRUE)
          if (length(header) < ncol(table)) names(table)[1L] <- ""
        }
        table
      }, error = function(e) fail("Could not read the table. Check that the file has a header and one value per column."))
    }
    names(tab) <- fix_names(names(tab))
    columns <- Map(describe, names(tab), tab)
    assign(request$name, tab, envir = .GlobalEnv)
    list(table = list(name = request$name, rows = nrow(tab), columns = unname(columns)))
  }

  get_table <- function(name) {
    tab <- get0(name, envir = .GlobalEnv, inherits = FALSE)
    if (!is.data.frame(tab)) fail(paste0("Could not find the table '", name, "'."))
    tab
  }

  column <- function(tab, name, purpose, optional = FALSE, numeric = FALSE) {
    if (is.null(name) && optional) return(NULL)
    if (is.null(name)) fail(paste0("Choose a column for ", purpose, "."))
    if (!name %in% names(tab)) fail(paste0("The table has no column named '", name, "'."))
    x <- tab[[name]]
    if (numeric && !is.numeric(x)) fail(paste0("The column '", name, "' must contain numbers."))
    x
  }

  # A gene's label: its symbol when the card has a label column
  # (s$label) and the gene has one, else its ID, else its row number.
  gene_labels <- function(tab, s, rows) {
    ids <- column(tab, s$gene, "gene names", optional = TRUE)
    labels <- if (is.null(ids)) as.character(rows) else as.character(ids[rows])
    symbols <- column(tab, s$label, "gene symbols", optional = TRUE)
    if (!is.null(symbols)) {
      named <- as.character(symbols[rows])
      use <- !is.na(named) & nzchar(named)
      labels[use] <- named[use]
    }
    absent <- is.na(labels) | !nzchar(labels)
    labels[absent] <- as.character(rows[absent])
    labels
  }

  # Adds the gene symbols for an Ensembl or Entrez ID column, from the
  # organism's Bioconductor package, as a column beside it.
  add_symbols <- function(request) {
    org <- switch(request$organism, human = "org.Hs.eg.db", mouse = "org.Mm.eg.db",
                  fail("Gene symbols are available for human and mouse."))
    need_packages(c("AnnotationDbi", org))
    keytype <- switch(request$ids, ensembl = "ENSEMBL", entrez = "ENTREZID",
                      fail("Gene symbols come from Ensembl or Entrez IDs."))
    tab <- get_table(request$table)
    written <- as.character(column(tab, request$column, "gene IDs"))
    ids <- trimws(written)
    if (keytype == "ENSEMBL") ids <- sub("\\.[0-9]+$", "", ids)
    db <- getExportedValue(org, org)
    wanted <- unique(ids[!is.na(ids) & nzchar(ids)])
    known <- intersect(wanted, AnnotationDbi::keys(db, keytype = keytype))
    found <- if (length(known)) {
      suppressMessages(AnnotationDbi::mapIds(db, keys = known, column = "SYMBOL", keytype = keytype,
                                             multiVals = "list"))
    } else list()
    found <- Filter(function(x) length(x) && !all(is.na(x)), found)
    first <- vapply(found, function(x) as.character(x[!is.na(x)][1L]), character(1))
    symbol <- unname(first[ids])
    mapped <- sum(!is.na(symbol))
    # A gene with no symbol keeps its ID, as the table writes it, rather
    # than NA (Huan, 2026-11-19).
    symbol[is.na(symbol)] <- written[is.na(symbol)]
    out <- request$output
    if (is.null(out)) {
      out <- "symbol"
      n <- 1L
      while (out %in% names(tab)) {
        n <- n + 1L
        out <- paste0("symbol_", n)
      }
    }
    tab[[out]] <- symbol
    others <- setdiff(names(tab), out)
    at <- match(request$column, others)
    tab <- tab[, append(others, out, after = if (is.na(at)) length(others) else at), drop = FALSE]
    assign(request$table, tab, envir = .GlobalEnv)
    list(facts = list(column = out, total = length(ids), mapped = mapped,
                      several = sum(lengths(found[names(found) %in% ids]) > 1L)))
  }

  # The look of PubOmixR (Huan's package, Huan 2026-11-19): theme_classic at
  # 13 pt, a thin black frame, black axis text, no grid, the title centred
  # and the legend below. Up is red, down blue, as in its volcano plots.
  col_up <- "#E41A1C"
  col_up_light <- "#FB9A99"
  col_down <- "#1F78B4"
  col_down_light <- "#A6CEE3"
  col_ns <- "grey70"
  border_width <- 0.2
  darken <- function(col, factor = 0.7) {
    rgb <- grDevices::col2rgb(col) * factor
    grDevices::rgb(rgb[1, ], rgb[2, ], rgb[3, ], maxColorValue = 255)
  }
  plot_theme <- function(angled_x = FALSE, legend = "bottom") {
    theme <- ggplot2::theme_classic(base_size = 13) +
      ggplot2::theme(plot.title = ggplot2::element_text(hjust = 0.5),
                     axis.line = ggplot2::element_blank(),
                     panel.border = ggplot2::element_rect(colour = "black", fill = NA, linewidth = border_width),
                     axis.text = ggplot2::element_text(size = 13, colour = "black"),
                     axis.ticks = ggplot2::element_line(colour = "black", linewidth = border_width),
                     axis.ticks.length = ggplot2::unit(4, "pt"),
                     panel.grid.major = ggplot2::element_blank(),
                     panel.grid.minor = ggplot2::element_blank(),
                     legend.position = legend)
    if (angled_x) theme <- theme + ggplot2::theme(axis.text.x = ggplot2::element_text(angle = 45, hjust = 1))
    theme
  }

  finish_plot <- function(plot, facts, angled_x = FALSE, legend = "bottom") {
    print(plot + plot_theme(angled_x, legend))
    list(facts = facts)
  }

  de_plots <- function(plot, tab, s, output) {
    fc <- column(tab, s$fc, "fold change", numeric = TRUE)
    padj <- column(tab, s$padj, "adjusted p-values", numeric = TRUE)
    tested <- is.finite(fc) & is.finite(padj)
    if (any(padj[tested] < 0 | padj[tested] > 1)) fail("Adjusted p-values must be between 0 and 1.")
    lfc <- if (is.null(s$cutoffs$lfc)) 1 else s$cutoffs$lfc
    p_cutoff <- if (is.null(s$cutoffs$padj)) 0.05 else s$cutoffs$padj
    if (lfc < 0 || p_cutoff < 0 || p_cutoff > 1) fail("Choose a nonnegative fold change and an adjusted p-value between 0 and 1.")
    significant <- tested & padj < p_cutoff
    up <- which(significant & fc >= lfc)
    down <- which(significant & fc <= -lfc & fc < lfc)
    best <- function(rows, n) {
      rows[utils::head(order(padj[rows], rows), n)]
    }
    if (plot %in% c("top_genes", "results_heatmap")) {
      changed <- best(c(up, down), if (plot == "top_genes") length(up) + length(down) else 50L)
      if (!length(changed)) fail("No genes pass the cutoffs.")
      if (plot == "top_genes") {
        if (is.null(output)) fail("Choose a name for the top genes table.")
        column(tab, s$gene, "gene names", optional = TRUE)
        column(tab, s$label, "gene symbols", optional = TRUE)
        average <- s[["mean"]]
        column(tab, average, "average expression", optional = TRUE)
        assign(output, tab[changed, unique(c(s$gene, s$label, s$fc, s$padj, average, names(tab))), drop = FALSE], envir = .GlobalEnv)
        return(list(facts = list(rows = length(changed)), output = output))
      }
      expr <- sample_values(tab, s)
      draw_heatmap(z_scores(expr$x[changed, , drop = FALSE]), expr$group,
                   labels_row = gene_labels(tab, s, changed), title = "Most significant changed genes")
      return(list(facts = list(genes = length(changed), samples = ncol(expr$x))))
    }

    comparison <- s$comparison
    if (is.null(comparison)) {
      fc_label <- "log2 fold change"
      change_labels <- c("Positive fold change", "Negative fold change", "Not changed")
    } else {
      fc_label <- paste0("log2 fold change (", comparison$higher, " vs ", comparison$lower, ")")
      change_labels <- c(paste("Higher in", comparison$higher), paste("Higher in", comparison$lower), "Not changed")
    }
    keys <- c("up", "down", "unchanged")
    colours <- stats::setNames(c(col_up, col_down, col_ns), keys)
    top_up <- best(up, 10L)
    top_down <- best(down, 10L)
    facts <- list(tested = sum(tested), up = length(up), down = length(down), missing = sum(!tested),
                  top_up = I(gene_labels(tab, s, utils::head(top_up, 5L))),
                  top_down = I(gene_labels(tab, s, utils::head(top_down, 5L))))
    if (plot == "up_down") {
      d <- data.frame(change = factor(keys[1:2], levels = rev(keys[1:2])), n = c(length(up), length(down)))
      p <- ggplot2::ggplot(d, ggplot2::aes(x = n, y = change, fill = change)) + ggplot2::geom_col(width = 0.65) +
        ggplot2::scale_y_discrete(labels = stats::setNames(change_labels, keys)) +
        ggplot2::scale_fill_manual(values = colours, guide = "none") + ggplot2::labs(x = "Genes", y = NULL)
      return(finish_plot(p, facts))
    }

    status <- rep("unchanged", length(fc))
    status[up] <- "up"
    status[down] <- "down"
    rows <- which(tested)
    if (plot == "volcano") {
      # PubOmixR's own volcano plot when it is installed; the same look
      # drawn here otherwise, so a recipe runs on any computer.
      if (requireNamespace("PubOmixR", quietly = TRUE)) {
        v <- data.frame(log2FoldChange = fc[rows], padj = padj[rows])
        if (!is.null(s$gene)) v$symbol <- gene_labels(tab, s, rows)
        if (!is.null(s[["mean"]]) && !isTRUE(s$mean_log)) {
          v$baseMean <- column(tab, s[["mean"]], "average expression", numeric = TRUE)[rows]
        }
        suppressMessages(PubOmixR::vizVolcano(v, lfc_threshold = lfc, padj_cutoff = p_cutoff,
                                              top_n_labels = if (is.null(s$gene)) 0L else 10L,
                                              xlab = fc_label, title = NULL))
        return(list(facts = facts))
      }
      d <- data.frame(fc = fc[rows], p = -log10(pmax(padj[rows], 1e-300)), change = factor(status[rows], keys))
      p <- ggplot2::ggplot(d, ggplot2::aes(x = fc, y = p, colour = change)) +
        ggplot2::geom_vline(xintercept = c(-lfc, lfc), linetype = "dashed", colour = "grey40", linewidth = 0.5) +
        ggplot2::geom_hline(yintercept = -log10(max(p_cutoff, 1e-300)), linetype = "dashed", colour = "grey40", linewidth = 0.5) +
        ggplot2::geom_point(size = 1.2, alpha = 0.7) +
        # The counts above the outer zones, as PubOmixR places them.
        ggplot2::annotate("text", x = c((min(d$fc) - lfc) / 2, (lfc + max(d$fc)) / 2),
                          y = -0.02 * max(d$p) + 1.12 * max(d$p) * 0.95,
                          label = c(length(down), length(up)), colour = darken(c(col_down, col_up)),
                          size = 4.5, fontface = "bold") +
        ggplot2::scale_y_continuous(expand = ggplot2::expansion(mult = c(0.02, 0.10))) +
        ggplot2::labs(x = fc_label, y = "-log10 adjusted p-value")
      if (!is.null(s$gene) && length(c(top_up, top_down))) {
        selected <- c(top_up, top_down)
        labels <- d[match(selected, rows), , drop = FALSE]
        labels$gene <- gene_labels(tab, s, selected)
        p <- p + ggrepel::geom_text_repel(data = labels, ggplot2::aes(label = gene),
                                         colour = darken(colours[as.character(labels$change)]),
                                         max.overlaps = Inf, seed = 42L, size = 3.5, show.legend = FALSE)
      }
    } else {
      average <- column(tab, s[["mean"]], "average expression", numeric = TRUE)
      rows <- rows[is.finite(average[rows]) & (isTRUE(s$mean_log) | average[rows] > 0)]
      d <- data.frame(average = average[rows], fc = fc[rows], change = factor(status[rows], keys))
      p <- ggplot2::ggplot(d, ggplot2::aes(x = average, y = fc, colour = change)) +
        ggplot2::geom_hline(yintercept = c(-lfc, lfc), linetype = "dashed", colour = "grey40", linewidth = 0.5) +
        ggplot2::geom_point(size = 1.2, alpha = 0.7) + ggplot2::labs(x = "Average expression", y = fc_label)
      if (!isTRUE(s$mean_log)) p <- p + ggplot2::scale_x_log10()
    }
    finish_plot(p + ggplot2::scale_colour_manual(name = NULL, values = colours, limits = keys,
                                                 labels = change_labels), facts)
  }

  group_palette <- function(groups) {
    levels <- levels(groups)
    paired <- c(col_up, col_down, "#FF7F00", "#6A3D9A", col_up_light, col_down_light, "#33A02C", "#B2DF8A")
    colours <- if (length(levels) <= length(paired)) paired[seq_along(levels)] else
      c(paired, grDevices::hcl.colors(length(levels) - length(paired), "Dark 3"))
    stats::setNames(colours, levels)
  }

  sample_groups <- function(samples, groups) {
    if (!length(groups)) return(NULL)
    group <- vapply(groups, `[[`, character(1), 2L)
    index <- match(samples, vapply(groups, `[[`, character(1), 1L))
    if (all(is.na(index))) return(NULL)
    unassigned <- "No group"
    while (unassigned %in% group) unassigned <- paste0(unassigned, " (unassigned)")
    assigned <- group[index]
    assigned[is.na(index)] <- unassigned
    factor(assigned, levels = unique(assigned))
  }

  sample_values <- function(tab, s, library_sizes = FALSE) {
    samples <- vapply(s$samples, identity, character(1))
    if (length(unique(samples)) < 2L) fail("Choose at least two samples.")
    for (sample in samples) column(tab, sample, "samples", numeric = TRUE)
    x <- as.matrix(tab[samples])
    if (!nrow(x)) fail("The table has no values to plot.")
    if (any(!is.finite(x))) fail("The sample columns have missing or invalid values.")
    if (library_sizes && !identical(s$values, "counts")) fail("Library sizes need raw counts.")
    if (is.null(s$values)) fail("Choose whether the sample values are counts, normalized, or already logged.")
    if (s$values == "counts") {
      if (any(x != trunc(x))) fail("These values aren't whole numbers, so they can't be raw counts.")
      if (any(x < 0)) fail("Raw counts cannot be negative.")
      totals <- colSums(x)
      if (library_sizes) return(list(totals = totals, group = sample_groups(samples, s$groups)))
      if (any(totals == 0)) fail("Each sample needs at least one count above zero.")
      x <- log2(sweep(x, 2L, totals / 1e6, "/") + 1)
    } else if (s$values == "normalized") {
      if (any(x < 0)) fail("Normalized values cannot be negative unless they are already logged.")
      x <- log2(x + 1)
    }
    list(x = x, group = sample_groups(samples, s$groups))
  }

  row_variances <- function(x) rowSums((x - rowMeans(x))^2) / (ncol(x) - 1L)

  z_scores <- function(x) {
    x <- x - rowMeans(x)
    spread <- sqrt(rowSums(x^2) / (ncol(x) - 1L))
    spread[spread == 0] <- 1
    x / spread
  }

  draw_heatmap <- function(x, group, labels_row = rownames(x), title, bound = max(1, abs(x))) {
    annotation <- if (!is.null(group)) data.frame(Group = group, row.names = colnames(x))
    colours <- if (!is.null(group)) list(Group = group_palette(group))
    gradient <- grDevices::colorRampPalette(c(col_down, col_down_light, "white", col_up_light, col_up))(100L)
    invisible(pheatmap::pheatmap(x, color = gradient, breaks = seq(-bound, bound, length.out = 101L),
                                cluster_rows = nrow(x) > 1L, annotation_col = annotation,
                                annotation_colors = colours, labels_row = labels_row, main = title,
                                border_color = NA, angle_col = 45, fontsize = 10,
                                fontsize_row = if (nrow(x) > 30L) 6 else 9))
  }

  expression_plots <- function(plot, tab, s) {
    expr <- sample_values(tab, s, library_sizes = plot == "library_sizes")
    group <- expr$group
    if (plot == "library_sizes") {
      millions <- expr$totals / 1e6
      d <- data.frame(sample = factor(names(millions), levels = names(millions)), millions = unname(millions))
      p <- ggplot2::ggplot(d, ggplot2::aes(x = sample, y = millions)) + ggplot2::labs(x = NULL, y = "Total counts (millions)")
      if (is.null(group)) p <- p + ggplot2::geom_col(fill = col_down) else {
        d$group <- group
        p <- p + ggplot2::geom_col(data = d, ggplot2::aes(fill = group)) +
          ggplot2::scale_fill_manual(name = "Group", values = group_palette(group))
      }
      size <- function(i) list(sample = names(millions)[i], millions = unname(millions[i]))
      return(finish_plot(p, list(smallest = size(which.min(millions)), largest = size(which.max(millions))), angled_x = TRUE))
    }

    x <- expr$x
    if (plot == "gene_by_group") {
      genes <- as.character(column(tab, s$gene, "gene names"))
      if (is.null(s$show_gene) || !nzchar(s$show_gene)) fail("Choose a gene to show.")
      # Found by its symbol or its ID, as written, then ignoring case.
      labels <- gene_labels(tab, s, seq_along(genes))
      row <- match(s$show_gene, labels)
      if (is.na(row)) row <- match(s$show_gene, genes)
      if (is.na(row)) row <- match(tolower(s$show_gene), tolower(labels))
      if (is.na(row)) row <- match(tolower(s$show_gene), tolower(genes))
      if (is.na(row)) fail(paste0("Could not find the gene '", s$show_gene, "'."))
      label <- if (is.null(group)) "Sample" else "Group"
      if (is.null(group)) group <- factor(colnames(x), levels = colnames(x))
      d <- data.frame(group = group, value = unname(x[row, ]))
      by_group <- split(d$value, d$group)
      means <- data.frame(group = factor(names(by_group), levels(group)), mean = vapply(by_group, mean, numeric(1)),
                          n = lengths(by_group))
      axis_label <- switch(s$values, counts = "log2(counts per million + 1)", normalized = "log2(expression + 1)",
                           logged = "Logged expression")
      p <- ggplot2::ggplot(d, ggplot2::aes(x = group, y = value)) +
        ggplot2::geom_col(data = means, ggplot2::aes(y = mean, fill = group), width = 0.65, alpha = 0.55) +
        ggplot2::geom_point(position = ggplot2::position_jitter(width = 0.12, height = 0, seed = 42L), size = 2) +
        ggplot2::scale_fill_manual(values = group_palette(group), guide = "none") +
        ggplot2::labs(x = label, y = axis_label, title = labels[row])
      facts <- list(gene = labels[row], means = lapply(seq_len(nrow(means)), function(i) {
        list(group = as.character(means$group[i]), mean = means$mean[i], n = means$n[i])
      }))
      return(finish_plot(p, facts, angled_x = TRUE))
    }

    variance <- row_variances(x)
    if (plot == "correlation") {
      varying <- x[variance > 0, , drop = FALSE]
      if (nrow(varying) < 2L) fail("At least two genes need to vary across samples to compare them.")
      if (any(apply(varying, 2L, function(col) all(col == col[1L])))) {
        fail("Each sample needs more than one expression value to compare it with the others.")
      }
      r <- stats::cor(varying)
      pairs <- which(lower.tri(r), arr.ind = TRUE)
      i <- which.min(r[pairs])
      draw_heatmap(r, group, title = "Sample correlation", bound = 1)
      return(list(facts = list(lowest = list(a = colnames(r)[pairs[i, 2L]], b = rownames(r)[pairs[i, 1L]],
                                             r = round(r[pairs[i, , drop = FALSE]], 3L)))))
    }

    selected <- utils::head(order(variance, decreasing = TRUE), if (plot == "pca") 500L else 50L)
    if (plot == "variable_heatmap") {
      draw_heatmap(z_scores(x[selected, , drop = FALSE]), group,
                   labels_row = gene_labels(tab, s, selected), title = "Most variable genes")
      return(list(facts = list(genes = length(selected), samples = ncol(x))))
    }

    if (length(selected) < 2L || !any(variance > 0)) fail("PCA needs at least two genes and some variation across samples.")
    pc <- stats::prcomp(t(x[selected, , drop = FALSE]), rank. = 2L)
    percent <- round(100 * pc$sdev[1:2]^2 / sum(pc$sdev^2), 1L)
    d <- data.frame(sample = colnames(x), pc1 = pc$x[, 1L], pc2 = pc$x[, 2L])
    p <- ggplot2::ggplot(d, ggplot2::aes(x = pc1, y = pc2)) +
      ggplot2::labs(x = sprintf("PC1 (%.1f%%)", percent[1L]), y = sprintf("PC2 (%.1f%%)", percent[2L]))
    if (is.null(group)) p <- p + ggplot2::geom_point(colour = col_down, size = 2.6) else {
      d$group <- group
      p <- p + ggplot2::geom_point(data = d, ggplot2::aes(colour = group), size = 2.6) +
        ggplot2::scale_colour_manual(name = "Group", values = group_palette(group))
    }
    p <- p + ggrepel::geom_text_repel(ggplot2::aes(label = sample), max.overlaps = Inf, seed = 42L, size = 3)
    finish_plot(p, list(pc1 = percent[1L], pc2 = percent[2L], genes = length(selected)))
  }

  enrich_plots <- function(plot, tab, s) {
    terms <- as.character(column(tab, s$term, "term names"))
    padj <- column(tab, s$padj, "adjusted p-values", numeric = TRUE)
    if (any(padj[is.finite(padj)] < 0 | padj[is.finite(padj)] > 1)) fail("Adjusted p-values must be between 0 and 1.")
    valid <- which(is.finite(padj) & !is.na(terms) & nzchar(terms))
    rows <- utils::head(valid[order(padj[valid])], 20L)
    if (!length(rows)) fail("No terms have both a name and an adjusted p-value.")
    shown <- length(rows)
    d <- data.frame(term = rev(seq_len(shown)), padj = padj[rows])
    labels <- vapply(terms[rows], function(term) paste(strwrap(term, width = 32L), collapse = "\n"), character(1))
    p <- ggplot2::ggplot(d, ggplot2::aes(y = term)) +
      ggplot2::scale_y_continuous(breaks = d$term, labels = labels) + ggplot2::labs(y = NULL) +
      # PubOmixR's light bands behind every other term.
      lapply(seq(1L, shown, by = 2L), function(i) {
        ggplot2::annotate("rect", xmin = -Inf, xmax = Inf, ymin = i - 0.5, ymax = i + 0.5,
                          fill = "grey93", colour = NA, alpha = 0.5)
      })
    if (plot == "enrich_bar") {
      d$significance <- -log10(pmax(d$padj, 1e-300))
      p <- p + ggplot2::geom_col(data = d, ggplot2::aes(x = significance), fill = col_up, width = 0.7, orientation = "y") +
        ggplot2::labs(x = "-log10 adjusted p-value")
    } else {
      count <- column(tab, s$count, "gene counts", optional = TRUE, numeric = TRUE)[rows]
      ratio <- column(tab, s$ratio, "gene ratios", optional = TRUE)[rows]
      if (!is.null(ratio) && !is.numeric(ratio)) {
        parts <- regmatches(ratio, regexec("^\\s*([0-9]+)\\s*/\\s*([0-9]+)\\s*$", ratio, perl = TRUE))
        ratio <- vapply(parts, function(part) if (length(part)) as.numeric(part[2L]) / as.numeric(part[3L]) else NA_real_, numeric(1))
      }
      if (!is.null(count) && any(!is.finite(count) | count < 0)) fail("The top terms need valid gene counts.")
      if (!is.null(ratio) && all(is.finite(ratio) & ratio >= 0)) {
        d$value <- ratio
        axis_label <- "Gene ratio"
      } else if (!is.null(count)) {
        d$value <- count
        axis_label <- "Gene count"
      } else if (!is.null(ratio)) fail("The top terms need valid gene ratios.") else fail("Choose a column for gene counts or gene ratios.")
      # PubOmixR's dot plot: circles outlined in black, filled from red
      # (most significant) to white.
      if (is.null(count)) {
        p <- p + ggplot2::geom_point(data = d, ggplot2::aes(x = value, fill = pmax(padj, 1e-300)), size = 4,
                                     shape = 21, colour = "black", stroke = 0.5)
      } else {
        d$count <- count
        p <- p + ggplot2::geom_point(data = d, ggplot2::aes(x = value, fill = pmax(padj, 1e-300), size = count),
                                     shape = 21, colour = "black", stroke = 0.5) +
          ggplot2::scale_size_continuous(name = "Gene count", range = c(3, 8))
      }
      p <- p + ggplot2::scale_fill_gradientn(name = "Adjusted p-value", colours = c(col_up, "#FFFFFF"),
                                             trans = "log10", guide = ggplot2::guide_colourbar(reverse = TRUE),
                                             labels = function(x) parse(text = ifelse(is.na(x) | x <= 0, "NA",
                                                                                      paste0("10^{", floor(log10(x)), "}")))) +
        ggplot2::scale_x_continuous(expand = ggplot2::expansion(mult = c(0.12, 0.15))) +
        ggplot2::labs(x = axis_label)
    }
    finish_plot(p, list(terms = length(terms), shown = shown, top = I(utils::head(terms[rows], 3L))),
                legend = "right")
  }

  workbook_names <- function(nms) {
    used <- character()
    for (i in seq_along(nms)) {
      base <- gsub("[\\[\\]:*?/\\\\]", "_", nms[i], perl = TRUE)
      base <- sub("^'+", "", trimws(base))
      short <- function(width) sub("'+$", "", substr(base, 1L, width))
      name <- short(31L)
      if (!nzchar(name)) base <- name <- "Sheet"
      j <- 1L
      while (tolower(name) %in% used) {
        suffix <- paste0("_", j)
        name <- paste0(short(31L - nchar(suffix)), suffix)
        j <- j + 1L
      }
      used <- c(used, tolower(name))
      nms[i] <- name
    }
    nms
  }

  function(request) {
    if (request$action == "sheets") {
      need_packages("readxl")
      sheets <- tryCatch(readxl::excel_sheets(request$file), error = function(e) fail("Could not read the workbook."))
      return(list(sheets = I(sheets)))
    }
    if (request$action == "load") return(load_table(request))
    if (request$action == "symbols") return(add_symbols(request))
    if (request$action == "pairs") {
      tab <- get_table(request$table)
      key <- as.character(column(tab, request$key, "the samples"))
      value <- as.character(column(tab, request$value, "the groups"))
      keep <- !is.na(key) & nzchar(key) & !is.na(value) & nzchar(value)
      return(list(pairs = I(Map(function(k, v) I(c(k, v)), key[keep], value[keep], USE.NAMES = FALSE))))
    }
    if (request$action == "workbook") {
      need_packages("writexl")
      first <- function(pairs) vapply(pairs, `[[`, character(1), 1L)
      second <- function(pairs) vapply(pairs, `[[`, character(1), 2L)
      sheets <- c(list(data.frame(Field = first(request$readme), Description = second(request$readme))),
                  lapply(second(request$sheets), get_table))
      names(sheets) <- workbook_names(c("Read_me", first(request$sheets)))
      tryCatch(writexl::write_xlsx(sheets, path = request$path), error = function(e) fail("Could not write the workbook."))
      return(list(facts = list(path = request$path)))
    }

    plot <- request$plot
    s <- request$settings
    need_packages(switch(plot, top_genes = character(), results_heatmap = "pheatmap", variable_heatmap = "pheatmap",
                         correlation = "pheatmap", pca = c("ggplot2", "ggrepel"),
                         volcano = c("ggplot2", if (!is.null(s$gene)) "ggrepel"), "ggplot2"))
    tab <- get_table(request$table)
    if (plot %in% c("volcano", "ma", "up_down", "top_genes", "results_heatmap")) return(de_plots(plot, tab, s, request$output))
    if (plot %in% c("enrich_dot", "enrich_bar")) return(enrich_plots(plot, tab, s))
    expression_plots(plot, tab, s)
  }
})
