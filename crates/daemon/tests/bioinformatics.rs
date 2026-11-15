//! Real selected-kernel checks; missing kernels/packages are explicitly skipped.
mod common;

use sandhill_daemon::{
    Daemon, StateDir,
    bio::{checks, guides},
};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, EVENT_RUN_FINISHED, Event, SessionInfo};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{net::UnixListener, task::JoinHandle};

struct Harness {
    daemon: Arc<Daemon>,
    socket: PathBuf,
    server: JoinHandle<()>,
}
impl Harness {
    async fn open(path: &Path) -> Self {
        let daemon = Daemon::open(StateDir::new(path)).await.unwrap();
        let socket = path.join("bio.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn({
            let daemon = daemon.clone();
            async move {
                daemon.serve(listener).await.unwrap();
            }
        });
        Self {
            daemon,
            socket,
            server,
        }
    }
    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
        self.daemon.detach_all();
    }
}

fn design_spec() -> Value {
    json!({"workflow":"study_design","guide_id":"bio-study-design","guide_digest":guides::digest("bio-study-design").unwrap(),"checker_digest":checks::digest("design"),"goal":"Assess feasibility, not assert biological correctness","design":{},"parameters":{}})
}
fn id(value: &Value) -> &str {
    value["analysis"]["id"].as_str().unwrap()
}
fn item<'a>(value: &'a Value, name: &str) -> &'a Value {
    value["latest_check"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == name)
        .unwrap_or_else(|| panic!("missing {name}: {value}"))
}
async fn check(daemon: &Daemon, root: &Path, id: &str, checkpoint: &str, details: Value) -> Value {
    daemon
        .check_analysis(root.to_str().unwrap(), id, checkpoint, details)
        .await
        .unwrap()
}
async fn run(client: &mut Client, session: &str, code: &str) -> (String, String, String) {
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, code, None).await.unwrap().run;
    let mut stdout = String::new();
    loop {
        let event: Event = tokio::time::timeout(Duration::from_secs(120), client.next_event())
            .await
            .unwrap()
            .unwrap();
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        if event.msg_type == "stream" {
            stdout.push_str(event.content["text"].as_str().unwrap_or(""));
        }
        if event.msg_type == "error" {
            stdout.push_str(&event.content.to_string());
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            return (
                stdout,
                event.content["status"].as_str().unwrap().into(),
                run,
            );
        }
    }
}
async fn session(harness: &Harness, root: &Path, language: &str) -> Option<(Client, String)> {
    let specs = sandhill_daemon::kernel::list_kernelspecs(None).await;
    let mut candidates: Vec<_> = specs
        .into_iter()
        .filter(|spec| sandhill_daemon::cells::normalize_language(&spec.language) == language)
        .collect();
    candidates.sort_by_key(|spec| !spec.name.to_lowercase().contains("omicverse"));
    let kernel = candidates.into_iter().next();
    let Some(kernel) = kernel else {
        eprintln!("skipping {language}: kernel not installed");
        return None;
    };
    let mut client = harness.client().await;
    let info: SessionInfo = client
        .call(Call::StartSession {
            kernel: kernel.name,
            cwd: Some(root.to_string_lossy().into()),
            project: Some(root.to_string_lossy().into()),
        })
        .await
        .unwrap();
    let probe = if language == "python" {
        "import importlib.util\nprint('BIO_PACKAGES_OK' if all(importlib.util.find_spec(p) is not None for p in ['anndata','numpy','pandas','scipy','h5py','omicverse','scanpy','igraph','umap']) else 'BIO_PACKAGES_MISSING')"
    } else {
        "cat(if (all(vapply(c('Seurat','SeuratObject','Matrix','jsonlite','RANN','uwot'),requireNamespace,logical(1),quietly=TRUE))) 'BIO_PACKAGES_OK' else 'BIO_PACKAGES_MISSING')"
    };
    let (out, status, _) = run(&mut client, &info.id, probe).await;
    if status != "ok" || !out.contains("BIO_PACKAGES_OK") {
        eprintln!(
            "skipping {language}: required checker packages unavailable in selected kernel: {out}"
        );
        client.shutdown(&info.id).await.unwrap();
        return None;
    }
    Some((client, info.id))
}

