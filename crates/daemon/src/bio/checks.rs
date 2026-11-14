//! Fixed scripts only: declarations are JSON, never executable expressions.
use crate::{journal::Journal, session::SessionHandle};
use anyhow::{Context, Result, bail};
use base64::Engine;
use sandhill_proto::analysis::{CheckItem, CheckStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const MAX_RESULT: usize = 1024 * 1024;
const PYTHON: &str = include_str!("../../assets/bio/check.py");
const R: &str = include_str!("../../assets/bio/check.R");
const DESIGN_VERSION: &str =
    "sandhill-design-check-v1:required-fields-sources-replicates-human-review";

pub fn hash(bytes: &[u8]) -> String {
    format!("sha256:{}", sandhill_proto::hex(&Sha256::digest(bytes)))
}

pub fn digest(language: &str) -> String {
    let script = match language {
        "python" => PYTHON,
        "r" => R,
        _ => DESIGN_VERSION,
    };
    // Include the invocation contract, not just the resource body.
    hash(
        format!(
            "sandhill-bio-check-v2\0{script}\0{}\0{}",
            include_str!("checks.rs"),
            include_str!("analysis.rs")
        )
        .as_bytes(),
    )
}

pub fn required(language: &str, checkpoint: &str) -> Vec<&'static str> {
    if language == "python" {
        return match checkpoint {
            "input" => vec![
                "object",
                "identifiers",
                "metadata",
                "counts_source",
                "counts_values",
                "processing_state",
                "environment",
                "seed",
                "input_sources",
                "gene_species",
                "study_design",
            ],
            "qc" => vec![
                "qc_metrics",
                "qc_thresholds",
                "qc_exclusions",
                "qc_subset",
                "qc_counts_unchanged",
                "qc_metadata",
                "qc_doublets",
            ],
            "result" => vec![
                "result_file",
                "result_metadata",
                "result_clusters",
                "result_embeddings",
                "result_parameters",
                "result_markers",
                "result_annotation",
                "result_design",
                "human_review",
            ],
            _ => vec![],
        };
    }
    let mut ids = vec![
        "object",
        "counts_source",
        "counts_values",
        "identifiers",
        "metadata",
        "environment",
        "seed",
        "processing_state",
        "mitochondrial_genes",
    ];
    ids.extend_from_slice(match checkpoint {
        "input" => &["source_correspondence", "study_design"][..],
        "qc" => &[
            "qc_metrics",
            "qc_thresholds",
            "qc_exclusions",
            "retained_counts",
            "doublets",
        ][..],
        "result" => &[
            "result_reload",
            "result_metadata",
            "result_embeddings",
            "execution_provenance",
            "parameter_evidence",
            "markers",
            "annotation_evidence",
            "human_review",
        ][..],
        _ => &[],
    });
    ids
}

pub fn incomplete(id: &str, message: impl Into<String>) -> CheckItem {
    CheckItem {
        id: id.into(),
        status: CheckStatus::NotChecked,
        message: message.into(),
        evidence: Value::Null,
    }
}

