//! Shared bioinformatics discovery and project-bound analysis calls.

use anyhow::{Context, Result, bail};
use sandhill_proto::Call;
use serde_json::{Value, json};

use super::{Tools, text_item};

pub(crate) const GUIDANCE: &str = "For bioinformatics design or analysis, inspect get_bio_guide first. \
Guides awaiting human domain review are not reviewed recommendations. Python analysis and plotting \
prefer omicverse when its installed interfaces fit; use other tools when needed and explain why. \
Keep analysis code in the user's document. For an analysis the user requests, use begin_analysis \
and check_analysis and inspect the returned evidence before reporting completion. Checks establish \
technical facts, not biological truth; human review remains required. Ordinary questions do not \
authorize starting an analysis, downloading data or installing packages. These tools do not sandbox \
the CLI's own shell or file access.";

impl Tools {
    pub(super) async fn bio_tool(&self, name: &str, args: &Value) -> Result<Vec<Value>> {
        if name == "get_bio_guide" {
            let optional = |key: &str| -> Result<Option<&str>> {
                match args.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value)),
                    _ => bail!("`{key}` must be a nonempty string"),
                }
            };
            let result = crate::bio::guides::get(optional("id")?, optional("resource")?)?;
            return Ok(vec![text_item(&serde_json::to_string(&result)?)]);
        }
        if matches!(name, "begin_analysis" | "check_analysis")
            && let Some(dir) = self.terminal_dir.as_deref()
        {
            let report = crate::terminal::choices::report(dir);
            if report["mode"] == "plan" || report["permission"] == "plan" {
                bail!(
                    "Plan mode only allows reading bioinformatics guides and recorded analysis evidence; switch modes to create records or run checks"
                );
            }
        }
        let project = match args.get("project") {
            Some(Value::String(value)) if !value.trim().is_empty() => value.as_str(),
            None | Some(Value::Null) => self.project.as_deref().context("`project` is required")?,
            _ => bail!("`project` must be a nonempty project path"),
        };
        let project = crate::projects::normalize(project)?;
        if let Some(scope) = self.project.as_deref()
            && crate::projects::normalize(scope)? != project
        {
            bail!("analysis tools cannot access another project from this MCP connection");
        }
        let required = |key: &str| -> Result<String> {
            args[key]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(String::from)
                .with_context(|| format!("`{key}` is required"))
        };
        let call = match name {
            "begin_analysis" => Call::BeginAnalysis {
                project,
                spec: args
                    .get("spec")
                    .filter(|value| value.is_object())
                    .cloned()
                    .context("`spec` must be an analysis specification from get_bio_guide")?,
            },
            "check_analysis" => Call::CheckAnalysis {
                project,
                analysis: required("analysis")?,
                checkpoint: required("checkpoint")?,
                details: args.get("details").cloned().unwrap_or_else(|| json!({})),
            },
            "get_analysis" => Call::GetAnalysis {
                project,
                analysis: required("analysis")?,
            },
            _ => bail!("no bioinformatics tool named {name}"),
        };
        let result = self.client().await?.call::<Value>(call).await.map_err(|error| {
            let error = anyhow::Error::from(error);
            let message = error.to_string();
            if message.contains("unknown variant") || message.contains("method not found") {
                error.context("this daemon does not support analysis tools; update Sandhill on this device and restart this Agent conversation")
            } else {
                error
            }
        })?;
        Ok(vec![text_item(&serde_json::to_string(&result)?)])
    }
}

