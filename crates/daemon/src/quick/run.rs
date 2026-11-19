//! Running a Quick Tools template in an R session: the code that defines
//! the templates once per session (quietly), the one visible line each run
//! shows, its result, the plain sentence about a plot, and `steps.R`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    QuickColumn, QuickPlot, QuickRequest, QuickResult, QuickSettings, QuickTable, hex,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::folder::inside;

/// The templates (`assets/quick/quick.R`).
pub const SCRIPT: &str = include_str!("../../assets/quick/quick.R");

fn digest() -> String {
    hex(&Sha256::digest(SCRIPT.as_bytes()))[..16].to_string()
}

/// A string as R writes one, in double quotes.
fn r_string(text: &str) -> String {
    serde_json::to_string(text).expect("a string always serializes")
}

/// A string in single quotes, as the visible line shows JSON.
fn r_single(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Quiet code that defines `.sandhill_quick(json, token)` in the session,
/// unless this version of the templates is already there.
pub fn define_code() -> String {
    let digest = digest();
    format!(
        r#"if (!identical(get0(".sandhill_quick_digest", envir = globalenv(), inherits = FALSE), "{digest}")) {{
  assign(".sandhill_quick", local({{
    eval(parse(text = {script}))
    function(json, token) {{
      request <- jsonlite::fromJSON(json, simplifyVector = FALSE)
      result <- sandhill_quick(request)
      text <- as.character(jsonlite::toJSON(result, auto_unbox = TRUE, null = "null", na = "null", digits = NA))
      cat("SANDHILL_QUICK_", token, ":", text, "\n", sep = "")
      invisible(NULL)
    }}
  }}), envir = globalenv())
  assign(".sandhill_quick_digest", "{digest}", envir = globalenv())
}}"#,
        script = r_string(SCRIPT),
    )
}

/// `request` with its file names made full paths inside `folder`.
pub fn resolve(folder: &Path, request: &QuickRequest) -> Result<QuickRequest> {
    let full =
        |name: &str| -> Result<String> { Ok(inside(folder, name)?.to_string_lossy().into_owned()) };
    Ok(match request {
        QuickRequest::Sheets { file } => QuickRequest::Sheets { file: full(file)? },
        QuickRequest::Pairs { table, key, value } => {
            if !r_name(table) {
                bail!("{table} can't be an R variable's name");
            }
            QuickRequest::Pairs {
                table: table.clone(),
                key: key.clone(),
                value: value.clone(),
            }
        }
        QuickRequest::Load { file, sheet, name } => {
            if !r_name(name) {
                bail!("{name} can't be an R variable's name");
            }
            QuickRequest::Load {
                file: full(file)?,
                sheet: sheet.clone(),
                name: name.clone(),
            }
        }
        QuickRequest::Plot {
            plot,
            table,
            settings,
            output,
        } => {
            if !r_name(table) || output.as_deref().is_some_and(|o| !r_name(o)) {
                bail!("not an R variable's name");
            }
            QuickRequest::Plot {
                plot: *plot,
                table: table.clone(),
                settings: settings.clone(),
                output: output.clone(),
            }
        }
        QuickRequest::Workbook {
            path,
            readme,
            sheets,
        } => {
            if sheets.iter().any(|(_, variable)| !r_name(variable)) {
                bail!("not an R variable's name");
            }
            let at = inside(folder, path)?;
            if let Some(dir) = at.parent() {
                std::fs::create_dir_all(dir)?;
            }
            QuickRequest::Workbook {
                path: at.to_string_lossy().into_owned(),
                readme: readme.clone(),
                sheets: sheets.clone(),
            }
        }
    })
}

/// A syntactic R name Sandhill makes: a letter, then letters, digits, `_`
/// or `.`.
fn r_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && name.len() <= 64
}

/// The line a run shows: `.sandhill_quick('{"action":…}', 'token')`.
pub fn visible_code(request: &QuickRequest, token: &str) -> Result<String> {
    let json = serde_json::to_string(request)?;
    Ok(format!(
        ".sandhill_quick({}, {})",
        r_single(&json),
        r_single(token)
    ))
}

/// What the template printed after its marker.
pub fn reply(stdout: &str, token: &str) -> Result<Value> {
    let marker = format!("SANDHILL_QUICK_{token}:");
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix(&marker))
        .context("the template didn't answer")?;
    Ok(serde_json::from_str(line)?)
}

#[derive(Deserialize)]
struct RawTable {
    name: String,
    rows: u64,
    columns: Vec<QuickColumn>,
}