#[tokio::test]
async fn design_is_kernel_free_owned_historical_and_journal_authoritative() {
    let state = common::tempdir().unwrap();
    let project = common::tempdir().unwrap();
    let other = common::tempdir().unwrap();
    let harness = Harness::open(state.path()).await;
    let record = harness
        .daemon
        .begin_analysis(project.path().to_str().unwrap(), design_spec())
        .await
        .unwrap();
    assert_eq!(record["analysis"]["language"], "design");
    assert!(record["analysis"].get("baseline").is_none());
    let checked = check(
        &harness.daemon,
        project.path(),
        id(&record),
        "design",
        json!({}),
    )
    .await;
    assert_eq!(item(&checked, "design_experimental_unit")["status"], "fail");
    assert_eq!(item(&checked, "source_identifiers")["status"], "fail");
    assert!(checked["latest_check"]["run_id"].is_null());
    assert_eq!(checked["latest_check"]["technical_pass"], false);
    assert!(
        harness
            .daemon
            .get_analysis(other.path().to_str().unwrap(), id(&record))
            .await
            .is_err()
    );
    assert!(
        harness
            .daemon
            .check_analysis(
                project.path().to_str().unwrap(),
                id(&record),
                "input",
                json!({})
            )
            .await
            .is_err()
    );
    let exported = project.path().join(".sandhill/analysis").join(id(&record));
    std::fs::write(
        exported.join("checks").join(format!(
            "{}.json",
            checked["latest_check"]["id"].as_str().unwrap()
        )),
        b"{\"technical_pass\":true}",
    )
    .unwrap();
    let historical = harness
        .daemon
        .get_analysis(project.path().to_str().unwrap(), id(&record))
        .await
        .unwrap();
    assert_eq!(historical["latest_check"]["technical_pass"], false);
    assert_eq!(historical["validity"]["current"], false);
    let retry = check(
        &harness.daemon,
        project.path(),
        id(&record),
        "design",
        json!({}),
    )
    .await;
    assert_eq!(retry["export"]["saved"], false);
    assert!(
        retry["export"]["error"]
            .as_str()
            .unwrap()
            .contains("preserved")
    );
    assert_eq!(
        std::fs::read(exported.join("checks").join(format!(
            "{}.json",
            checked["latest_check"]["id"].as_str().unwrap()
        )))
        .unwrap(),
        b"{\"technical_pass\":true}"
    );
    let mut stale = design_spec();
    stale["checker_digest"] = json!("sha256:old");
    assert!(
        harness
            .daemon
            .begin_analysis(project.path().to_str().unwrap(), stale)
            .await
            .unwrap_err()
            .to_string()
            .contains("restart")
    );
    let client = harness.client().await;
    let _: Value = client
        .call(Call::StopResources { force: false })
        .await
        .unwrap();
    let _: Value = client
        .call(Call::CleanStorage {
            categories: vec!["history".into()],
        })
        .await
        .unwrap();
    assert!(
        harness
            .daemon
            .get_analysis(project.path().to_str().unwrap(), id(&record))
            .await
            .is_err()
    );
    assert!(exported.join("manifest.json").exists());
}

#[tokio::test]
async fn exports_reject_symlink_and_hardlink_targets() {
    let state = common::tempdir().unwrap();
    let root = common::tempdir().unwrap();
    let outside = common::tempdir().unwrap();
    let harness = Harness::open(state.path()).await;
    std::fs::create_dir(root.path().join(".sandhill")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join(".sandhill/analysis")).unwrap();
    let record = harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), design_spec())
        .await
        .unwrap();
    assert_eq!(record["export"]["saved"], false);
    assert!(!outside.path().join(id(&record)).exists());
    std::fs::remove_file(root.path().join(".sandhill/analysis")).unwrap();
    let checked = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "design",
        json!({}),
    )
    .await;
    assert_eq!(checked["export"]["saved"], true);
    let report = root
        .path()
        .join(".sandhill/analysis")
        .join(id(&record))
        .join("report.md");
    let alias = outside.path().join("report-alias");
    std::fs::hard_link(&report, &alias).unwrap();
    let old = std::fs::read(&alias).unwrap();
    let checked = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "design",
        json!({}),
    )
    .await;
    assert_eq!(checked["export"]["saved"], false);
    assert_eq!(std::fs::read(alias).unwrap(), old);
}

#[test]
fn checker_contract_rejects_missing_duplicate_and_invalid_status_items() {
    for bad in [
        json!({"items":[]}),
        json!({"items":[{"id":"object","status":"pass","message":"ok"},{"id":"object","status":"pass","message":"duplicate"}]}),
        json!({"items":[{"id":"object","status":"made_up","message":"bad"}]}),
    ] {
        let items = checks::items(&bad, "python", "input");
        assert!(
            items
                .iter()
                .any(|i| i.status == sandhill_proto::analysis::CheckStatus::NotChecked)
        );
    }
    let script = checks::code("python", &json!({"object":"x; malicious()"}), "token").unwrap();
    assert!(!script.contains("x; malicious()"));
}

fn scrna_spec(language: &str, session: &str, source: &Path) -> Value {
    let guide = if language == "python" {
        "scrna-scanpy"
    } else {
        "scrna-seurat"
    };
    let counts = if language == "python" {
        json!({"kind":"raw_counts","processing_state":"raw","provenance":"Synthetic integer counts saved before preprocessing","layer":"counts"})
    } else {
        json!({"kind":"raw_counts","processing_state":"raw","provenance":"Synthetic integer counts saved before preprocessing","assay":"RNA","seurat_layer":"counts"})
    };
    json!({"workflow":"scrna","guide_id":guide,"guide_digest":guides::digest(guide).unwrap(),"checker_digest":checks::digest(language),"goal":"Verify synthetic technical structure; not discover biological identities","session":session,"object":"fixture","counts_source":counts,"species":"human","gene_id_type":"symbol","source_files":[source],"documents":[],"design":{"biological_replicates":1},"parameters":{"seed":7,"tissue":"pbmc"}})
}

