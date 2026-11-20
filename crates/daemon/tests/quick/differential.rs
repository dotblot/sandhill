use std::path::{Path, PathBuf};

use sandhill_proto::{
    Call, QuickComparison, QuickDeBatch, QuickDeContrast, QuickDeDesign, QuickDeMethod,
    QuickDeOutput, QuickDeSettings, QuickFactor, QuickPlot, QuickRequest, QuickResult,
    QuickSettings, QuickTableKind, QuickValues, SessionInfo,
};
use tokio::process::Command;

use super::{Client, common, start};

#[tokio::test]
async fn real_counts_match_the_three_packages_and_refuse_invalid_designs() {
    if sandhill_daemon::kernel::find_kernelspec("ir", None)
        .await
        .is_err()
    {
        eprintln!("skipping differential expression: the IRkernel kernel is not installed");
        return;
    }
    let r = std::env::var_os("SANDHILL_TEST_RSCRIPT").map_or_else(
        || {
            if Path::new("/opt/homebrew/bin/Rscript").is_file() {
                PathBuf::from("/opt/homebrew/bin/Rscript")
            } else {
                PathBuf::from("Rscript")
            }
        },
        PathBuf::from,
    );
    let script = "quit(status = if (all(vapply(c('jsonlite', 'DESeq2', 'edgeR', 'limma', 'sva'), requireNamespace, logical(1), quietly=TRUE))) 0L else 44L)";
    match Command::new(&r).args(["-e", script]).status().await {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping differential expression: Rscript was not found");
            return;
        }
        Ok(status) if status.code() == Some(44) => {
            eprintln!(
                "skipping differential expression: real R needs jsonlite, DESeq2, edgeR, limma and sva"
            );
            return;
        }
        result => assert!(result.unwrap().success()),
    }
    let real = std::env::var_os("SANDHILL_SAMPLE_REAL")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../sandhill-sample-real"),
            PathBuf::from,
        )
        .join("data/gse159851");
    let counts = real.join("GSE159851_Schaffenrath_RNAseq_GBM_BM_control.csv.gz");
    let samples_path = real.join("samples.tsv");
    if !counts.is_file() || !samples_path.is_file() {
        eprintln!(
            "skipping differential expression: the GSE159851 fixture is not in {} (set SANDHILL_SAMPLE_REAL)",
            real.display()
        );
        return;
    }
    let dir = common::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let invalid = dir.path().join("unnormalized.csv");
    std::fs::write(&invalid, "gene,A,B,C,D\ngene_1,1.2,2,3,4\n").unwrap();
    let (_daemon, socket) = start(&state).await;
    let client = Client::connect_unix(&socket).await.unwrap();
    let folder: String = client
        .call(Call::QuickStart {
            tables: [&counts, &samples_path, &invalid]
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        })
        .await
        .unwrap();
    let session: SessionInfo = client
        .call(Call::StartSession {
            kernel: "ir".into(),
            cwd: Some(folder.clone()),
            project: Some(folder.clone()),
        })
        .await
        .unwrap();
    let run = |request: QuickRequest| {
        client.call::<QuickResult>(Call::QuickRun {
            session: session.id.clone(),
            project: folder.clone(),
            request,
        })
    };
    for (file, name) in [
        (&counts, "counts"),
        (&samples_path, "samples"),
        (&invalid, "invalid"),
    ] {
        let result = run(QuickRequest::Load {
            file: file.file_name().unwrap().to_string_lossy().into_owned(),
            sheet: None,
            name: name.into(),
        })
        .await
        .unwrap();
        let table = result.table.unwrap();
        assert!(table.rows > 0);
        if name == "counts" {
            assert_eq!(table.recognition.kind, QuickTableKind::Counts);
            assert_eq!(table.recognition.samples.len(), 17);
        }
    }
    let factors = run(QuickRequest::Factors {
        table: "samples".into(),
        key: "sample".into(),
    })
    .await
    .unwrap()
    .factors;
    let [condition] = factors.as_slice() else {
        panic!("Expected one condition factor: {factors:?}");
    };
    assert_eq!(condition.levels, ["control", "GBM", "metastasis"]);
    let samples: Vec<_> = condition
        .assignments
        .iter()
        .map(|(s, _)| s.clone())
        .collect();
    assert_eq!(
        condition
            .levels
            .iter()
            .map(|level| condition
                .assignments
                .iter()
                .filter(|(_, l)| l == level)
                .count())
            .collect::<Vec<_>>(),
        [6, 5, 6]
    );
    let batches = QuickFactor::from_pairs(
        "batch".into(),
        samples
            .iter()
            .enumerate()
            .map(|(i, sample)| {
                (
                    sample.clone(),
                    if i % 2 == 0 { "one" } else { "two" }.into(),
                )
            })
            .collect(),
    );
    let design = QuickDeDesign {
        formula: "~ condition".into(),
        factors: vec![condition.clone(), batches],
        numeric: None,
        combined: None,
    };
    let comparison = QuickComparison {
        higher: "GBM".into(),
        lower: "control".into(),
    };
    let output = |name: &str| QuickDeOutput {
        name: name.into(),
        comparison: comparison.clone(),
        contrast: QuickDeContrast::Levels {
            factor: "condition".into(),
            higher: "GBM".into(),
            lower: "control".into(),
            at: vec![],
        },
    };
    let make = |method, name: &str| QuickDeSettings {
        method,
        design: design.clone(),
        reduced: None,
        batch: QuickDeBatch::None,
        gene: "gene_ID".into(),
        label: None,
        samples: samples.clone(),
        cutoffs: Default::default(),
        outputs: vec![output(name)],
        normalized: format!("{name}_normalized"),
        directory: "results".into(),
        shrink: false,
        quality_weights: false,
    };
    let analyze = |settings| {
        run(QuickRequest::Differential {
            table: "counts".into(),
            settings: Box::new(settings),
        })
    };
    for (method, name) in [
        (QuickDeMethod::Deseq2, "deseq"),
        (QuickDeMethod::Edger, "edger"),
        (QuickDeMethod::Limma, "voom"),
    ] {
        let result = analyze(make(method, name))
            .await
            .unwrap_or_else(|err| panic!("{name}: {err:#}"));
        let [de, normalized] = result.differential.as_slice() else {
            panic!("Expected results and expression tables")
        };
        assert_eq!(de.table.name, name);
        assert_eq!(de.table.recognition.kind, QuickTableKind::Results);
        assert_eq!(de.info.comparison.as_ref().unwrap().value, comparison);
        assert_eq!(de.gene, "gene_ID");
        assert_eq!(de.table.recognition.fc.as_deref(), Some("log2FoldChange"));
        assert_eq!(de.table.recognition.padj.as_deref(), Some("padj"));
        assert!(de.info.methods.as_ref().unwrap().contains(method.label()));
        assert!(
            de.info
                .methods
                .as_ref()
                .unwrap()
                .contains("Batch handling: none")
        );
        assert_eq!(
            normalized.table.recognition.kind,
            QuickTableKind::Normalized
        );
        assert_eq!(
            normalized.table.recognition.values,
            Some(QuickValues::Logged)
        );
        assert_eq!(normalized.samples, samples);
        assert!(result.sentence.unwrap().contains("GBM vs control"));
    }
    let names = run(QuickRequest::DeCoefficients {
        design: Box::new(design.clone()),
        samples: samples.clone(),
        batch: QuickDeBatch::Model {
            factor: "batch".into(),
        },
    })
    .await
    .unwrap()
    .coefficients;
    assert_eq!(names, ["batchtwo", "conditionGBM", "conditionmetastasis"]);

    for (method, name) in [
        (QuickDeMethod::Deseq2, "lrt"),
        (QuickDeMethod::Edger, "edge_f"),
        (QuickDeMethod::Limma, "voom_f"),
    ] {
        let mut omnibus = make(method, name);
        omnibus.reduced = Some("~ 1".into());
        let result = analyze(omnibus)
            .await
            .unwrap_or_else(|err| panic!("{name}: {err:#}"));
        assert_eq!(result.facts["formula"], "~condition against ~1");
        assert!(result.differential[0].info.omnibus);
    }

    let mut shrunk = make(QuickDeMethod::Deseq2, "shrunk_lrt");
    shrunk.reduced = Some("~ 1".into());
    shrunk.shrink = true;
    shrunk.outputs[0].comparison = QuickComparison {
        higher: "metastasis".into(),
        lower: "GBM".into(),
    };
    shrunk.outputs[0].contrast = QuickDeContrast::Levels {
        factor: "condition".into(),
        higher: "metastasis".into(),
        lower: "GBM".into(),
        at: vec![],
    };
    match analyze(shrunk).await {
        Err(err) if format!("{err:#}").contains("Missing R packages: apeglm") => {
            eprintln!("skipping apeglm: {err:#}")
        }
        result => assert!(
            result.unwrap().differential[0]
                .info
                .methods
                .as_ref()
                .unwrap()
                .contains("shrunk with apeglm")
        ),
    }

    let mut quality = make(QuickDeMethod::Limma, "quality_weights");
    quality.quality_weights = true;
    assert!(
        analyze(quality).await.unwrap().differential[0]
            .info
            .methods
            .as_ref()
            .unwrap()
            .contains("quality weights")
    );

    let mut interaction = make(QuickDeMethod::Limma, "interaction");
    interaction.design.formula = "~ condition * batch".into();
    interaction.reduced = Some("~ condition + batch".into());
    interaction.outputs[0].contrast = QuickDeContrast::Interaction {
        first: "condition".into(),
        second: "batch".into(),
        first_higher: "GBM".into(),
        second_higher: "two".into(),
    };
    assert!(
        analyze(interaction).await.unwrap().differential[0]
            .info
            .omnibus
    );

    let mut in_model = make(QuickDeMethod::Edger, "modeled");
    in_model.batch = QuickDeBatch::Model {
        factor: "batch".into(),
    };
    let result = analyze(in_model.clone()).await.unwrap();
    assert_eq!(result.facts["formula"], "~batch + condition");
    let plot_batch = result.differential[1].info.plot_batch.clone();
    assert!(plot_batch.is_some());
    let plot = run(QuickRequest::Plot {
        plot: QuickPlot::Pca,
        table: "modeled_normalized".into(),
        output: None,
        settings: Box::new(QuickSettings {
            samples: samples.clone(),
            values: Some(QuickValues::Logged),
            plot_batch,
            ..Default::default()
        }),
    })
    .await;
    match plot {
        Err(err) if format!("{err:#}").contains("Missing R packages") => {
            eprintln!("skipping batch PCA: {err:#}")
        }
        result => assert_eq!(
            result.unwrap().facts["batch"],
            "Plot values corrected for batch"
        ),
    }
    for (batch, name) in [
        (
            QuickDeBatch::CombatSeq {
                factor: "batch".into(),
            },
            "combat_seq",
        ),
        (
            QuickDeBatch::CombatRef {
                factor: "batch".into(),
            },
            "combat_ref",
        ),
        (QuickDeBatch::Sva, "sva"),
        (QuickDeBatch::Ruv, "ruv"),
    ] {
        let mut settings = make(QuickDeMethod::Edger, name);
        settings.batch = batch;
        match analyze(settings).await {
            Err(err)
                if name == "ruv" && format!("{err:#}").contains("Missing R packages: RUVSeq") =>
            {
                eprintln!("skipping RUVg: {err:#}")
            }
            result => {
                let result = result.unwrap_or_else(|err| panic!("{name}: {err:#}"));
                assert!(
                    !result.facts["formula"]
                        .as_str()
                        .unwrap()
                        .contains("batch +")
                );
                assert_eq!(result.differential.len(), 2);
            }
        }
    }
    let mut confounded = in_model.clone();
    confounded.design.factors[1] = QuickFactor {
        name: "batch".into(),
        ..condition.clone()
    };
    let err = analyze(confounded).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("cannot be told apart"),
        "{err:#}"
    );

    let mut singleton = make(QuickDeMethod::Edger, "singleton");
    singleton.batch = QuickDeBatch::CombatSeq {
        factor: "batch".into(),
    };
    singleton.design.factors[1].levels.push("alone".into());
    singleton.design.factors[1].assignments[0].1 = "alone".into();
    let err = analyze(singleton).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("at least two samples in each batch"),
        "{err:#}"
    );

    let mut too_few = make(QuickDeMethod::Limma, "one_sample");
    too_few.design.factors[0]
        .assignments
        .iter_mut()
        .filter(|(_, level)| level == "GBM")
        .skip(1)
        .for_each(|(_, level)| *level = "control".into());
    let err = analyze(too_few).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("'GBM' needs at least two samples"),
        "{err:#}"
    );

    let mut no_rank = make(QuickDeMethod::Edger, "bad_rank");
    no_rank.design.formula = "~ condition + clone".into();
    no_rank.design.factors.push(QuickFactor {
        name: "clone".into(),
        ..condition.clone()
    });
    let err = analyze(no_rank).await.unwrap_err();
    assert!(format!("{err:#}").contains("not of full rank"), "{err:#}");

    let mut decimal = make(QuickDeMethod::Edger, "normalized_as_counts");
    decimal.samples = ["A", "B", "C", "D"]
        .into_iter()
        .map(str::to_string)
        .collect();
    decimal.gene = "gene".into();
    let err = run(QuickRequest::Differential {
        table: "invalid".into(),
        settings: Box::new(decimal),
    })
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("whole numbers"), "{err:#}");

    let steps = std::fs::read_to_string(Path::new(&folder).join("steps.R")).unwrap();
    assert!(steps.contains(r#""action":"differential""#));
    assert!(steps.contains(r#""directory":"results""#));
    assert!(!steps.contains("bad_rank"));
    let reference = Command::new(r)
        .arg("-e")
        .arg(DIRECT)
        .args([counts.as_os_str(), samples_path.as_os_str()])
        .current_dir(&folder)
        .output()
        .await
        .unwrap();
    assert!(
        reference.status.success(),
        "Direct-package comparison failed:\n{}\n{}",
        String::from_utf8_lossy(&reference.stdout),
        String::from_utf8_lossy(&reference.stderr)
    );
}

// Independent examples in each package's own API, not Sandhill's helpers.
const DIRECT: &str = r#"
paths <- commandArgs(trailingOnly=TRUE)
from <- read.csv(paths[[1L]], row.names=1L, check.names=FALSE)
sample <- read.delim(paths[[2L]], row.names=1L)
condition <- factor(sample[colnames(from), 'condition'], levels=c('control','GBM','metastasis'))
coldata <- data.frame(condition, row.names=colnames(from))
x <- as.matrix(from)
m <- model.matrix(~ condition, coldata)
contrast <- c(0,1,0)
compare_top <- function(name, p) {
  made <- read.csv(file.path('results',paste0(name,'.csv')), row.names=1L)
  take <- function(nms, p) head(nms[order(p, na.last=NA)], 100L)
  hits <- length(intersect(take(rownames(made), made$pvalue), take(names(p), p)))
  message(name, ': ', hits, '/100 overlapping top genes')
  stopifnot(hits >= 95L)
  invisible(made)
}
dds <- DESeq2::DESeqDataSetFromMatrix(x, coldata, ~ condition)
fit <- DESeq2::DESeq(dds, quiet=TRUE)
w <- DESeq2::results(fit, contrast=c('condition','GBM','control'), alpha=.05)
compare_top('deseq', setNames(w$pvalue, rownames(w)))
lrt <- DESeq2::results(DESeq2::DESeq(dds, test='LRT', reduced=~1, quiet=TRUE))
made <- compare_top('lrt', setNames(lrt$pvalue, rownames(lrt)))
keep <- is.finite(made$log2FoldChange) & is.finite(w$log2FoldChange)
stopifnot(cor(made$log2FoldChange[keep], w$log2FoldChange[keep]) > .99)
y <- edgeR::DGEList(x)
y <- edgeR::normLibSizes(y[edgeR::filterByExpr(y, design=m),,keep.lib.sizes=FALSE])
fitted <- edgeR::glmQLFit(edgeR::estimateDisp(y, m, robust=TRUE), m, robust=TRUE)
q <- edgeR::glmQLFTest(fitted, contrast=contrast)$table
compare_top('edger', setNames(q$PValue, rownames(q)))
f <- edgeR::glmQLFTest(fitted, coef=2:3)$table
compare_top('edge_f', setNames(f$PValue, rownames(f)))
v <- limma::voom(y, m)
lm <- limma::lmFit(v, m)
ct <- limma::eBayes(limma::contrasts.fit(lm, matrix(contrast, ncol=1L)))
t <- limma::topTable(ct, coef=1L, number=Inf, sort.by='none')
compare_top('voom', setNames(t$P.Value, rownames(t)))
f <- limma::topTable(limma::eBayes(lm), coef=2:3, number=Inf, sort.by='none')
compare_top('voom_f', setNames(f$P.Value, rownames(f)))
"#;
