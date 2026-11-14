//! Durable bioinformatics declarations and program-measured check evidence.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Workflow {
    StudyDesign,
    Scrna,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisSpec {
    pub workflow: Workflow,
    pub guide_id: String,
    pub guide_digest: String,
    pub checker_digest: String,
    pub goal: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub counts_source: Value,
    #[serde(default)]
    pub species: Option<String>,
    #[serde(default)]
    pub gene_id_type: Option<String>,
    #[serde(default)]
    pub source_files: Vec<String>,
    #[serde(default)]
    pub documents: Vec<String>,
    #[serde(default)]
    pub design: Value,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub output_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEvidence {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisRecord {
    pub format_version: u32,
    pub id: String,
    pub project: String,
    pub created_ms: i64,
    pub language: String,
    pub spec: AnalysisSpec,
    pub input_files: Vec<FileEvidence>,
    pub baseline: Value,
    /// Digests of managed exports last successfully published by this daemon.
    pub exports: BTreeMap<String, String>,
    /// Journaled before publication; permits recovery after an atomic rename.
    #[serde(default)]
    pub pending_exports: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
    NotChecked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckItem {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(default)]
    pub evidence: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisCheck {
    pub id: String,
    pub analysis: String,
    pub checkpoint: String,
    /// Caller declarations used for this checkpoint, never measured evidence.
    pub details: Value,
    pub created_ms: i64,
    pub checker_digest: String,
    pub guide_digest: String,
    pub run_id: Option<String>,
    pub code_digest: Option<String>,
    pub result_digest: String,
    pub items: Vec<CheckItem>,
    pub facts: Value,
    pub files: Vec<FileEvidence>,
    pub session_evidence: Value,
    /// Technical checks only; scientific validity always requires human review.
    pub technical_pass: bool,
}