fn qc_details(root: &Path, language: &str) -> Value {
    let columns = if language == "python" {
        json!({"total_counts":"total_counts","n_genes":"n_genes_by_counts","pct_mito":"pct_counts_mt"})
    } else {
        json!({"total_counts":"nCount_RNA","n_genes":"nFeature_RNA","pct_mito":"percent.mt"})
    };
    json!({"qc_metrics":columns,"qc_thresholds":[{"metric":"total_counts","min":1,"max":1000,"reason":"Synthetic libraries intentionally retained"},{"metric":"n_genes","min":1,"max":16,"reason":"All known synthetic genes retained"},{"metric":"pct_mito","min":0,"max":100,"reason":"Synthetic exact metric comparison, not a general biological cutoff"}],"qc_exclusions_file":root.join("exclusions.csv"),"doublets":{"status":"not_performed","reason":"Synthetic cell identities have no independently identifiable multiplets; not claiming exclusion"}})
}

#[tokio::test]
async fn real_seurat_all_stages_same_path_damage_busy_and_freshness() {
    let state = common::tempdir().unwrap();
    let root = common::tempdir().unwrap();
    let harness = Harness::open(state.path()).await;
    let Some((mut client, session)) = session(&harness, root.path(), "r").await else {
        return;
    };
    let raw = root.path().join("raw.rds");
    let setup = r#"
genes <- c('MS4A1','CD79A','CD74','CD37','CD3D','CD3E','TRAC','IL7R','LYZ','S100A8','NKG7','GNLY','MT-CO1','MT-ND1','RPL3','RPS3')
x <- matrix(0,nrow=16,ncol=40,dimnames=list(genes,paste0('cell',seq_len(40))))
for(j in seq_len(40)) {
 x[1:4,j] <- if(j<=20) c(12,9,7,6)+(j%%3) else c(0,1,1,0)
 x[5:8,j] <- if(j>20) c(12,9,7,6)+(j%%3) else c(0,1,1,0)
 x[9:12,j] <- c(j%%2,(j+1)%%2,j%%3,(j+2)%%3)
 x[13:16,j] <- c(1,1,5+(j%%4),4+(j%%3))
}
fixture <- SeuratObject::CreateSeuratObject(Matrix::Matrix(x,sparse=TRUE),assay='RNA',min.cells=0,min.features=0)
fixture$percent.mt <- Seurat::PercentageFeatureSet(fixture,pattern='^MT-',assay='RNA')
saveRDS(fixture,'raw.rds')
write.csv(data.frame(axis=character(),id=character(),reason=character()),'exclusions.csv',row.names=FALSE)
set.seed(7)
read_only_before <- serialize(list(object=fixture,rng=.Random.seed,cwd=getwd(),options=options()),NULL)
"#;
    let (out, status, _) = run(&mut client, &session, setup).await;
    assert_eq!(status, "ok", "{out}");
    let spec = scrna_spec("r", &session, &raw);
    let record = harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), spec.clone())
        .await
        .unwrap();
    let input = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "input",
        json!({}),
    )
    .await;
    assert_eq!(input["latest_check"]["technical_pass"], true, "{input}");
    assert_eq!(item(&input, "source_correspondence")["status"], "pass");
    let (out,status,_)=run(&mut client,&session,"stopifnot(identical(read_only_before,serialize(list(object=fixture,rng=.Random.seed,cwd=getwd(),options=options()),NULL)))").await;
    assert_eq!(status, "ok", "{out}");
    let qc = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "qc",
        qc_details(root.path(), "r"),
    )
    .await;
    assert_eq!(qc["latest_check"]["technical_pass"], true, "{qc}");
    let mut selected_threshold = qc_details(root.path(), "r");
    selected_threshold["qc_thresholds"] = json!([{"metric":"n_genes","min":1,"max":16,"reason":"Selected fixture criterion; all three QC metrics are still measured"}]);
    let selected = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "qc",
        selected_threshold,
    )
    .await;
    assert_eq!(
        item(&selected, "qc_thresholds")["status"],
        "pass",
        "{selected}"
    );
    assert_eq!(
        item(&selected, "qc_metrics")["status"],
        "pass",
        "{selected}"
    );
    let processing = r#"
