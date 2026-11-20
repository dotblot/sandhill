//! Tools for the open Explore a Table window, scoped to the agent's session.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::{Call, QuickAgentTool};
use serde_json::{Value, json};

use super::{Tools, text_item};

/// Room for both the quiet R setup and the visible template run.
pub(crate) const RUN_WAIT: Duration = Duration::from_secs(12 * 60);

pub(crate) const GUIDANCE: &str = "You are helping in Sandhill's Explore a Table window. \
Call quick_state first to see the tables, what Sandhill recognized, the facts the user has \
accepted and what each plot needs. Use plain words and say what the data cannot show. \
Never name which group is higher unless the card has a Found or Told comparison. \
Use quick_suggest to fill unknown facts when you have evidence; explain why. \
Suggestions do not count until the user clicks Accept. Use quick_make for plots and tables \
that have a Sandhill template, and explain the result from the numbers it returns. \
For a plot without a template, only write your own R code if the user asks; use the \
Explore session and say that it was made by AI code. There is no editor in this window. \
After a finished reply, offer one or two short next steps as `<!-- next: First step | Second step -->`.";

pub(crate) fn is_explore(project: Option<&str>) -> bool {
    project.is_some_and(|project| Path::new(project).join(".sandhill/quick.json").is_file())
}

impl Tools {
    pub(super) async fn quick_tool(
        &self,
        tool: QuickAgentTool,
        args: &Value,
    ) -> Result<Vec<Value>> {
        let project = self
            .project
            .as_ref()
            .filter(|_| self.explore)
            .context("This tool is only available in an Explore a Table session.")?;
        if tool != QuickAgentTool::State
            && let Some(dir) = self.terminal_dir.as_deref()
        {
            let report = crate::terminal::choices::report(dir);
            if report["mode"] == "plan" || report["permission"] == "plan" {
                bail!("Plan mode can read the Explore card but can't change it or make plots.");
            }
        }
        let reply: Value = self
            .client()
            .await?
            .call(Call::QuickAgent {
                project: project.clone(),
                tool,
                params: args.clone(),
            })
            .await?;
        Ok(vec![text_item(&reply.to_string())])
    }
}

pub(super) fn tool_list() -> [Value; 3] {
    [
        json!({
            "name": QuickAgentTool::State.method(),
            "title": "Read the Explore session",
            "description": "Read the What we know card with the source of each fact, its sample factors (first level is the reference), the recognized tables and their available plots. Suggested facts are not accepted yet.",
            "inputSchema": {"type": "object", "additionalProperties": false},
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": QuickAgentTool::Suggest.method(),
            "title": "Suggest Explore card facts",
            "description": "Suggest missing facts without replacing anything Found or Told. Only send fields you can explain. The user must accept a suggestion before a template can use it. Use a table's R name from quick_state, or omit table for the one shown.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "table": {"type": "string"},
                "organism": {"type": "string"},
                "comparison": {"type": "object", "required": ["higher", "lower"], "additionalProperties": false, "properties": {
                    "higher": {"type": "string"}, "lower": {"type": "string"}
                }},
                "factors": {"type": "array", "description": "Sample factors for the session. The first level is the reference.", "items": {
                    "type": "object", "required": ["name", "levels", "assignments"], "additionalProperties": false, "properties": {
                        "name": {"type": "string"}, "levels": {"type": "array", "items": {"type": "string"}},
                        "assignments": {"type": "array", "items": {
                            "type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "string"}
                        }}
                    }
                }},
                "kind": {"enum": ["results", "counts", "normalized", "sample_sheet", "enrichment"]},
                "gene": {"type": "string", "description": "Column holding gene identifiers."},
                "gene_ids": {"enum": ["ensembl", "symbol", "entrez"]},
                "fc": {"type": "string", "description": "Log2 fold change column."},
                "padj": {"type": "string", "description": "Adjusted p-value column."},
                "mean": {"type": "string", "description": "Average expression column."},
                "mean_log": {"type": "boolean", "description": "Whether the suggested average expression column is already logged. Only with mean."},
                "samples": {"type": "array", "items": {"type": "string"}, "description": "Sample columns, in order."},
                "values": {"enum": ["counts", "normalized", "logged"]},
                "term": {"type": "string", "description": "Term name column."},
                "count": {"type": "string", "description": "Gene count column."},
                "ratio": {"type": "string", "description": "Gene ratio column."}
            }},
        }),
        json!({
            "name": QuickAgentTool::Make.method(),
            "title": "Make an Explore plot or table",
            "description": "Run a Sandhill template from quick_state using accepted facts only. Returns the template's numbers and summary after R finishes, or what the table still needs.",
            "inputSchema": {"type": "object", "required": ["plot"], "additionalProperties": false, "properties": {
                "plot": {"enum": ["volcano", "ma", "up_down", "top_genes", "results_heatmap", "library_sizes", "pca", "correlation", "variable_heatmap", "gene_by_group", "enrich_dot", "enrich_bar"]},
                "table": {"type": "string", "description": "The table's R name from quick_state. Omit it to use the table shown."},
                "gene": {"type": "string", "description": "For gene_by_group, the gene to show, as written in the table."}
            }},
        }),
    ]
}
