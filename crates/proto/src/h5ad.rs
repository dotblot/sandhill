//! Read-only AnnData previews.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H5adSummary {
    pub n_obs: u64,
    pub n_vars: u64,
    pub x: Option<H5adMatrix>,
    pub layers: Vec<(String, H5adMatrix)>,
    pub obs: H5adTable,
    pub var: H5adTable,
    pub obsm: Vec<(String, Vec<u64>)>,
    pub varm: Vec<(String, Vec<u64>)>,
    pub obsp: Vec<(String, Vec<u64>)>,
    pub varp: Vec<(String, Vec<u64>)>,
    pub uns: Vec<String>,
    pub raw_n_vars: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H5adMatrix {
    pub dtype: String,
    pub storage: H5adStorage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum H5adStorage {
    Dense,
    Csr { nnz: u64, share: f64 },
    Csc { nnz: u64, share: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H5adTable {
    pub index_name: String,
    pub index: Vec<String>,
    pub columns: Vec<H5adColumn>,
    /// Total number of columns, including any beyond the preview limit.
    pub column_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H5adColumn {
    pub name: String,
    pub kind: H5adColumnKind,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum H5adColumnKind {
    Number { dtype: String },
    Text,
    Category { count: u64, examples: Vec<String> },
    Bool,
}