fixture <- Seurat::NormalizeData(fixture,verbose=FALSE)
fixture <- Seurat::FindVariableFeatures(fixture,nfeatures=12,verbose=FALSE)
fixture <- Seurat::ScaleData(fixture,features=rownames(fixture),verbose=FALSE)
fixture <- Seurat::RunPCA(fixture,features=rownames(fixture),npcs=5,seed.use=7,verbose=FALSE)
fixture <- Seurat::FindNeighbors(fixture,reduction='pca',dims=1:5,k.param=5,nn.method='rann',verbose=FALSE)
fixture <- Seurat::FindClusters(fixture,resolution=0.2,algorithm=1,random.seed=7,verbose=FALSE)
fixture <- Seurat::RunUMAP(fixture,reduction='pca',dims=1:5,n.neighbors=5,min.dist=0.3,seed.use=7,verbose=FALSE)
markers <- Seurat::FindAllMarkers(fixture,assay='RNA',slot='data',test.use='wilcox',only.pos=FALSE,min.pct=0,logfc.threshold=0,return.thresh=1,verbose=FALSE)
stopifnot(nrow(markers)>0)
expr <- SeuratObject::LayerData(fixture,assay='RNA',layer='data')
cl <- as.character(fixture$seurat_clusters)
for(i in seq_len(nrow(markers))) {
 v <- as.numeric(expr[markers$gene[i],cl==as.character(markers$cluster[i])])
 markers$pct_expr[i] <- mean(v>0); markers$mean_expr[i] <- mean(v)
}
write.csv(markers[,c('cluster','gene','p_val','p_val_adj','pct_expr','mean_expr')],'markers.csv',row.names=FALSE)
annotation <- do.call(rbind,lapply(unique(cl),function(k) {
 rows <- markers[as.character(markers$cluster)==k,,drop=FALSE]
 rows <- rows[!duplicated(rows$gene),,drop=FALSE];stopifnot(nrow(rows)>=2)
 rows <- rows[1:2,,drop=FALSE]
 data.frame(cluster=k,label='pending',gene=rows$gene,pct_expr=rows$pct_expr,mean_expr=rows$mean_expr,evidence_type='support',source='Synthetic fixture; not an external biological reference',status='pending',reason='No-conflict assessment in synthetic data; awaiting independent human review')
}))
write.csv(annotation,'annotations.csv',row.names=FALSE)
keys <- list(pca=c('npcs'),neighbors=c('dims','reduction','k.param','nn.method'),clustering=c('resolution','algorithm'),umap=c('dims','reduction','n.neighbors','min.dist'))
prefixes <- c(pca='RunPCA',neighbors='FindNeighbors',clustering='FindClusters',umap='RunUMAP')
pe <- lapply(names(keys),function(stage) {
 command <- names(fixture@commands)[startsWith(names(fixture@commands),prefixes[[stage]])]
 stopifnot(length(command)==1)
 list(command=command,parameters=fixture@commands[[command]]@params[keys[[stage]]],seed=7)
});names(pe)<-names(keys)
saveRDS(fixture,'result.rds')
details <- list(result_file=file.path(getwd(),'result.rds'),cluster_key='seurat_clusters',embedding_key='umap',marker_file=file.path(getwd(),'markers.csv'),annotation_file=file.path(getwd(),'annotations.csv'),expression_assay='RNA',expression_layer='data',marker_method='Seurat::FindAllMarkers(test.use=wilcox)',p_adjust_method='bonferroni',parameter_evidence=pe)
cat('BIO_DETAILS:',as.character(jsonlite::toJSON(details,auto_unbox=TRUE)), '\n',sep='')
read_only_before <- serialize(list(object=fixture,rng=.Random.seed,cwd=getwd(),options=options()),NULL)
"#;
    let (out, status, execution) = run(&mut client, &session, processing).await;
    assert_eq!(status, "ok", "{out}");
    let details_text = out
        .lines()
        .find_map(|line| line.strip_prefix("BIO_DETAILS:"))
        .expect("actual result details");
    let mut details: Value = serde_json::from_str(details_text).unwrap();
    details["execution_run_ids"] = json!([execution]);
    let result = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        details.clone(),
    )
    .await;
    assert_eq!(result["latest_check"]["technical_pass"], true, "{result}");
    assert_eq!(item(&result, "result_reload")["status"], "pass");
    assert_eq!(
        result["validity"]["scientific_validity"],
        "awaiting_human_review"
    );
    let (out,status,_)=run(&mut client,&session,"stopifnot(identical(read_only_before,serialize(list(object=fixture,rng=.Random.seed,cwd=getwd(),options=options()),NULL)))").await;
    assert_eq!(status, "ok", "{out}");
    let negative_layer = r#"
