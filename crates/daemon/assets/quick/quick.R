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

  table_extension <- function(file) tolower(tools::file_ext(sub("\\.gz$", "", file, ignore.case = TRUE)))

  table_input <- function(file, read) {
    if (!grepl("\\.gz$", file, ignore.case = TRUE)) return(read(file))
    input <- gzfile(file, open = "rt")
    on.exit(close(input))
    read(input)
  }

  table_separator <- function(file, extension, fallback = ",") {
    switch(extension, csv = ",", tsv = "\t", tab = "\t", {
      line <- table_input(file, function(input) readLines(input, n = 8L, warn = FALSE))
      line <- utils::head(line[nzchar(trimws(line))], 1L)
      count <- function(separator) nchar(line) - nchar(gsub(separator, "", line, fixed = TRUE))
      if (length(line) && count("\t") > count(",")) "\t"
      else if (length(line) && count(",") > 0) ","
      else fallback
    })
  }

  describe_table <- function(name, tab) {
    columns <- Map(describe, names(tab), tab)
    assign(name, tab, envir = .GlobalEnv)
    list(name = name, rows = nrow(tab), columns = unname(columns))
  }

  read_sample_counts <- function(file) {
    invalid <- function() fail(paste0(basename(file), " needs two columns: gene IDs and whole, nonnegative counts."))
    tab <- tryCatch({
      sep <- table_separator(file, table_extension(file), fallback = "")
      table_input(file, function(input) {
        utils::read.table(input, header = FALSE, sep = sep, quote = '"', comment.char = "",
                          na.strings = NULL, colClasses = "character", fill = FALSE)
      })
    }, error = function(e) invalid())
    if (ncol(tab) != 2L || !nrow(tab)) invalid()
    tab[] <- lapply(tab, trimws)
    tab[1L, 1L] <- sub("^\ufeff", "", tab[1L, 1L])
    header <- tolower(gsub("[ .-]+", "_", unlist(tab[1L, ])))
    count_names <- c("count", "counts", "reads", "raw_count", "raw_counts")
    gene_names <- c("gene", "gene_id", "geneid", "id", "symbol", "ensembl", "entrez")
    candidates <- c(which(header %in% count_names), 3L - which(header %in% gene_names))
    numbers <- lapply(tab, function(col) suppressWarnings(as.numeric(col)))
    valid <- lapply(numbers, function(x) is.finite(x) & x >= 0 & x == trunc(x))
    if (length(candidates)) {
      count <- candidates[1L]
      # A real gene can be called ID or SYMBOL in a headerless file.
      start <- if (valid[[count]][1L]) 1L else 2L
    } else {
      start <- 1L
      # When both columns contain numbers, the first holds the IDs.
      count <- if (all(valid[[1L]]) && all(is.na(numbers[[2L]]))) 1L else 2L
    }
    if (nrow(tab) < start) invalid()
    rows <- seq.int(start, nrow(tab))
    genes <- tab[[3L - count]][rows]
    if (!all(valid[[count]][rows]) || any(!nzchar(genes))) invalid()
    # HTSeq includes these counters below the gene rows.
    keep <- !genes %in% c("__no_feature", "__ambiguous", "__too_low_aQual",
                          "__not_aligned", "__alignment_not_unique")
    genes <- genes[keep]
    if (!length(genes)) invalid()
    if (anyDuplicated(genes)) fail(paste0(basename(file), " lists a gene ID more than once."))
    list(genes = genes, counts = numbers[[count]][rows][keep])
  }

  sample_names <- function(files) {
    stems <- sub("\\.(csv|tsv|txt|tab)(\\.gz)?$", "", basename(files), ignore.case = TRUE)
    tokens <- strsplit(stems, "[_. -]+")
    common <- function(last) {
      all(lengths(tokens) > 1L) &&
        length(unique(vapply(tokens, function(x) x[if (last) length(x) else 1L], ""))) == 1L
    }
    while (common(FALSE)) tokens <- lapply(tokens, function(x) x[-1L])
    while (common(TRUE)) tokens <- lapply(tokens, function(x) x[-length(x)])
    names <- vapply(tokens, paste, "", collapse = "_")
    names[!nzchar(names)] <- stems[!nzchar(names)]
    make.unique(c("gene", names), sep = "_")[-1L]
  }

  combine_counts <- function(request) {
    files <- unlist(request$files, use.names = FALSE)
    if (length(files) < 2L) fail("Choose at least two samples to combine.")
    samples <- lapply(files, read_sample_counts)
    genes <- Reduce(intersect, lapply(samples, `[[`, "genes"))
    if (!length(genes)) fail("The files have no gene IDs in common.")
    names <- sample_names(files)
    counts <- lapply(samples, function(s) s$counts[match(genes, s$genes)])
    tab <- as.data.frame(c(list(gene = genes), setNames(counts, names)), check.names = FALSE)
    output <- tempfile(tmpdir = dirname(request$file))
    on.exit(unlink(output))
    tryCatch({
      utils::write.csv(tab, output, row.names = FALSE)
      if (!file.rename(output, request$file)) stop("rename failed")
    }, error = function(e) fail("Could not save the combined counts table."))
    inputs <- Map(function(name, sample) list(sample = name, genes = length(sample$genes)),
                  names, samples, USE.NAMES = FALSE)
    list(table = describe_table(request$name, tab),
         facts = list(kept = length(genes), inputs = I(inputs)))
  }

  load_table <- function(request) {
    file <- request$file
    extension <- table_extension(file)
    if (extension %in% c("xlsx", "xls")) {
      need_packages("readxl")
      tab <- tryCatch(readxl::read_excel(file, sheet = request$sheet, .name_repair = "minimal"),
                      error = function(e) fail("Could not read the selected sheet."))
      tab <- as.data.frame(tab, check.names = FALSE, stringsAsFactors = FALSE)
    } else {
      tab <- tryCatch({
        sep <- table_separator(file, extension)
        table <- table_input(file, function(input) {
          utils::read.table(input, header = TRUE, sep = sep, quote = '"', comment.char = "",
                            na.strings = c("NA", ""), row.names = NULL, check.names = FALSE,
                            stringsAsFactors = FALSE)
        })
        # read.table uses this name for an extra leading field when the header is short.
        if (identical(names(table)[1L], "row.names")) {
          header <- table_input(file, function(input) {
            scan(input, what = character(), sep = sep, quote = '"', comment.char = "",
                 nlines = 1L, quiet = TRUE)
          })
          if (length(header) < ncol(table)) names(table)[1L] <- ""
        }
        table
      }, error = function(e) fail("Could not read the table. Check that the file has a header and one value per column."))
    }
    names(tab) <- fix_names(names(tab))
    list(table = describe_table(request$name, tab))
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

  strings <- function(x) vapply(x, identity, character(1))
  pair_side <- function(x, i) vapply(x, `[[`, character(1), i)

  sample_factors <- function(request) {
    tab <- get_table(request$table)
    key <- as.character(column(tab, request$key, "sample names"))
    present <- !is.na(key) & nzchar(key)
    if (anyDuplicated(key[present])) fail("The sample sheet lists the same sample more than once.")
    factors <- lapply(setdiff(names(tab), request$key), function(name) {
      value <- as.character(tab[[name]])
      keep <- present & !is.na(value) & nzchar(value)
      list(name = name, levels = I(unique(value[keep])),
           assignments = I(Map(function(k, v) I(c(k, v)), key[keep], value[keep], USE.NAMES = FALSE)))
    })
    list(factors = I(factors))
  }

  de_formula <- function(text, available) {
    expr <- tryCatch(str2lang(text), error = function(e) NULL)
    if (!is.call(expr) || length(expr) != 2L || !identical(expr[[1L]], as.name("~"))) {
      fail("Start the design with ~, followed by the sample factors to include.")
    }
    # Check the parsed tree before any part is handed to model.matrix.
    check <- function(x) {
      if (is.symbol(x)) {
        if (!as.character(x) %in% available) fail(paste0("There is no sample factor named '", x, "'."))
      } else if (is.call(x)) {
        op <- x[[1L]]
        if (!is.symbol(op) || !as.character(op) %in% c("+", "-", "*", ":", "/", "(", "^")) {
          fail("Use only sample factors and formula operators such as +, * and :.")
        }
        if (identical(op, as.name("^"))) {
          if (length(x) != 3L || !is.numeric(x[[3L]]) || !x[[3L]] %in% 1:4) {
            fail("Use a whole-number interaction order from 1 to 4 after ^.")
          }
          check(x[[2L]])
        } else for (part in as.list(x)[-1L]) check(part)
      } else if (!is.numeric(x) || !x %in% 0:1) {
        fail("Use only sample factors, 0 or 1, and formula operators in the design.")
      }
    }
    check(expr[[2L]])
    stats::as.formula(expr, env = baseenv())
  }

  de_factor <- function(factors, name, samples) {
    spec <- factors[[name]]
    if (is.null(spec)) fail(paste0("There is no sample factor named '", name, "'."))
    ids <- pair_side(spec$assignments, 1L)
    if (anyDuplicated(ids)) fail(paste0("A sample has more than one value for '", name, "'."))
    written <- pair_side(spec$assignments, 2L)[match(samples, ids)]
    values <- factor(written, levels = strings(spec$levels))
    if (anyNA(values)) fail(paste0("Give every sample a value for '", name, "'."))
    if (nlevels(values) < 2L) fail(paste0("The factor '", name, "' needs at least two levels."))
    empty <- which(table(values) == 0L)
    if (length(empty)) fail(paste0("The group '", levels(values)[empty[1L]], "' has no samples."))
    values
  }

  de_matrix <- function(formula, data) {
    used <- all.vars(formula)
    cats <- used[vapply(data[used], is.factor, logical(1))]
    coding <- setNames(rep("contr.treatment", length(cats)), cats)
    mat <- tryCatch(stats::model.matrix(formula, data, contrasts.arg = as.list(coding)),
                    error = function(e) fail("This formula does not describe a valid design. Check its terms and operators."))
    if (ncol(mat)) colnames(mat) <- make.unique(colnames(mat))
    mat
  }

  de_rank <- function(m) if (ncol(m)) qr(m)$rank else 0L
  de_terms <- function(formula) attr(stats::terms(formula), "term.labels")
  de_formula_text <- function(formula) paste(deparse(formula, width.cutoff = 500L), collapse = " ")

  de_model <- function(samples, design, batch, reduced = NULL) {
    if (length(samples) < 3L || anyDuplicated(samples)) fail("Choose at least three different samples.")
    names_ <- vapply(design$factors, `[[`, character(1), "name")
    if (any(!nzchar(names_) | names_ == ".") || anyDuplicated(names_)) fail("Give each sample factor a different name.")
    factors <- setNames(design$factors, names_)
    combined <- design$combined
    if (!is.null(combined) && (combined$name %in% names_ || identical(combined$first, combined$second))) {
      fail("Choose two different sample factors for the combined groups.")
    }
    formula <- de_formula(design$formula, c(names_, combined$name))
    reduced <- if (!is.null(reduced)) de_formula(reduced, c(names_, combined$name))
    used <- union(all.vars(formula), all.vars(reduced))
    batch_name <- batch$factor
    if (!is.null(batch_name) && batch_name %in% c(used, combined$first, combined$second)) {
      fail("Batch is already in the design. Choose a different batch factor, or use None.")
    }
    needed <- setdiff(c(used, batch_name, combined$first, combined$second), combined$name)
    data <- data.frame(row.names = samples)
    for (name in needed) data[[name]] <- de_factor(factors, name, samples)
    if (!is.null(combined)) {
      a <- data[[combined$first]]
      b <- data[[combined$second]]
      level_names <- as.vector(outer(levels(b), levels(a), function(b, a) paste(a, b, sep = "_")))
      if (anyDuplicated(level_names)) fail("Some combined groups have the same name. Rename their factor levels.")
      data[[combined$name]] <- droplevels(factor(paste(a, b, sep = "_"), levels = level_names))
      if (nlevels(data[[combined$name]]) < 2L) fail("The combined factor needs at least two groups.")
    }
    if (!is.null(design$numeric)) {
      name <- design$numeric
      if (!name %in% used) fail("Choose a numeric factor in the design.")
      numeric_levels <- suppressWarnings(as.numeric(levels(data[[name]])))
      if (any(!is.finite(numeric_levels)) || anyDuplicated(numeric_levels)) fail("A dose trend needs distinct numeric levels.")
      data[[name]] <- numeric_levels[as.integer(data[[name]])]
    }
    biology <- de_matrix(formula, data)
    if (!ncol(biology) || de_rank(biology) != ncol(biology)) {
      fail("The design is not of full rank: some terms cannot be told apart. Check for missing groups or factors that always go together.")
    }
    batch_values <- if (!is.null(batch_name)) data[[batch_name]]
    combat <- batch$mode %in% c("combat_seq", "combat_ref")
    if (!is.null(batch_values)) {
      if (combat && any(table(batch_values) < 2L)) fail("ComBat needs at least two samples in each batch.")
      together <- cbind(biology, stats::model.matrix(~ batch_values)[, -1L, drop = FALSE])
      if (de_rank(together) < ncol(together)) {
        fail(paste0("The factor '", batch_name, "' and the groups in this design cannot be told apart. ",
                    "Check that the compared groups are not separated by batch."))
      }
      if (combat && nrow(together) <= ncol(together)) fail("There are not enough samples to estimate the batch and the design together.")
    }
    add <- function(f) {
      if (is.null(f) || !batch$mode %in% c("model", "pairs")) return(f)
      stats::as.formula(call("~", call("+", as.name(batch_name), f[[2L]])), env = baseenv())
    }
    full <- add(formula)
    null <- add(reduced)
    m <- de_matrix(full, data)
    if (de_rank(m) < ncol(m)) fail("The design is not of full rank: some terms cannot be told apart.")
    if (nrow(m) <= ncol(m)) fail("There are not enough samples left to estimate variation between samples.")
    r <- if (!is.null(null)) de_matrix(null, data)
    if (!is.null(r) && (!ncol(r) || de_rank(r) != ncol(r) || ncol(r) >= ncol(m) || de_rank(cbind(m, r)) != ncol(m))) {
      fail("The reduced formula must be a smaller part of the full design. Remove the terms you want to test.")
    }
    list(data = data, full = full, reduced = null, m = m, r = r,
         biology = biology, batch = batch_values, combined = combined)
  }

  de_replicates <- function(n, label) {
    if (n < 2L) fail(paste0("The group '", label, "' needs at least two samples."))
  }

  de_term_replicates <- function(data, term) {
    factors <- all.vars(str2lang(term))
    factors <- factors[vapply(data[factors], is.factor, logical(1))]
    if (!length(factors)) return(invisible(NULL))
    groups <- do.call(interaction, c(unname(as.list(data[factors])), list(drop = FALSE, sep = " / ")))
    sizes <- table(groups)
    for (group in names(sizes)[sizes < 2L]) de_replicates(sizes[[group]], group)
  }

  de_contrast <- function(model, contrast) {
    data <- model$data
    m <- model$m
    references <- data[rep(1L, 4L), , drop = FALSE]
    for (name in names(data)) {
      references[[name]][] <- if (is.factor(data[[name]])) levels(data[[name]])[1L] else min(data[[name]])
    }
    terms <- de_terms(model$full)
    row_at <- function(row, factor, level) {
      if (!level %in% levels(data[[factor]])) fail(paste0("There is no level '", level, "' in the factor '", factor, "'."))
      references[[factor]][row] <<- level
    }
    check_cells <- function(row, factors) {
      values <- vapply(factors, function(f) as.character(references[[f]][row]), character(1))
      selected <- rep(TRUE, nrow(data))
      for (i in seq_along(factors)) selected <- selected & data[[factors[i]]] == values[i]
      de_replicates(sum(selected), paste(values, collapse = " / "))
    }
    if (contrast$kind == "coefficient") {
      at <- match(contrast$name, colnames(m))
      if (is.na(at)) fail(paste0("There is no coefficient named '", contrast$name, "'. List coefficients from this formula first."))
      term <- attr(m, "assign")[at]
      if (!term) fail("Choose a coefficient other than the intercept.")
      de_term_replicates(data, terms[term])
      effect <- rep(0, ncol(m))
      effect[at] <- 1
    } else if (contrast$kind == "levels") {
      factor <- contrast$factor
      if (!factor %in% all.vars(model$full)) fail(paste0("The design does not use the factor '", factor, "'."))
      row_at(1L, factor, contrast$higher)
      row_at(2L, factor, contrast$lower)
      for (pair in contrast$at) row_at(1:2, pair[[1L]], pair[[2L]])
      scopes <- lapply(terms, function(term) all.vars(str2lang(term)))
      scopes <- unique(unlist(scopes[lengths(scopes) > 1L & vapply(scopes, function(x) factor %in% x, logical(1))]))
      cats <- scopes[vapply(data[scopes], is.factor, logical(1))]
      groups <- unique(c(factor, cats, pair_side(contrast$at, 1L)))
      check_cells(1L, groups)
      check_cells(2L, groups)
      rows <- de_matrix(model$full, references[1:2, , drop = FALSE])
      effect <- rows[1L, ] - rows[2L, ]
    } else if (contrast$kind == "interaction") {
      a <- contrast$first
      b <- contrast$second
      row_at(c(1L, 2L), a, contrast$first_higher)
      row_at(c(1L, 3L), b, contrast$second_higher)
      for (row in 1:4) check_cells(row, c(a, b))
      rows <- de_matrix(model$full, references)
      effect <- rows[1L, ] - rows[2L, ] - rows[3L, ] + rows[4L, ]
    } else fail("Choose the groups or coefficient to compare.")
    if (!any(effect != 0)) fail("Choose two different groups that the design can compare.")
    unname(effect)
  }

  de_counts <- function(tab, samples) {
    for (sample in samples) column(tab, sample, "sample counts", numeric = TRUE)
    x <- as.matrix(tab[samples])
    if (!nrow(x)) fail("The table has no genes to test.")
    if (any(!is.finite(x) | x < 0 | x != trunc(x))) {
      fail("These do not look like raw counts. DESeq2, edgeR and limma-voom need nonnegative whole numbers with no missing values.")
    }
    if (any(colSums(x) == 0)) fail("Every sample needs at least one count above zero.")
    rownames(x) <- as.character(seq_len(nrow(x)))
    x
  }

  # Give methods that need a null mean an intercept, even with ~ 0 + factor.
  de_with_intercept <- function(m) {
    both <- cbind(Intercept = 1, m)
    q <- qr(both)
    both[, sort(q$pivot[seq_len(q$rank)]), drop = FALSE]
  }

  de_dge <- function(x, m) {
    y <- edgeR::DGEList(x)
    keep <- edgeR::filterByExpr(y, design = m)
    if (!any(keep)) fail("No genes have enough counts to test.")
    edgeR::normLibSizes(y[keep, , keep.lib.sizes = FALSE])
  }

  de_latent <- function(x, m, mode) {
    mod <- de_with_intercept(m)
    if (nrow(mod) - ncol(mod) < 2L) fail("There are not enough samples to estimate hidden batch factors.")
    positive <- rowSums(x) > 0
    if (sum(positive) < 3L) fail("Estimating hidden batch factors needs at least three expressed genes.")
    if (mode == "sva") {
      n <- min(sva::num.sv(log(x[positive, , drop = FALSE] + 1), mod, method = "leek"), nrow(mod) - ncol(mod) - 1L)
      if (n <= 0L) return(matrix(numeric(), nrow(m), 0L))
      w <- sva::svaseq(x[positive, , drop = FALSE], mod, mod0 = matrix(1, nrow(mod), 1L), n.sv = n)$sv
    } else {
      if (ncol(mod) <= 1L) fail("RUVg needs a biological term to find empirical control genes.")
      y <- de_dge(x, mod)
      v <- limma::voom(y, mod, plot = FALSE)
      fit <- limma::eBayes(limma::lmFit(v, mod))
      first <- limma::topTable(fit, coef = seq.int(2L, ncol(mod)), number = Inf, sort.by = "none")
      p <- first$P.Value
      eligible <- which(is.finite(p))
      if (length(eligible) < 3L) fail("Too few expressed genes to choose controls for RUVg.")
      # Least evidence of change in the biological model from a first pass.
      controls <- utils::head(eligible[order(p[eligible], decreasing = TRUE)],
                             min(1000L, max(3L, floor(length(eligible) / 4L))))
      w <- RUVSeq::RUVg(y$counts, cIdx = controls, k = 1L)$W
    }
    colnames(w) <- paste0(if (mode == "sva") "SV" else "W", seq_len(ncol(w)))
    w
  }

  de_shrink <- function(fit, m, contrast, omnibus) {
    idx <- which.max(abs(contrast))
    if (omnibus || sum(contrast != 0) != 1L || contrast[idx] != 1) {
      # Express the chosen contrast as one coefficient for apeglm.
      basis <- diag(ncol(m))
      basis[idx, ] <- contrast
      reparam <- m %*% solve(basis)
      colnames(reparam) <- colnames(m)
      fit <- DESeq2::nbinomWaldTest(fit, betaPrior = FALSE, modelMatrix = reparam, quiet = TRUE)
    }
    DESeq2::lfcShrink(fit, coef = idx, type = "apeglm", quiet = TRUE)
  }

  de_fit <- function(x, model, contrasts, s) {
    m <- model$m
    reduced <- model$r
    omnibus <- !is.null(reduced)
    if (s$method == "deseq2") {
      if (any(x > .Machine$integer.max)) fail("A count is too large for DESeq2. Choose edgeR or limma-voom instead.")
      colnames(m) <- paste0("coefficient_", seq_len(ncol(m)))
      if (omnibus) colnames(reduced) <- paste0("reduced_", seq_len(ncol(reduced)))
      dds <- DESeq2::DESeqDataSetFromMatrix(x, model$data, design = m)
      # Median-ratio size factors need a gene with positive counts in every sample.
      sf <- if (any(rowSums(x == 0) == 0L)) "ratio" else "poscounts"
      fit <- if (omnibus) DESeq2::DESeq(dds, test = "LRT", reduced = reduced, sfType = sf, quiet = TRUE) else
        DESeq2::DESeq(dds, test = "Wald", sfType = sf, quiet = TRUE)
      results <- lapply(contrasts, function(contrast) {
        res <- as.data.frame(DESeq2::results(fit, contrast = contrast, alpha = s$cutoffs$padj))
        if (isTRUE(s$shrink)) {
          lead <- de_shrink(fit, m, contrast, omnibus)
          res$log2FoldChange <- lead$log2FoldChange
          res$lfcSE <- lead$lfcSE
        }
        res
      })
      vst <- DESeq2::varianceStabilizingTransformation(fit, blind = FALSE)
      return(list(results = results, norm = SummarizedExperiment::assay(vst), size_factors = sf))
    }

    y <- de_dge(x, m)
    if (omnibus) {
      # Same full model in a basis whose last terms are the part absent from
      # the reduced model. Transform the displayed contrast to that basis.
      added <- seq.int(ncol(reduced) + 1L, ncol(m))
      extra <- qr(cbind(reduced, m))$pivot[added] - ncol(reduced)
      fit_design <- cbind(reduced, m[, extra, drop = FALSE])
      mapping <- qr.coef(qr(m), fit_design)
      contrasts <- lapply(contrasts, function(ct) drop(crossprod(mapping, ct)))
    } else fit_design <- m
    colnames(fit_design) <- paste0("coefficient_", seq_len(ncol(fit_design)))
    effective <- y$samples$lib.size * y$samples$norm.factors
    expand <- function(result) result[match(rownames(x), rownames(result)), , drop = FALSE]
    if (s$method == "edger") {
      y <- edgeR::estimateDisp(y, fit_design, robust = TRUE)
      fit <- edgeR::glmQLFit(y, fit_design, robust = TRUE)
      test <- function(ct) edgeR::glmQLFTest(fit, contrast = ct)$table
      joint <- if (omnibus) edgeR::glmQLFTest(fit, coef = added)$table
      results <- lapply(contrasts, function(ct) {
        fc <- test(ct)
        p <- if (omnibus) joint$PValue else fc$PValue
        expand(data.frame(AveExpr = fc$logCPM, log2FoldChange = fc$logFC, pvalue = p,
                          padj = stats::p.adjust(p, "BH"), F = if (omnibus) joint$F else fc$F,
                          row.names = rownames(fc)))
      })
      norm <- edgeR::cpm(x, lib.size = effective, log = TRUE)
    } else {
      v <- if (isTRUE(s$quality_weights)) limma::voomWithQualityWeights(y, fit_design, plot = FALSE) else
        limma::voom(y, fit_design, plot = FALSE)
      fit <- limma::lmFit(v, fit_design)
      joint <- if (omnibus) limma::topTable(limma::eBayes(fit), coef = added, number = Inf, sort.by = "none")
      results <- lapply(contrasts, function(ct) {
        cf <- limma::contrasts.fit(fit, matrix(ct, ncol = 1L))
        fc <- limma::topTable(limma::eBayes(cf), coef = 1L, number = Inf, sort.by = "none")
        if (omnibus) joint <- joint[rownames(fc), , drop = FALSE]
        tab <- data.frame(AveExpr = fc$AveExpr, log2FoldChange = fc$logFC,
                          pvalue = if (omnibus) joint$P.Value else fc$P.Value,
                          padj = if (omnibus) joint$adj.P.Val else fc$adj.P.Val,
                          row.names = rownames(fc))
        if (omnibus) tab$F <- if (length(added) > 1L) joint$F else joint$t^2
        else { tab$t <- fc$t; tab$B <- fc$B }
        expand(tab)
      })
      norm <- log2(sweep(x + 0.5, 2L, effective + 1, "/") * 1e6)
    }
    list(results = results, norm = norm)
  }

  differential <- function(request) {
    s <- request$settings
    samples <- strings(s$samples)
    tab <- get_table(request$table)
    gene_cols <- unique(c(s$gene, s$label))
    if (any(gene_cols %in% samples)) fail("The gene columns cannot be sample columns.")
    for (name in gene_cols) column(tab, name, "gene identifiers")
    x <- de_counts(tab, samples)
    model <- de_model(samples, s$design, s$batch, s$reduced)
    mode <- s$batch$mode
    omnibus <- !is.null(model$r)
    if (omnibus) {
      for (term in setdiff(de_terms(model$full), de_terms(model$reduced))) de_term_replicates(model$data, term)
    }
    contrasts <- lapply(s$outputs, function(out) de_contrast(model, out$contrast))
    packages <- unique(c(switch(s$method, deseq2 = c("DESeq2", if (isTRUE(s$shrink)) "apeglm"),
                                edger = "edgeR", limma = c("edgeR", "limma")),
                         switch(mode, sva = "sva", ruv = c("RUVSeq", "edgeR", "limma"),
                                combat_seq = "sva", combat_ref = c("edgeR", "matrixStats"))))
    need_packages(packages)
    hidden <- ""
    reference <- ""
    if (mode %in% c("sva", "ruv")) {
      w <- de_latent(x, model$m, mode)
      model$m <- cbind(model$m, w)
      if (de_rank(model$m) != ncol(model$m)) fail("The estimated hidden factors cannot be told apart from the design.")
      if (omnibus) model$r <- cbind(model$r, w)
      contrasts <- lapply(contrasts, function(ct) c(ct, rep(0, ncol(w))))
      if (ncol(w)) hidden <- paste0(" + ", paste(colnames(w), collapse = " + "))
    } else if (mode %in% c("combat_seq", "combat_ref")) {
      adjust <- de_with_intercept(model$biology)
      in_every_batch <- Reduce(`&`, lapply(levels(model$batch), function(b) rowSums(x[, model$batch == b, drop = FALSE]) > 0))
      if (sum(in_every_batch) < 2L) fail("ComBat needs at least two genes expressed in every batch.")
      if (mode == "combat_seq") {
        x <- sva::ComBat_seq(x, model$batch, covar_mod = adjust)
      } else {
        x <- combat_ref(x, model$batch, adjust)
        reference <- paste0("; reference batch '", attr(x, "reference_batch"), "' was kept unchanged")
      }
      if (any(!is.finite(x)) || any(colSums(x) == 0)) fail("ComBat did not leave valid counts for every sample.")
    }

    fitted <- de_fit(x, model, contrasts, s)
    batch_text <- switch(mode, none = "none", model = paste("in the model:", s$batch$factor),
                         pairs = paste("pairs or donors in the model:", s$batch$factor),
                         sva = "SVA (svaseq)", ruv = "RUVSeq (RUVg with empirical control genes)",
                         combat_seq = paste("ComBat-seq corrected counts by", s$batch$factor),
                         combat_ref = paste0("ComBat-ref corrected counts by ", s$batch$factor, reference))
    if (mode %in% c("sva", "ruv")) batch_text <- paste0(batch_text, ", ", ncol(w), " estimated factors")
    test <- if (omnibus) switch(s$method, deseq2 = "a likelihood ratio test", edger = "a quasi-likelihood F-test", limma = "a moderated F-test") else
      switch(s$method, deseq2 = "a Wald test", edger = "a quasi-likelihood F-test", limma = "a moderated t-test")
    formula <- paste0(de_formula_text(model$full), hidden)
    reduced <- if (omnibus) paste0(" against ", de_formula_text(model$reduced), hidden) else ""
    method <- switch(s$method, deseq2 = "DESeq2", edger = "edgeR", limma = "limma-voom")
    details <- c(if (!is.null(s$design$numeric)) paste(s$design$numeric, "was treated as a number."),
                 if (!is.null(model$combined)) paste0(model$combined$name, " combined ", model$combined$first, " and ", model$combined$second, "."),
                 if (isTRUE(s$shrink) && s$method == "deseq2") "Displayed log2 fold changes were shrunk with apeglm.",
                 if (identical(fitted$size_factors, "poscounts")) "DESeq2 estimated size factors from positive counts.",
                 if (isTRUE(s$quality_weights) && s$method == "limma") "Voom estimated sample quality weights.",
                 if (omnibus) "P-values test the full design against the reduced design; fold changes show the selected comparison.")
    versions <- paste(vapply(packages, function(pkg) paste(pkg, utils::packageVersion(pkg)), character(1)), collapse = ", ")
    methods <- paste(c(paste0(method, " used ", formula, reduced, " with ", test, ". Batch handling: ", batch_text, "."),
                       details, paste0("R ", getRversion(), "; ", versions, ".")), collapse = " ")

    if (!dir.exists(s$directory)) dir.create(s$directory, recursive = TRUE)
    save <- function(name, values, changed = NULL) {
      values <- as.data.frame(values, check.names = FALSE)
      output <- cbind(tab[gene_cols], values)
      unique_cols <- make.unique(c(names(values), gene_cols))
      names(output) <- c(tail(unique_cols, length(gene_cols)), names(values))
      tryCatch(utils::write.csv(output, file.path(s$directory, paste0(name, ".csv")), row.names = FALSE, na = ""),
               error = function(e) fail("Could not write the differential expression table to the session's results folder."))
      assign(name, output, envir = .GlobalEnv)
      list(table = list(name = name, rows = nrow(output), columns = unname(Map(describe, names(output), output))),
           gene = names(output)[1L], label = if (length(gene_cols) > 1L) names(output)[2L], changed = changed)
    }
    results <- Map(function(out, stats) {
      changed <- stats$padj < s$cutoffs$padj
      if (!omnibus) changed <- changed & abs(stats$log2FoldChange) >= s$cutoffs$lfc
      save(out$name, stats, sum(changed, na.rm = TRUE))
    }, s$outputs, fitted$results)
    results <- c(results, list(save(s$normalized, fitted$norm)))
    list(tables = I(results), facts = list(formula = paste0(formula, reduced), test = test, methods = methods))
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
    omnibus <- isTRUE(s$omnibus)
    lfc <- if (omnibus) 0 else if (is.null(s$cutoffs$lfc)) 1 else s$cutoffs$lfc
    p_cutoff <- if (is.null(s$cutoffs$padj)) 0.05 else s$cutoffs$padj
    if (lfc < 0 || p_cutoff < 0 || p_cutoff > 1) fail("Choose a nonnegative fold change and an adjusted p-value between 0 and 1.")
    significant <- tested & padj < p_cutoff
    up <- which(significant & fc >= lfc & (!omnibus | fc > 0))
    down <- which(significant & fc <= -lfc & fc < lfc)
    best <- function(rows, n) {
      rows[utils::head(order(padj[rows], rows), n)]
    }
    if (plot %in% c("top_genes", "results_heatmap")) {
      hits <- if (omnibus) which(is.finite(padj) & padj < p_cutoff) else c(up, down)
      changed <- best(hits, if (plot == "top_genes") length(hits) else 50L)
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
      expression <- if (!is.null(s$expression_table)) get_table(s$expression_table) else tab
      if (nrow(expression) != nrow(tab)) fail("The expression values no longer match the results table.")
      expr <- sample_values(expression, s)
      corrected <- plot_batch_values(expr$x, s)
      draw_heatmap(z_scores(corrected$x[changed, , drop = FALSE]), expr$group,
                   labels_row = gene_labels(tab, s, changed), title = plot_title("Most significant changed genes", corrected$note),
                   group_name = s$group_name)
      return(list(facts = list(genes = length(changed), samples = ncol(expr$x), batch = corrected$note)))
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
    if (omnibus) facts$omnibus_changed <- sum(is.finite(padj) & padj < p_cutoff)
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

  sample_groups <- function(samples, s) {
    if (!length(s$groups)) return(NULL)
    group <- pair_side(s$groups, 2L)
    index <- match(samples, pair_side(s$groups, 1L))
    if (all(is.na(index))) return(NULL)
    unassigned <- "No group"
    while (unassigned %in% group) unassigned <- paste0(unassigned, " (unassigned)")
    assigned <- group[index]
    assigned[is.na(index)] <- unassigned
    factor(assigned, levels = intersect(unique(c(strings(s$group_levels), assigned)), assigned))
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
      if (library_sizes) return(list(totals = totals, group = sample_groups(samples, s)))
      if (any(totals == 0)) fail("Each sample needs at least one count above zero.")
      x <- log2(sweep(x, 2L, totals / 1e6, "/") + 1)
    } else if (s$values == "normalized") {
      if (any(x < 0)) fail("Normalized values cannot be negative unless they are already logged.")
      x <- log2(x + 1)
    }
    list(x = x, group = sample_groups(samples, s))
  }

  plot_batch_values <- function(x, s) {
    correction <- s$plot_batch
    if (is.null(correction)) return(list(x = x, note = s$batch_note))
    need_packages("limma")
    model <- de_model(colnames(x), correction$design, list(mode = "model", factor = correction$factor))
    list(x = limma::removeBatchEffect(x, batch = model$batch, design = model$biology),
         note = paste0("Plot values corrected for ", correction$factor))
  }

  plot_title <- function(title, note) if (is.null(note)) title else paste(title, note, sep = "\n")
  plot_group_name <- function(name) if (is.null(name)) "Group" else name

  row_variances <- function(x) rowSums((x - rowMeans(x))^2) / (ncol(x) - 1L)

  z_scores <- function(x) {
    x <- x - rowMeans(x)
    spread <- sqrt(rowSums(x^2) / (ncol(x) - 1L))
    spread[spread == 0] <- 1
    x / spread
  }

  draw_heatmap <- function(x, group, labels_row = rownames(x), title, bound = max(1, abs(x)), group_name = NULL) {
    annotation <- if (!is.null(group)) setNames(data.frame(group, row.names = colnames(x)), plot_group_name(group_name))
    colours <- if (!is.null(group)) setNames(list(group_palette(group)), plot_group_name(group_name))
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
          ggplot2::scale_fill_manual(name = plot_group_name(s$group_name), values = group_palette(group))
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
      label <- if (is.null(group)) "Sample" else plot_group_name(s$group_name)
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

    corrected <- plot_batch_values(x, s)
    x <- corrected$x
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
      draw_heatmap(r, group, title = plot_title("Sample correlation", corrected$note), bound = 1, group_name = s$group_name)
      return(list(facts = list(lowest = list(a = colnames(r)[pairs[i, 2L]], b = colnames(r)[pairs[i, 1L]],
                                             r = round(r[pairs[i, , drop = FALSE]], 3L)), batch = corrected$note)))
    }

    selected <- utils::head(order(variance, decreasing = TRUE), if (plot == "pca") 500L else 50L)
    if (plot == "variable_heatmap") {
      draw_heatmap(z_scores(x[selected, , drop = FALSE]), group,
                   labels_row = gene_labels(tab, s, selected), title = plot_title("Most variable genes", corrected$note),
                   group_name = s$group_name)
      return(list(facts = list(genes = length(selected), samples = ncol(x), batch = corrected$note)))
    }

    if (length(selected) < 2L || !any(variance > 0)) fail("PCA needs at least two genes and some variation across samples.")
    pc <- stats::prcomp(t(x[selected, , drop = FALSE]), rank. = 2L)
    percent <- round(100 * pc$sdev[1:2]^2 / sum(pc$sdev^2), 1L)
    d <- data.frame(sample = colnames(x), pc1 = pc$x[, 1L], pc2 = pc$x[, 2L])
    p <- ggplot2::ggplot(d, ggplot2::aes(x = pc1, y = pc2)) +
      ggplot2::labs(x = sprintf("PC1 (%.1f%%)", percent[1L]), y = sprintf("PC2 (%.1f%%)", percent[2L]), subtitle = corrected$note)
    if (is.null(group)) p <- p + ggplot2::geom_point(colour = col_down, size = 2.6) else {
      d$group <- group
      p <- p + ggplot2::geom_point(data = d, ggplot2::aes(colour = group), size = 2.6) +
        ggplot2::scale_colour_manual(name = plot_group_name(s$group_name), values = group_palette(group))
    }
    p <- p + ggrepel::geom_text_repel(ggplot2::aes(label = sample), max.overlaps = Inf, seed = 42L, size = 3)
    finish_plot(p, list(pc1 = percent[1L], pc2 = percent[2L], genes = length(selected), batch = corrected$note))
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
    if (request$action == "combine_counts") return(combine_counts(request))
    if (request$action == "symbols") return(add_symbols(request))
    if (request$action == "factors") return(sample_factors(request))
    if (request$action == "de_coefficients") {
      model <- de_model(strings(request$samples), request$design, request$batch)
      return(list(coefficients = I(colnames(model$m)[attr(model$m, "assign") != 0L])))
    }
    if (request$action == "differential") return(differential(request))
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