pub(super) fn tool_list() -> Vec<Value> {
    let Value::Array(tools) = json!([
        {
            "name": "get_bio_guide",
            "title": "Read Sandhill's bioinformatics guides",
            "description": "List four built-in guides with versions, digests and human-review status, or read one and its allowlisted reference resources. Python analysis prioritizes omicverse, using other tools where needed. Does not start a kernel, install packages or download data. Unreviewed guides are not reviewed recommendations.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "id": {"type": "string", "enum": ["bio-study-design", "scrna-scanpy", "scrna-seurat", "bio-results-review"]},
                "resource": {"type": "string", "description": "An exact relative resource name listed by that guide; requires id."},
            }},
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "begin_analysis",
            "title": "Record an explicitly requested bioinformatics analysis",
            "description": "Create a journal-backed study_design or scrna analysis and readable project exports. Read get_bio_guide first and use its exact current guide/checker digests and specification. Study design needs no kernel; scrna binds an explicitly selected existing session and plain object variable name, never starts another environment. Declarations are not measured facts. Does not run the analysis, install software or download data.",
            "inputSchema": {"type": "object", "required": ["spec"], "additionalProperties": false, "properties": {
                "project": {"type": "string", "description": "Absolute project folder on the analysis device; defaults to this MCP connection's project, cannot change its scope."},
                "spec": {"type": "object", "required": ["workflow", "guide_id", "guide_digest", "checker_digest", "goal"], "properties": {
                    "workflow": {"type": "string", "enum": ["study_design", "scrna"]},
                    "guide_id": {"type": "string"},
                    "guide_digest": {"type": "string", "description": "Exact digest returned by get_bio_guide."},
                    "checker_digest": {"type": "string", "description": "Current design, Python or R checker digest from the guide."},
                    "goal": {"type": "string"},
                    "source_files": {"type": "array", "items": {"type": "string"}, "description": "Absolute read-only input file paths; originals are fingerprinted, never overwritten by managed exports."},
                    "documents": {"type": "array", "items": {"type": "string"}, "description": "Absolute paths of the visible analysis code documents."},
                    "design": {"type": "object", "description": "Declared experimental design; get_bio_guide specifies required fields."},
                    "parameters": {"type": "object", "description": "Recorded QC decisions, seed and method parameters; see the chosen guide."},
                    "output_dir": {"type": "string", "description": "Explicit result directory; otherwise results/<analysis-id> in this project."},
                    "session": {"type": "string", "description": "scrna: exact existing session ID in this project."},
                    "object": {"type": "string", "description": "scrna: plain live variable name, not an expression."},
                    "counts_source": {"type": "object", "description": "scrna: kind raw_counts, explicit provenance, processing_state raw, named AnnData layer or Seurat assay/layer."},
                    "species": {"type": "string"},
                    "gene_id_type": {"type": "string"},
                }},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": false},
        },
        {
            "name": "check_analysis",
            "title": "Run a published analysis checkpoint",
            "description": "Run fixed design structure checks or input/qc/result checks in the analysis's existing live kernel, journal actual execution evidence and export a report. A busy kernel is not interrupted or queued behind a long user analysis. Details follow the guide's evidence schema, not Agent-written pass results. Technical pass never means scientific correctness; required unchecked items cannot pass. Saved final .h5ad/.rds files must be independently reopened.",
            "inputSchema": {"type": "object", "required": ["analysis", "checkpoint"], "additionalProperties": false, "properties": {
                "project": {"type": "string"},
                "analysis": {"type": "string"},
                "checkpoint": {"type": "string", "enum": ["design", "input", "qc", "result"]},
                "details": {"type": "object", "description": "Checkpoint evidence locations and explicit decisions from the guide (for results, saved object, marker and annotation files). No arbitrary check code."},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": false},
        },
        {
            "name": "get_analysis",
            "title": "Read recorded analysis evidence",
            "description": "Read the daemon journal's authoritative analysis and checkpoint evidence, timestamps, known changes, export status and human-review limitations. Does not start a kernel or rerun a check. Handwritten or modified project reports never become verified evidence; old records do not assert current validity.",
            "inputSchema": {"type": "object", "required": ["analysis"], "additionalProperties": false, "properties": {
                "project": {"type": "string"},
                "analysis": {"type": "string"},
            }},
            "annotations": {"readOnlyHint": true},
        },
    ]) else {
        unreachable!()
    };
    tools
}