original_result <- fixture
custom <- SeuratObject::LayerData(fixture,assay='RNA',layer='data')
uncited <- setdiff(rownames(custom),unique(annotation$gene))[1]
stopifnot(!is.na(uncited))
custom[uncited,1] <- -0.25
SeuratObject::LayerData(fixture,assay='RNA',layer='custom_abundance') <- custom
write.csv(markers[markers$gene!=uncited,c('cluster','gene','p_val','p_val_adj','pct_expr','mean_expr')],'markers.csv',row.names=FALSE)
saveRDS(fixture,'result.rds')
"#;
    let (out, status, _) = run(&mut client, &session, negative_layer).await;
    assert_eq!(status, "ok", "{out}");
    let mut negative_details = details.clone();
    negative_details["expression_layer"] = json!("custom_abundance");
    let negative_result = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        negative_details,
    )
    .await;
    assert_eq!(
        item(&negative_result, "markers")["status"],
        "fail",
        "{negative_result}"
    );
    assert_eq!(
        item(&negative_result, "annotation_evidence")["status"],
        "fail",
        "{negative_result}"
    );
    let (out,status,_)=run(&mut client,&session,"fixture <- original_result\nsaveRDS(fixture,'result.rds')\nwrite.csv(markers[,c('cluster','gene','p_val','p_val_adj','pct_expr','mean_expr')],'markers.csv',row.names=FALSE)").await;
    assert_eq!(status, "ok", "{out}");
    let historical = harness
        .daemon
        .get_analysis(root.path().to_str().unwrap(), id(&record))
        .await
        .unwrap();
    assert_eq!(historical["validity"]["current"], false);
    let (_, status, _) = run(
        &mut client,
        &session,
        "bad <- readRDS('result.rds'); bad@meta.data[1,1] <- 'damaged'; saveRDS(bad,'result.rds')",
    )
    .await;
    assert_eq!(status, "ok");
    let damaged = check(&harness.daemon, root.path(), id(&record), "result", details).await;
    assert_eq!(
        item(&damaged, "result_metadata")["status"],
        "fail",
        "{damaged}"
    );
    assert_eq!(damaged["latest_check"]["technical_pass"], false);
    let _ = client
        .execute(&session, "Sys.sleep(2)", None)
        .await
        .unwrap();
    let busy = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "input",
        json!({}),
    )
    .await;
    assert!(busy["latest_check"]["run_id"].is_null());
    assert_eq!(busy["latest_check"]["technical_pass"], false);
    assert!(
        busy["latest_check"]["facts"]["execution_error"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    let other = common::tempdir().unwrap();
    assert!(
        harness
            .daemon
            .begin_analysis(other.path().to_str().unwrap(), spec)
            .await
            .is_err()
    );
    let (_, status, _) = run(
        &mut client,
        &session,
        "fixture@assays$RNA@layers$counts@x[1] <- -1",
    )
    .await;
    assert_eq!(status, "ok");
    let negative = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "input",
        json!({}),
    )
    .await;
    assert_eq!(
        item(&negative, "counts_values")["status"],
        "fail",
        "{negative}"
    );
    let (_, status, _) = run(&mut client, &session, "fixture <- readRDS('raw.rds')").await;
    assert_eq!(status, "ok");
    let mut no_source = scrna_spec("r", &session, &raw);
    no_source["source_files"] = json!([]);
    let no_source = harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), no_source)
        .await
        .unwrap();
    let missing = check(
        &harness.daemon,
        root.path(),
        id(&no_source),
        "input",
        json!({}),
    )
    .await;
    assert_eq!(
        item(&missing, "source_correspondence")["status"],
        "not_checked",
        "{missing}"
    );
    assert_eq!(missing["latest_check"]["technical_pass"], false);
    let mapping_setup = r#"
raw_fixture <- readRDS('raw.rds')
mapping_counts <- SeuratObject::LayerData(raw_fixture,assay='RNA',layer='counts')
rownames(mapping_counts) <- sprintf('ENSG%011d',seq_len(nrow(mapping_counts)))
fixture <- SeuratObject::CreateSeuratObject(mapping_counts,assay='RNA',min.cells=0,min.features=0)
fixture$percent.mt <- raw_fixture$percent.mt
saveRDS(fixture,'ensembl-raw.rds')
"#;
    let (out, status, _) = run(&mut client, &session, mapping_setup).await;
    assert_eq!(status, "ok", "{out}");
    let mut mapped_spec = scrna_spec("r", &session, &root.path().join("ensembl-raw.rds"));
    mapped_spec["gene_id_type"] = json!("ensembl");
    let mapped = harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), mapped_spec)
        .await
        .unwrap();
    let mapping_details = json!({"mito_gene_ids":["ENSG00000000013","ENSG00000000014"],"mito_source":"Synthetic fixture generator: original MT-CO1 and MT-ND1 renamed as rows13 and14"});
    let mapped_input = check(
        &harness.daemon,
        root.path(),
        id(&mapped),
        "input",
        mapping_details,
    )
    .await;
    assert_eq!(
        item(&mapped_input, "mitochondrial_genes")["status"],
        "pass",
        "{mapped_input}"
    );
    assert_eq!(
        mapped_input["latest_check"]["technical_pass"], true,
        "{mapped_input}"
    );
    let mapped_qc = check(
        &harness.daemon,
        root.path(),
        id(&mapped),
        "qc",
        qc_details(root.path(), "r"),
    )
    .await;
    assert_eq!(
        item(&mapped_qc, "mitochondrial_genes")["status"],
        "pass",
        "{mapped_qc}"
    );
    assert_eq!(
        item(&mapped_qc, "qc_metrics")["status"],
        "pass",
        "{mapped_qc}"
    );
    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn design_record_survives_daemon_reopen_without_importing_exports() {
    let state = common::tempdir().unwrap();
    let root = common::tempdir().unwrap();
    let daemon = Daemon::open(StateDir::new(state.path())).await.unwrap();
    let record = daemon
        .begin_analysis(root.path().to_str().unwrap(), design_spec())
        .await
        .unwrap();
    let checked = check(&daemon, root.path(), id(&record), "design", json!({})).await;
    drop(daemon);
    let reopened = Daemon::open(StateDir::new(state.path())).await.unwrap();
    let historical = reopened
        .get_analysis(root.path().to_str().unwrap(), id(&record))
        .await
        .unwrap();
    assert_eq!(
        historical["latest_check"]["id"],
        checked["latest_check"]["id"]
    );
    assert_eq!(historical["validity"]["current"], false);
    assert!(historical["analysis"].get("baseline").is_none());
}