/// The run's result, from the template's reply to `request` (as the app
/// sent it, with names relative to the session folder).
pub fn result(request: &QuickRequest, reply: Value) -> Result<QuickResult> {
    let mut result = QuickResult {
        facts: reply.get("facts").cloned().unwrap_or(Value::Null),
        output: reply
            .get("output")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..QuickResult::default()
    };
    match request {
        QuickRequest::Sheets { .. } => {
            result.sheets = serde_json::from_value(reply["sheets"].clone())
                .context("the template's sheets can't be read")?;
        }
        QuickRequest::Load { file, sheet, .. } => {
            let raw: RawTable = serde_json::from_value(reply["table"].clone())
                .context("the template's table can't be read")?;
            let recognition = super::recognise::recognise(&raw.columns);
            result.table = Some(QuickTable {
                name: raw.name,
                file: file.clone(),
                sheet: sheet.clone(),
                rows: raw.rows,
                columns: raw.columns,
                recognition,
            });
        }
        QuickRequest::Plot { plot, settings, .. } => {
            result.sentence = sentence(*plot, settings, &result.facts);
        }
        QuickRequest::Pairs { .. } => {
            result.pairs = serde_json::from_value(reply["pairs"].clone())
                .context("the template's pairs can't be read")?;
        }
        QuickRequest::Workbook { .. } => {}
    }
    Ok(result)
}

/// Adds a run to the session's `steps.R`, which with `.sandhill/quick.R`
/// repeats the session in plain R.
pub fn record_step(folder: &Path, request: &QuickRequest) -> Result<()> {
    if matches!(
        request,
        QuickRequest::Sheets { .. } | QuickRequest::Pairs { .. }
    ) {
        return Ok(());
    }
    let templates = folder.join(".sandhill/quick.R");
    if std::fs::read_to_string(&templates).ok().as_deref() != Some(SCRIPT) {
        std::fs::create_dir_all(folder.join(".sandhill"))?;
        std::fs::write(&templates, SCRIPT)?;
    }
    let steps = folder.join("steps.R");
    let new = !steps.exists();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&steps)
        .context("cannot write steps.R")?;
    if new {
        writeln!(
            file,
            "# What Sandhill ran in this folder, in order. Run it in R from this folder\n# to repeat the session; it needs the packages for exploring tables.\nsource(\".sandhill/quick.R\")\n"
        )?;
    }
    writeln!(
        file,
        "sandhill_quick(jsonlite::fromJSON({}, simplifyVector = FALSE))",
        r_single(&serde_json::to_string(request)?)
    )?;
    Ok(())
}

/// 12345 as "12,345".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "2-fold" for a log2 cutoff of 1, "1.5-fold" for 0.585.
fn fold(lfc: f64) -> String {
    let fold = 2f64.powf(lfc);
    let text = format!("{fold:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text);
    format!("{text}-fold")
}

fn number(facts: &Value, key: &str) -> Option<u64> {
    facts.get(key)?.as_f64().map(|n| n.round() as u64)
}

fn names(facts: &Value, key: &str, most: usize) -> Vec<String> {
    facts
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .take(most)
                .collect()
        })
        .unwrap_or_default()
}

fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a plot shows, in one or two plain sentences from its numbers.
pub fn sentence(plot: QuickPlot, settings: &QuickSettings, facts: &Value) -> Option<String> {
    let cut = settings.cutoffs;
    Some(match plot {
        QuickPlot::Volcano | QuickPlot::Ma | QuickPlot::UpDown => {
            let tested = number(facts, "tested")?;
            let up = number(facts, "up")?;
            let down = number(facts, "down")?;
            let mut text = format!(
                "{} of {} genes pass the cutoffs (adjusted p below {}, at least {}): ",
                thousands(up + down),
                thousands(tested),
                cut.padj,
                fold(cut.lfc)
            );
            match &settings.comparison {
                Some(c) => text.push_str(&format!(
                    "{} higher in {}, {} higher in {}.",
                    thousands(up),
                    c.higher,
                    thousands(down),
                    c.lower
                )),
                None => text.push_str(&format!(
                    "{} with a positive fold change, {} negative. The table doesn't say which group is higher.",
                    thousands(up),
                    thousands(down)
                )),
            }
            let top_up = names(facts, "top_up", 3);
            let top_down = names(facts, "top_down", 3);
            if !top_up.is_empty() || !top_down.is_empty() {
                let mut parts = Vec::new();
                if !top_up.is_empty() {
                    parts.push(format!("{} up", listed(&top_up)));
                }
                if !top_down.is_empty() {
                    parts.push(format!("{} down", listed(&top_down)));
                }
                text.push_str(&format!(" Most significant: {}.", parts.join("; ")));
            }
            if let Some(missing) = number(facts, "missing").filter(|&m| m > 0) {
                text.push_str(&format!(
                    " {} genes have no adjusted p-value and are left out.",
                    thousands(missing)
                ));
            }
            text
        }
        QuickPlot::TopGenes => format!(
            "{} genes pass the cutoffs; their table is open in the Data Explorer.",
            thousands(number(facts, "rows")?)
        ),
        QuickPlot::ResultsHeatmap => format!(
            "The {} most significant changed genes across {} samples. Each row is scaled to its own average: red above it, blue below.",
            number(facts, "genes")?,
            number(facts, "samples")?
        ),
        QuickPlot::VariableHeatmap => format!(
            "The {} genes that vary most across {} samples. Each row is scaled to its own average: red above it, blue below.",
            number(facts, "genes")?,
            number(facts, "samples")?
        ),
        QuickPlot::LibrarySizes => {
            let small = facts.get("smallest")?;
            let large = facts.get("largest")?;
            format!(
                "Samples have between {:.1} million reads ({}) and {:.1} million ({}).",
                small.get("millions")?.as_f64()?,
                small.get("sample")?.as_str()?,
                large.get("millions")?.as_f64()?,
                large.get("sample")?.as_str()?
            )
        }
        QuickPlot::Pca => format!(
            "The first two components explain {}% and {}% of the variation in the {} most variable genes. Samples close together have similar expression.",
            facts.get("pc1")?.as_f64()?,
            facts.get("pc2")?.as_f64()?,
            number(facts, "genes")?
        ),
        QuickPlot::Correlation => {
            let lowest = facts.get("lowest")?;
            format!(
                "The least similar samples are {} and {} (correlation {}).",
                lowest.get("a")?.as_str()?,
                lowest.get("b")?.as_str()?,
                lowest.get("r")?.as_f64()?
            )
        }
        QuickPlot::GeneByGroup => {
            let gene = facts.get("gene")?.as_str()?;
            let means: Vec<String> = facts
                .get("means")?
                .as_array()?
                .iter()
                .filter_map(|m| {
                    Some(format!(
                        "{} {:.2}",
                        m.get("group")?.as_str()?,
                        m.get("mean")?.as_f64()?
                    ))
                })
                .collect();
            format!("{gene}, on a log2 scale, averages {}.", listed(&means))
        }
        QuickPlot::EnrichDot | QuickPlot::EnrichBar => {
            let top = names(facts, "top", 3);
            let mut text = format!(
                "The table has {} terms; the plot shows the {} most significant",
                thousands(number(facts, "terms")?),
                number(facts, "shown")?
            );
            if top.is_empty() {
                text.push('.');
            } else {
                text.push_str(&format!(", led by {}.", listed(&top)));
            }
            text
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sandhill_proto::{QuickComparison, QuickCutoffs};
    use serde_json::json;

    #[test]
    fn the_visible_line_quotes_json_for_r() {
        let request = QuickRequest::Load {
            file: "/tmp/it's.csv".into(),
            sheet: None,
            name: "de_results".into(),
        };
        let line = visible_code(&request, "abc").unwrap();
        assert_eq!(
            line,
            r#".sandhill_quick('{"action":"load","file":"/tmp/it\'s.csv","name":"de_results"}', 'abc')"#
        );
    }

    #[test]
    fn requests_stay_in_the_session_folder() {
        let folder = Path::new("/tmp/session");
        let load = |file: &str, name: &str| QuickRequest::Load {
            file: file.into(),
            sheet: None,
            name: name.into(),
        };
        let resolved = resolve(folder, &load("de.csv", "de")).unwrap();
        assert_eq!(resolved, load("/tmp/session/de.csv", "de"));
        assert!(resolve(folder, &load("../x.csv", "de")).is_err());
        assert!(resolve(folder, &load("de.csv", "system('rm')")).is_err());
    }

    #[test]
    fn the_reply_is_the_marked_line() {
        let out = "note\nSANDHILL_QUICK_t1:{\"facts\":{\"rows\":3}}\n";
        assert_eq!(reply(out, "t1").unwrap(), json!({"facts": {"rows": 3}}));
        assert!(reply(out, "t2").is_err());
    }

    #[test]
    fn a_volcano_sentence_names_the_groups_only_when_known() {
        let facts = json!({"tested": 18234, "up": 610, "down": 624, "missing": 12,
            "top_up": ["IFIT1", "MX1", "ISG15", "OAS1"], "top_down": ["COL1A1"]});
        let mut settings = QuickSettings {
            cutoffs: QuickCutoffs::default(),
            ..QuickSettings::default()
        };
        let unknown = sentence(QuickPlot::Volcano, &settings, &facts).unwrap();
        assert_eq!(
            unknown,
            "1,234 of 18,234 genes pass the cutoffs (adjusted p below 0.05, at least 2-fold): 610 with a positive fold change, 624 negative. The table doesn't say which group is higher. Most significant: IFIT1, MX1 and ISG15 up; COL1A1 down. 12 genes have no adjusted p-value and are left out."
        );
        settings.comparison = Some(QuickComparison {
            higher: "treated".into(),
            lower: "control".into(),
        });
        let known = sentence(QuickPlot::Volcano, &settings, &facts).unwrap();
        assert!(known.contains("610 higher in treated, 624 higher in control."));
    }

    #[test]
    fn folds_read_plainly() {
        assert_eq!(fold(1.0), "2-fold");
        assert_eq!(fold(0.585), "1.5-fold");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