pub fn code(language: &str, request: &Value, token: &str) -> Result<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(request)?);
    let prefix = serde_json::to_string(&format!("SANDHILL_BIO_{token}:"))?;
    match language {
        "python" => {
            let dispatch = "\ndef sandhill_bio_dispatch(request, namespace):\n    result = sandhill_bio_check(request, namespace)\n    if request['checkpoint'] == 'result':\n        qc = dict(request)\n        qc['checkpoint'] = 'qc'\n        qc['details'] = request['qc_details']\n        checked = sandhill_bio_check(qc, namespace)\n        result['items'].extend(dict(item, id='final_qc_' + item['id']) for item in checked['items'])\n        result['facts']['final_qc'] = checked['facts']\n    return result\n";
            let script = serde_json::to_string(&format!("{PYTHON}{dispatch}"))?;
            Ok(format!(
                r#"(lambda ns: (exec({script}, ns), (lambda result: (lambda text: print({prefix} + text) if len(text.encode('utf-8')) <= {MAX_RESULT} else (_ for _ in ()).throw(ValueError('checker result exceeds 1MiB')))(__import__('json').dumps(result, allow_nan=False, separators=(',', ':'))))(ns['sandhill_bio_dispatch'](__import__('json').loads(__import__('base64').b64decode('{encoded}')), globals()))))({{}})"#
            ))
        }
        "r" => {
            let script = serde_json::to_string(R)?;
            // local() owns every temporary; the checker sees the user's namespace separately.
            let request_text = serde_json::to_string(&serde_json::to_string(request)?)?;
            Ok(format!(
                r#"local({{
  .scope <- new.env(parent=baseenv())
  eval(parse(text={script}), envir=.scope)
  .request <- jsonlite::fromJSON({request_text}, simplifyVector=FALSE)
  .result <- .scope$sandhill_bio_check(.request, namespace=.GlobalEnv)
  if (identical(.request$checkpoint, 'result')) {{
    .qc_request <- .request
    .qc_request$checkpoint <- 'qc'
    .qc_request$details <- .request$qc_details
    .qc_result <- .scope$sandhill_bio_check(.qc_request, namespace=.GlobalEnv)
    .qc_result$items <- lapply(.qc_result$items, function(item) {{ item$id <- paste0('final_qc_', item$id); item }})
    .result$items <- c(.result$items, .qc_result$items)
    .result$facts$final_qc <- .qc_result$facts
  }}
  .text <- as.character(jsonlite::toJSON(.result, auto_unbox=TRUE, null='null', na='null', digits=17))
  if (nchar(.text, type='bytes') > {MAX_RESULT}) stop('checker result exceeds 1MiB')
  cat({prefix}, .text, '\n', sep='')
}})"#
            ))
        }
        _ => bail!("unsupported checker language"),
    }
}

pub struct Measured {
    pub run_id: Option<String>,
    pub code_digest: Option<String>,
    pub result: Value,
}