#[tokio::test]
async fn real_omicverse_all_stages_sparse_subset_same_path_damage_and_read_only_checks() {
    let state = common::tempdir().unwrap();
    let root = common::tempdir().unwrap();
    let harness = Harness::open(state.path()).await;
    let Some((mut client, session)) = session(&harness, root.path(), "python").await else {
        return;
    };
    let setup = r#"
from pathlib import Path
import numpy as np
import pandas as pd
from scipy import sparse
import anndata as ad
import omicverse as ov
import scanpy as sc
import hashlib, pickle, random, os, json
rows, cols, data = [], [], []
for i in range(24):
    values = {0: 1+i%2, 1: 2, 2: 2+i%3, 3 if i<12 else 5: 30+i%5, 4 if i<12 else 6: 20+i%7}
    for j,value in values.items():
        rows.append(i);cols.append(j);data.append(value)
counts = sparse.csr_matrix((data,(rows,cols)),shape=(24,8),dtype=np.int32)
fixture = ad.AnnData(X=counts,obs=pd.DataFrame({'donor':pd.Categorical(['single_tutorial_donor']*24)},index=['c%d'%i for i in range(24)]),var=pd.DataFrame(index=['MT-CO1','RPL1','ACTB','CD3D','CD3E','MS4A1','CD79A','ZERO']))
fixture.layers['counts'] = counts
fixture.var['mt'] = fixture.var_names.str.startswith('MT-')
sc.pp.calculate_qc_metrics(fixture,qc_vars=['mt'],layer='counts',percent_top=None,log1p=False,inplace=True)
fixture.write_h5ad('raw.h5ad')
def fixture_fingerprint():
    def frame_values(frame):
        return (frame.index.tolist(), frame.index.name, frame.columns.tolist(),
                frame.columns.name, frame.to_dict(orient='split'),
                [(str(column.dtype), column.cat.categories.tolist() if isinstance(column.dtype, pd.CategoricalDtype) else None,
                  column.cat.ordered if isinstance(column.dtype, pd.CategoricalDtype) else None)
                 for _, column in frame.items()])
    values = (id(fixture), fixture.X, frame_values(fixture.obs), frame_values(fixture.var),
              dict(fixture.uns), dict(fixture.layers), dict(fixture.obsm), dict(fixture.varm),
              dict(fixture.obsp), dict(fixture.varp), np.random.get_state(),
              random.getstate(), os.getcwd())
    return hashlib.sha256(pickle.dumps(values, protocol=4)).hexdigest()
fingerprint_before = fixture_fingerprint()
"#;
    let (out, status, _) = run(&mut client, &session, setup).await;
    assert_eq!(status, "ok", "{out}");
    let mut spec = scrna_spec("python", &session, &root.path().join("raw.h5ad"));
    spec["parameters"]["seed"] = json!(0);
    spec["design"]["tissue"] = json!("synthetic PBMC-like tutorial");
    let record = harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), spec)
        .await
        .unwrap();
    let input = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "input",
        json!({}),
    )
    .await;
    assert_eq!(input["latest_check"]["technical_pass"], true, "{input}");
    assert_eq!(item(&input, "input_sources")["status"], "pass");
    assert!(input["analysis"].get("baseline").is_none());
    let (out,status,_)=run(&mut client,&session,"assert fixture_fingerprint() == fingerprint_before\nassert sparse.issparse(fixture.layers['counts'])").await;
    assert_eq!(status, "ok", "{out}");
    let qc_setup = r#"
fixture = fixture[:,fixture.var_names!='ZERO'].copy()
pd.DataFrame([{'axis':'gene','id':'ZERO','reason':'Zero counts across every input cell'}]).to_csv('exclusions.csv',index=False,mode='x')
fixture.var['mt']=fixture.var_names.str.startswith('MT-')
sc.pp.calculate_qc_metrics(fixture,qc_vars=['mt'],layer='counts',percent_top=None,log1p=False,inplace=True)
fingerprint_before = fixture_fingerprint()
"#;
    let (out, status, _) = run(&mut client, &session, qc_setup).await;
    assert_eq!(status, "ok", "{out}");
    let qc = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "qc",
        qc_details(root.path(), "python"),
    )
    .await;
    assert_eq!(qc["latest_check"]["technical_pass"], true, "{qc}");
    assert_eq!(item(&qc, "qc_counts_unchanged")["status"], "pass");
    let (out, status, _) = run(
        &mut client,
        &session,
        "assert fixture_fingerprint() == fingerprint_before",
    )
    .await;
    assert_eq!(status, "ok", "{out}");
    let processing = r#"
