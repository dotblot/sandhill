//! Tables and variables: a table view's state, rows, columns and summaries, and a session's variables.

use super::*;

/// A table view's state. Column positions refer to the original table.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TableFocus {
    pub session: String,
    pub path: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub sort: Vec<TableSort>,
    #[serde(default)]
    pub filters: Vec<TableFilter>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub search: String,
    #[serde(default)]
    pub hidden_columns: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_column: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_cells: Vec<TableCell>,
    pub shown_rows: u64,
    pub total_rows: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableCell {
    /// Zero-based row in the filtered/sorted view.
    pub row: u64,
    pub column: u32,
}

/// A session's variables, or the parts of one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Variables {
    pub session: String,
    pub path: Vec<String>,
    pub variables: Vec<Variable>,
    /// How many more parts there are than listed (a list shows its first
    /// 200).
    #[serde(default)]
    pub more: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariableMatches {
    pub session: String,
    pub matches: Vec<VariableMatch>,
    /// False when depth (6), node count (20,000), or time (2s) was hit.
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariableMatch {
    pub path: Vec<String>,
    pub name: String,
    pub class: String,
    pub summary: String,
    pub accessor: String,
    /// Whether this node has parts, as in Variable.
    #[serde(default)]
    pub children: bool,
    /// Whether this node opens as a table, as in Variable.
    #[serde(default)]
    pub table: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Variable {
    pub name: String,
    /// `data`, `value` (a single number, string or the like) or `function`.
    pub kind: String,
    /// The class or type, like `data.frame` or `AnnData`.
    pub class: String,
    /// In bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Whether it has parts to list (columns, items, slots).
    #[serde(default)]
    pub children: bool,
    /// Whether it opens in the Data Explorer (a data frame or a matrix).
    #[serde(default)]
    pub table: bool,
    /// Dimensions, the first values, or the like: "2,638 × 7; columns: …".
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessor: Option<String>,
}

/// A sort key for [`Call::TableRows`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableSort {
    /// The column's position, from 0.
    pub column: u32,
    #[serde(default)]
    pub descending: bool,
}

/// A filter for [`Call::TableRows`]: rows pass when `column` meets it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableFilter {
    /// The column's position, from 0.
    pub column: u32,
    /// `<`, `<=`, `>`, `>=`, `=` and `!=` (with one value), `between` (two),
    /// `in` and `not in` (any number), `contains` (text, ignoring case),
    /// `missing` and `present` (none).
    pub op: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// A table's column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableColumn {
    pub name: String,
    /// `number`, `text`, `category`, `logical`, `date` or `other`.
    pub kind: String,
    /// The language's own type: `dbl`, `int`, `chr`, `fct` in R; `float64`,
    /// `category`, `object` in Python.
    #[serde(rename = "type")]
    pub kind_detail: String,
}

/// Rows of a table, as [`Call::TableRows`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableRows {
    pub session: String,
    pub path: Vec<String>,
    /// The table as code in the session's language, like `seu@meta.data`
    /// or `adata.obs`.
    pub code: String,
    /// Whether it's a matrix (or, in Python, an array) rather than a data
    /// frame.
    #[serde(default)]
    pub matrix: bool,
    /// All its columns, unless only cells were asked for.
    #[serde(default)]
    pub columns: Vec<TableColumn>,
    /// Original positions for projected cells/headers, in requested order.
    /// Absent for contiguous pages and from old servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_indices: Option<Vec<u32>>,
    /// The column the cells in `values` start at: each row has the cells
    /// of the columns asked for, from this one on.
    #[serde(default)]
    pub column_offset: u32,
    /// How many columns it has.
    pub width: u64,
    /// How many rows it has.
    pub total: u64,
    /// How many rows pass the filters.
    pub rows: u64,
    pub offset: u64,
    /// Each returned row's label: its row name, or index label.
    pub labels: Vec<String>,
    /// Each returned row's cells: numbers in number columns, otherwise
    /// text, and null where missing.
    pub values: Vec<Vec<Value>>,
}

/// Each column's summary, as [`Call::TableSummary`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableSummary {
    pub session: String,
    pub path: Vec<String>,
    /// How many rows pass the filters.
    pub rows: u64,
    pub columns: Vec<ColumnSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ColumnSummary {
    /// The column's position, from 0.
    pub column: u32,
    pub missing: u64,
    /// How many different values there are, missing aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distinct: Option<u64>,
    /// For numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    /// Counts in equal bins from `min` to `max`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub histogram: Vec<u64>,
    /// The commonest values and how many rows have each, commonest first;
    /// left out when there are too many different values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top: Vec<ValueCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValueCount {
    pub value: String,
    pub count: u64,
}
