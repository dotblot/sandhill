//! A data frame as a kernel shows it in HTML (pandas, polars and
//! IRkernel all write a `<table class="dataframe">`), read into rows of
//! text for a grid under the cell; and the name of the data frame a cell
//! shows, for opening it in the Data Explorer.

/// A data frame's table: its column names, its rows (as the kernel cut
/// them short), and what the kernel says about its size.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Such as `100 rows × 3 columns` or `A data.frame: 3 × 2`.
    pub caption: Option<String>,
}

/// The data frame in `html`, if it is one.
pub fn parse(html: &str) -> Option<Table> {
    let lower = html.to_ascii_lowercase();
    if lower.contains("<script") {
        return None;
    }
    let start = lower.find("<table")?;
    let open_end = start + lower[start..].find('>')?;
    if !lower[start..open_end].contains("dataframe") {
        return None;
    }
    let end = lower[start..]
        .find("</table")
        .map_or(lower.len(), |e| start + e);
    let body = &html[start..end];
    let body_lower = &lower[start..end];

    let mut caption = between(body, body_lower, "<caption", "</caption").map(|s| text_of(&s));
    let mut head_rows: Vec<Vec<String>> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let head_end = body_lower.find("</thead").unwrap_or(0);
    let mut at = 0;
    while let Some(tr) = body_lower[at..].find("<tr") {
        let row_start = at + tr;
        let row_end = body_lower[row_start..]
            .find("</tr")
            .map_or(body_lower.len(), |e| row_start + e);
        let cells = row_cells(&body[row_start..row_end], &body_lower[row_start..row_end]);
        if row_start < head_end {
            head_rows.push(cells);
        } else {
            rows.push(cells);
        }
        at = row_end + 1;
        if at >= body_lower.len() {
            break;
        }
    }
    // No <thead>: a first row of <th>s is the header.
    if head_rows.is_empty() && !rows.is_empty() {
        let first = &body_lower[body_lower.find("<tr")?..];
        let first = &first[..first.find("</tr").unwrap_or(first.len())];
        if !first.contains("<td") {
            head_rows.push(rows.remove(0));
        }
    }
    let width = head_rows
        .iter()
        .chain(&rows)
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    if width == 0 {
        return None;
    }
    // Each column's name is the first one given: pandas puts the index's
    // name on a row of its own, IRkernel the columns' types.
    let columns = (0..width)
        .map(|i| {
            head_rows
                .iter()
                .filter_map(|r| r.get(i))
                .find(|s| !s.is_empty())
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    for row in &mut rows {
        row.resize(width, String::new());
    }
    if caption.as_deref().is_none_or(str::is_empty) {
        // pandas says the size after the table, polars before it.
        let after = text_of(&html[end..]);
        let before = between(&html[..start], &lower[..start], "<small", "</small")
            .map(|s| text_of(&s))
            .unwrap_or_default();
        caption = [after, before].into_iter().find(|s| !s.is_empty());
    }
    Some(Table {
        columns,
        rows,
        caption: caption.filter(|s| !s.is_empty()),
    })
}

/// The text of the first `open … close` element in `html`, tags and all
/// inside it.
fn between(html: &str, lower: &str, open: &str, close: &str) -> Option<String> {
    let start = lower.find(open)?;
    let inner = start + lower[start..].find('>')? + 1;
    let end = inner + lower[inner..].find(close)?;
    Some(html[inner..end].to_string())
}

/// A row's cells, a cell spanning columns repeated for each.
fn row_cells(row: &str, lower: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut at = 0;
    loop {
        let th = lower[at..].find("<th");
        let td = lower[at..].find("<td");
        let Some(start) = [th, td].into_iter().flatten().min().map(|i| at + i) else {
            break;
        };
        let Some(tag_end) = lower[start..].find('>').map(|e| start + e) else {
            break;
        };
        let tag = &lower[start..tag_end];
        // `<thead>` is not a cell.
        if tag.starts_with("<thead") {
            at = tag_end + 1;
            continue;
        }
        let close = if tag.starts_with("<th") {
            "</th"
        } else {
            "</td"
        };
        let end = lower[tag_end..]
            .find(close)
            .map_or(lower.len(), |e| tag_end + e);
        let text = text_of(&row[tag_end + 1..end]);
        let span = tag
            .split("colspan")
            .nth(1)
            .and_then(|s| {
                s.trim_start_matches(['=', '"', '\'', ' '])
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(1)
            .clamp(1, 64);
        for _ in 0..span {
            cells.push(text.clone());
        }
        at = end;
    }
    cells
}

/// HTML as text: tags dropped, entities read, spaces collapsed.
fn text_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_style = false;
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if in_style {
            match lower[i..].find("</style") {
                Some(e) => {
                    i += e;
                    in_style = false;
                }
                None => break,
            }
            continue;
        }
        let c = bytes[i];
        if c == b'<' {
            in_tag = true;
            if lower[i..].starts_with("<style") {
                in_style = true;
            }
            out.push(' ');
        } else if c == b'>' {
            in_tag = false;
        } else if !in_tag {
            let rest = &html[i..];
            let ch = rest.chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    let decoded = decode_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(10)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let ch = match name {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "times" => Some('×'),
            "hellip" => Some('…'),
            _ if name.starts_with("#x") || name.starts_with("#X") => {
                u32::from_str_radix(&name[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(ch) => {
                out.push(ch);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The data frame a cell shows, from its last line: a bare name (`df`),
/// or a name's first or last rows (`df.head()`, `head(df)`). The Data
/// Explorer opens it by that name.
pub fn shown_name(code: &str) -> Option<String> {
    let line = code.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_end_matches(';').trim();
    let is_name = |s: &str| {
        let mut chars = s.chars();
        chars
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '.')
            && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            && !s.ends_with('.')
    };
    // `df`. (A dotted name is an attribute in Python.)
    if is_name(line) && !line.contains('.') {
        return Some(line.to_string());
    }
    // df.head() / df.tail(10)
    for method in [".head(", ".tail("] {
        if let Some(at) = line.find(method)
            && line.ends_with(')')
            && is_name(&line[..at])
            && !line[..at].contains('.')
        {
            return Some(line[..at].to_string());
        }
    }
    // head(df) / tail(df, 10)
    for function in ["head(", "tail("] {
        if let Some(inner) = line
            .strip_prefix(function)
            .and_then(|s| s.strip_suffix(')'))
        {
            let name = inner.split(',').next()?.trim();
            if is_name(name) {
                return Some(name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Table, parse, shown_name};

    #[test]
    fn reads_a_pandas_data_frame() {
        let html = r#"<div>
<style scoped>.dataframe tbody tr th { vertical-align: top; }</style>
<table border="1" class="dataframe">
  <thead>
    <tr style="text-align: right;"><th></th><th>count</th><th>padj</th></tr>
    <tr><th>gene</th><th></th><th></th></tr>
  </thead>
  <tbody>
    <tr><th>TP53</th><td>120</td><td>0.001</td></tr>
    <tr><th>A&amp;B</th><td>45</td><td>&lt;0.2</td></tr>
  </tbody>
</table>
<p>2 rows &times; 2 columns</p>
</div>"#;
        assert_eq!(
            parse(html).unwrap(),
            Table {
                columns: vec!["gene".into(), "count".into(), "padj".into()],
                rows: vec![
                    vec!["TP53".into(), "120".into(), "0.001".into()],
                    vec!["A&B".into(), "45".into(), "<0.2".into()],
                ],
                caption: Some("2 rows × 2 columns".into()),
            }
        );
    }

    #[test]
    fn reads_an_irkernel_data_frame_and_its_caption() {
        let html = "<table class=\"dataframe\">\n<caption>A data.frame: 2 × 2</caption>\n\
            <thead>\n\t<tr><th></th><th scope=col>x</th><th scope=col>y</th></tr>\n\
            \t<tr><th></th><th scope=col>&lt;dbl&gt;</th><th scope=col>&lt;chr&gt;</th></tr>\n</thead>\n\
            <tbody>\n\t<tr><th scope=row>1</th><td>1</td><td>a</td></tr>\n\
            \t<tr><th scope=row>2</th><td>2</td><td>b</td></tr>\n</tbody>\n</table>\n";
        let table = parse(html).unwrap();
        assert_eq!(table.columns, ["", "x", "y"]);
        assert_eq!(table.rows[1], ["2", "2", "b"]);
        assert_eq!(table.caption.as_deref(), Some("A data.frame: 2 × 2"));
    }

    #[test]
    fn spans_columns_and_leaves_other_html_alone() {
        let html = "<small>shape: (1, 2)</small><table class=\"dataframe\"><thead>\
            <tr><th colspan=\"2\">both</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>";
        let table = parse(html).unwrap();
        assert_eq!(table.columns, ["both", "both"]);
        assert_eq!(table.caption.as_deref(), Some("shape: (1, 2)"));
        assert!(parse("<table><tr><td>1</td></tr></table>").is_none());
        assert!(parse("<b>bold</b>").is_none());
        assert!(parse("<script></script><table class=\"dataframe\"></table>").is_none());
    }

    #[test]
    fn finds_the_data_frame_a_cell_shows() {
        assert_eq!(shown_name("df = load()\ndf\n").as_deref(), Some("df"));
        assert_eq!(shown_name("counts.head()").as_deref(), Some("counts"));
        assert_eq!(shown_name("head(counts, 10)").as_deref(), Some("counts"));
        assert_eq!(shown_name("x;").as_deref(), Some("x"));
        assert_eq!(shown_name("df.describe()"), None);
        assert_eq!(shown_name("df[df.x > 1]"), None);
        assert_eq!(shown_name(""), None);
    }
}