pub async fn run(
    handle: &SessionHandle,
    journal: &Journal,
    language: &str,
    request: &Value,
) -> Measured {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut run_id = None;
    let mut code_digest = None;
    let measured = async {
        let script = code(language, request, &token)?;
        code_digest = Some(hash(script.as_bytes()));
        let mut events = handle.subscribe();
        let started = handle.bio_check(script).await?;
        run_id = Some(started.run.clone());
        let wait = async {
            loop {
                if let Some(status) = journal.bio_run_status(&started.run)? {
                    if status != "ok" {
                        bail!("checker run finished with status {status}");
                    }
                    return Ok(());
                }
                tokio::select! {
                    event = events.recv() => {
                        if matches!(event, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                            bail!("kernel disconnected during check");
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(120), wait)
            .await
            .context(
                "checkpoint timed out; run retained, no user analysis retried or interrupted",
            )??;
        let mut stdout = String::new();
        for (kind, content) in journal.bio_run_output(&started.run, MAX_RESULT * 2)? {
            if kind == "error" {
                bail!("checker error: {}", content["evalue"]);
            }
            if kind == "stream" && content["name"] == "stdout" {
                let text = content["text"].as_str().unwrap_or("");
                if stdout.len() + text.len() > MAX_RESULT + 256 {
                    bail!("checker output exceeds bound");
                }
                stdout.push_str(text);
            }
        }
        let prefix = format!("SANDHILL_BIO_{token}:");
        let rows: Vec<_> = stdout
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .collect();
        if rows.len() != 1 {
            bail!("checker did not return exactly one measured result");
        }
        let result: Value = serde_json::from_str(rows[0])?;
        if !result["facts"].is_object() || !result["items"].is_array() {
            bail!("invalid checker result schema");
        }
        Ok(result)
    }
    .await;
    let result = match measured {
        Ok(result) => result,
        Err(error) => json!({"items":[],"facts":{"execution_error":format!("{error:#}")}}),
    };
    Measured {
        run_id,
        code_digest,
        result,
    }
}

pub fn items(result: &Value, language: &str, checkpoint: &str) -> Vec<CheckItem> {
    let parsed = serde_json::from_value::<Vec<CheckItem>>(result["items"].clone());
    let mut items =
        parsed.unwrap_or_else(|err| vec![incomplete("checker_contract", err.to_string())]);
    let mut seen = std::collections::HashSet::new();
    if items.iter().any(|item| !seen.insert(item.id.clone())) {
        return vec![incomplete("checker_contract", "duplicate check item IDs")];
    }
    for id in required(language, checkpoint) {
        if !seen.contains(id) {
            items.push(incomplete(
                id,
                "required check did not complete; see execution evidence",
            ));
        }
    }
    if checkpoint == "result" {
        for id in required(language, "qc") {
            let id = format!("final_qc_{id}");
            if !seen.contains(&id) {
                items.push(incomplete(
                    &id,
                    "required final QC integrity check did not complete",
                ));
            }
        }
    }
    items
}

fn substantive(value: &Value) -> bool {
    match value {
        Value::String(s) => {
            let s = s.trim().to_lowercase();
            !s.is_empty()
                && ![
                    "unknown",
                    "tbd",
                    "not determined",
                    "未确定",
                    "尚未确定",
                    "未知",
                ]
                .iter()
                .any(|prefix| s.starts_with(prefix))
        }
        Value::Array(a) => !a.is_empty() && a.iter().all(substantive),
        Value::Object(o) => !o.is_empty() && o.values().all(substantive),
        Value::Number(_) | Value::Bool(_) => true,
        Value::Null => false,
    }
}

pub fn design(design: &Value) -> Value {
    let required = [
        "research_question",
        "evidence",
        "hypothesis",
        "experimental_unit",
        "groups",
        "pairing",
        "confounders",
        "primary_outcome",
        "analysis_method",
        "sample_size_rationale",
        "validation",
        "stopping_rules",
        "sources",
        "unresolved_questions",
    ];
    let mut items: Vec<CheckItem> = required
        .iter()
        .map(|key| CheckItem {
            id: format!("design_{key}"),
            status: if (*key == "unresolved_questions" && design[*key].as_array().is_some())
                || substantive(&design[*key])
            {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            message: format!("Declared {key}: structure only; not independently verified"),
            evidence: Value::Null,
        })
        .collect();
    let sources_ok = design["sources"].as_array().is_some_and(|sources| {
        !sources.is_empty()
            && sources.iter().all(|source| {
                source["url"].as_str().is_some_and(|s| {
                    (s.starts_with("https://") || s.starts_with("http://"))
                        && s.split("://")
                            .nth(1)
                            .is_some_and(|s| s.contains('.') && !s.contains(char::is_whitespace))
                }) || source["doi"].as_str().is_some_and(|s| {
                    s.starts_with("10.") && s.contains('/') && !s.contains(char::is_whitespace)
                }) || source["pmid"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            })
    });
    items.push(CheckItem {
        id: "source_identifiers".into(),
        status: if sources_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: "Identifiers recorded only; citation existence and support are NOT verified"
            .into(),
        evidence: Value::Null,
    });
    items.push(CheckItem { id:"biological_replicates".into(), status: if substantive(&design["biological_replicates"]) {CheckStatus::Warning} else {CheckStatus::NotChecked}, message:"Independent replication and inferential validity require human review; no disease/treatment contrast approved".into(), evidence:Value::Null });
    items.push(CheckItem { id:"human_review".into(), status:CheckStatus::Warning, message:"Scientific design, sources, sample-size justification and causal interpretation await independent human review".into(), evidence:Value::Null });
    json!({"items":items,"facts":{"kernel_executed":false,"declarations_only":true,"scientific_validity":"awaiting_human_review"}})
}