ov.settings.mode='cpu'
fixture.X=fixture.layers['counts'].astype(np.float32,copy=True)
ov.pp.normalize_total(fixture,target_sum=10000,exclude_highly_expressed=False)
ov.pp.log1p(fixture)
fixture.layers['lognorm']=fixture.X.copy()
ov.pp.pca(fixture,layer='lognorm',n_pcs=3,zero_center=True,svd_solver='arpack',random_state=0)
ov.pp.neighbors(fixture,use_rep='X_pca',n_pcs=3,n_neighbors=5,random_state=0)
ov.pp.leiden(fixture,resolution=0.5,key_added='leiden',random_state=0,flavor='igraph',n_iterations=2)
ov.pp.umap(fixture,min_dist=0.5,spread=1.0,random_state=0)
sc.tl.rank_genes_groups(fixture,groupby='leiden',layer='lognorm',use_raw=False,method='wilcoxon',corr_method='benjamini-hochberg',n_genes=fixture.n_vars,key_added='markers')
markers=sc.get.rank_genes_groups_df(fixture,group=None,key='markers').rename(columns={'group':'cluster','names':'gene','pvals':'p_val','pvals_adj':'p_val_adj'})
def fixture_summary(cluster,gene):
    mask=fixture.obs['leiden'].astype(str).to_numpy()==str(cluster)
    column=fixture.layers['lognorm'][mask,fixture.var_names.get_loc(gene)]
    return float((column>0).mean()),float(column.mean())
summary=[fixture_summary(row.cluster,row.gene) for row in markers.itertuples()]
markers['pct_expr']=[x[0] for x in summary];markers['mean_expr']=[x[1] for x in summary]
markers[['cluster','gene','p_val','p_val_adj','pct_expr','mean_expr']].to_csv('markers.csv',index=False,mode='x')
annotations=[]
for cluster in fixture.obs['leiden'].astype(str).unique():
    for gene in ['CD3D','MS4A1']:
        pct,mean=fixture_summary(cluster,gene)
        annotations.append({'cluster':str(cluster),'label':'unknown','gene':gene,'pct_expr':pct,'mean_expr':mean,'evidence_type':'support','source':'Synthetic fixture, not independent biological reference','status':'unknown','reason':'Assessed no-conflict in fixture-only candidates; independent human review required'})
pd.DataFrame(annotations).to_csv('annotations.csv',index=False,mode='x')
fixture.write_h5ad('result.h5ad')
fingerprint_before=fixture_fingerprint()
"#;
    std::fs::write(root.path().join("analysis.py"), processing).unwrap();
    let (out, status, execution) = run(&mut client, &session, processing).await;
    assert_eq!(status, "ok", "{out}");
    let details = json!({"result_file":root.path().join("result.h5ad"),"cluster_key":"leiden","embedding_key":"X_umap","expression_layer":"lognorm","marker_file":root.path().join("markers.csv"),"annotation_file":root.path().join("annotations.csv"),"marker_method":"wilcoxon","p_adjust_method":"benjamini-hochberg","marker_uns_key":"markers","code_files":[root.path().join("analysis.py")],"execution_run_ids":[execution],"parameter_evidence":{"pca":{"uns_key":"pca","parameters":{"n_pcs":3,"zero_center":true,"svd_solver":"arpack"},"seed":0},"neighbors":{"uns_key":"neighbors","parameters":{"n_neighbors":5,"n_pcs":3,"use_rep":"X_pca"},"seed":0},"clustering":{"uns_key":"leiden","parameters":{"resolution":0.5},"seed":0},"umap":{"uns_key":"umap","parameters":{"min_dist":0.5,"spread":1.0},"seed":0}}});
    let result = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        details.clone(),
    )
    .await;
    assert_eq!(result["latest_check"]["technical_pass"], true, "{result}");
    assert_eq!(item(&result, "result_file")["status"], "pass");
    let (out,status,_)=run(&mut client,&session,"assert fixture_fingerprint()==fingerprint_before\nbad=ad.read_h5ad('result.h5ad')\ndel bad.uns['markers']\nbad.write_h5ad('result.h5ad')").await;
    assert_eq!(status, "ok", "{out}");
    let missing_rank = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        details.clone(),
    )
    .await;
    assert_eq!(
        item(&missing_rank, "result_markers")["status"],
        "fail",
        "{missing_rank}"
    );
    let rank_damage = r#"
fixture.write_h5ad('result.h5ad')
original_names=fixture.uns['markers']['names'].copy()
group=fixture.uns['markers']['names'].dtype.names[0]
fixture.uns['markers']['names'][group][-1]='ABSENT_FEATURE'
fixture.write_h5ad('result.h5ad')
"#;
    let (out, status, _) = run(&mut client, &session, rank_damage).await;
    assert_eq!(status, "ok", "{out}");
    let unknown_rank = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        details.clone(),
    )
    .await;
    assert_eq!(
        item(&unknown_rank, "result_markers")["status"],
        "fail",
        "{unknown_rank}"
    );
    let (out,status,_)=run(&mut client,&session,"fixture.uns['markers']['names']=original_names\nfixture.write_h5ad('result.h5ad')\nfingerprint_before=fixture_fingerprint()").await;
    assert_eq!(status, "ok", "{out}");
    let (out,status,_)=run(&mut client,&session,"assert fixture_fingerprint()==fingerprint_before\nbad=ad.read_h5ad('result.h5ad')\nbad.obs['donor']=bad.obs['donor'].astype(str)\nbad.obs.iloc[0,bad.obs.columns.get_loc('donor')]='damaged'\nbad.write_h5ad('result.h5ad')").await;
    assert_eq!(status, "ok", "{out}");
    let history = harness
        .daemon
        .get_analysis(root.path().to_str().unwrap(), id(&record))
        .await
        .unwrap();
    assert_eq!(history["validity"]["current"], false);
    assert!(
        history["validity"]["known_changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("additional code"))
    );
    let damaged = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        details.clone(),
    )
    .await;
    assert_eq!(
        item(&damaged, "result_metadata")["status"],
        "fail",
        "{damaged}"
    );
    let (out, status, _) = run(
        &mut client,
        &session,
        "fixture.layers['counts'][0,0]+=1\nfixture.write_h5ad('result.h5ad')",
    )
    .await;
    assert_eq!(status, "ok", "{out}");
    let counts_changed = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "result",
        details.clone(),
    )
    .await;
    assert_eq!(
        item(&counts_changed, "final_qc_qc_counts_unchanged")["status"],
        "fail",
        "{counts_changed}"
    );
    assert_eq!(counts_changed["latest_check"]["technical_pass"], false);
    let (out, status, _) = run(
        &mut client,
        &session,
        "Path('result.h5ad').write_bytes(b'not hdf5')",
    )
    .await;
    assert_eq!(status, "ok", "{out}");
    let unreadable = check(&harness.daemon, root.path(), id(&record), "result", details).await;
    assert_eq!(
        item(&unreadable, "result_file")["status"],
        "fail",
        "{unreadable}"
    );
    let (out, status, _) = run(&mut client, &session, "fixture.layers['counts'][0,0]=-1").await;
    assert_eq!(status, "ok", "{out}");
    let negative = check(
        &harness.daemon,
        root.path(),
        id(&record),
        "input",
        json!({}),
    )
    .await;
    assert_eq!(
        item(&negative, "counts_values")["status"],
        "fail",
        "{negative}"
    );
    if let Err(error) = client.shutdown(&session).await {
        eprintln!(
            "graceful teardown of owned test kernel failed: {error}; stopping only this isolated fixture's resources"
        );
        let stopped: Value = client
            .call(Call::StopResources { force: true })
            .await
            .unwrap();
        assert_eq!(stopped["stopped"], true, "{stopped}");
    }
}

#[tokio::test]
async fn fifo_source_is_rejected_without_holding_analysis_requests_indefinitely() {
    let state = common::tempdir().unwrap();
    let root = common::tempdir().unwrap();
    let harness = Harness::open(state.path()).await;
    let fifo = root.path().join("unwritten.fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut spec = design_spec();
    spec["source_files"] = json!([fifo]);
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        harness
            .daemon
            .begin_analysis(root.path().to_str().unwrap(), spec),
    )
    .await
    .expect("FIFO must not block the global analysis gate");
    assert!(outcome.unwrap_err().to_string().contains("regular input"));
    harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), design_spec())
        .await
        .unwrap();
}

#[tokio::test]
async fn documents_are_reowned_each_check_and_cannot_alias_managed_exports() {
    let state = common::tempdir().unwrap();
    let root = common::tempdir().unwrap();
    let outside = common::tempdir().unwrap();
    let harness = Harness::open(state.path()).await;
    let document = root.path().join("analysis.py");
    std::fs::write(&document, b"original").unwrap();
    let mut spec = design_spec();
    spec["documents"] = json!([document]);
    let record = harness
        .daemon
        .begin_analysis(root.path().to_str().unwrap(), spec)
        .await
        .unwrap();
    let report = root
        .path()
        .join(".sandhill/analysis")
        .join(id(&record))
        .join("report.md");
    let report_before = std::fs::read(&report).unwrap();
    let foreign = outside.path().join("foreign.py");
    std::fs::write(&foreign, b"foreign").unwrap();
    std::fs::remove_file(&document).unwrap();
    std::os::unix::fs::symlink(&foreign, &document).unwrap();
    assert!(
        harness
            .daemon
            .check_analysis(
                root.path().to_str().unwrap(),
                id(&record),
                "design",
                json!({})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("owned")
    );
    std::fs::remove_file(&document).unwrap();
    std::fs::write(&document, b"original").unwrap();
    assert!(
        harness
            .daemon
            .check_analysis(
                root.path().to_str().unwrap(),
                id(&record),
                "design",
                json!({"documents":[report]})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("managed")
    );
    let alias = root.path().join("report-alias.md");
    std::fs::hard_link(&report, &alias).unwrap();
    assert!(
        harness
            .daemon
            .check_analysis(
                root.path().to_str().unwrap(),
                id(&record),
                "design",
                json!({"documents":[alias]})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("aliases")
    );
    assert_eq!(std::fs::read(report).unwrap(), report_before);
    let history = harness
        .daemon
        .get_analysis(root.path().to_str().unwrap(), id(&record))
        .await
        .unwrap();
    assert_eq!(history["history_count"], 0);
    assert_eq!(std::fs::read(foreign).unwrap(), b"foreign");
}
