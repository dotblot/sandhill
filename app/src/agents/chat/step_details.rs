use super::*;

const SANDHILL: &str = "mcp__sandhill__";

pub(super) fn s(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or_default().to_string()
}

pub(super) fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn line_count(text: Option<&str>) -> usize {
    text.filter(|t| !t.is_empty())
        .map_or(0, |t| t.split('\n').count())
}

pub(super) fn language_name(language: &str) -> String {
    if language.eq_ignore_ascii_case("r") {
        return "R".into();
    }
    let mut chars = language.chars();
    chars
        .next()
        .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a step did, for its row and its card.
pub fn step_view(
    tool: &str,
    input: &Value,
    output: Option<&Value>,
    linked: Option<&str>,
) -> StepView {
    // ACP keeps its display metadata around the tool's original input.
    if input["title"].is_string() && input.get("rawInput").is_some() {
        let mut view = step_view(tool, &input["rawInput"], output, linked);
        view.desc = s(&input["title"]).unwrap_or_default();
        if view.file.is_none() {
            view.file = input["locations"]
                .as_array()
                .and_then(|locations| locations.first())
                .and_then(|location| s(&location["path"]));
        }
        if output.is_none() {
            let content = content_text(&input["content"]);
            if !content.is_empty() {
                view.output = Some(content);
            }
        }
        return view;
    }
    let mut view = StepView {
        name: display_name(tool),
        ..Default::default()
    };
    let null = Value::Null;
    let out = output.unwrap_or(&null);
    if tool.starts_with(SANDHILL) {
        let text = content_text(out);
        let (head, rest) = text.split_once('\n').unwrap_or((&text, ""));
        let ran = sandhill_run(head);
        match view.name.as_str() {
            "run_code" => {
                view.desc = s(&input["description"])
                    .unwrap_or_else(|| first_line(input["code"].as_str().unwrap_or_default()));
                view.code = s(&input["code"]);
                view.language = s(&input["language"]).or(ran.language);
                view.place = Some(head.to_string());
                view.output = Some(rest.trim_end().to_string());
                view.detail = ran.detail;
            }
            "add_cell" | "edit_cell" => {
                let file = s(&input["file"])
                    .or(ran.file)
                    .or(linked.map(str::to_string));
                let in_file = file.clone().unwrap_or("the document".into());
                view.desc = if view.name == "add_cell" {
                    format!("Add a cell to {in_file}")
                } else {
                    format!(
                        "Change {} in {in_file}",
                        input["cell"].as_str().unwrap_or("a cell")
                    )
                };
                view.file = file;
                view.code = s(&input["code"]);
                view.language = s(&input["language"]).or(ran.language);
                view.place = Some(head.to_string());
                view.output = Some(rest.trim_end().to_string());
                let code = input["code"].as_str();
                if view.name == "add_cell" {
                    view.added = Some(line_count(code));
                    view.removed = Some(0);
                    view.diff = Some(
                        code.unwrap_or_default()
                            .split('\n')
                            .map(|l| format!("+{l}"))
                            .collect(),
                    );
                } else {
                    let n = line_count(code);
                    view.detail = Some(plural(n as u64, "line", "lines"));
                }
            }
            "list_variables" => {
                view.desc = match input["language"].as_str() {
                    Some(l) => format!("List the {} session's variables", language_name(l)),
                    None => "List the session's variables".into(),
                };
                view.output = Some(text.clone());
            }
            "list_sessions" => {
                view.desc = "List the running sessions".into();
                view.output = Some(text.clone());
            }
            "get_focus" => {
                view.desc = "See where you are".into();
                view.output = Some(text.clone());
            }
            "get_outputs" => {
                view.desc = match input["cell"].as_str() {
                    Some(c) => format!("Read the outputs of {c}"),
                    None => "Read the outputs".into(),
                };
                view.file = s(&input["file"]);
                view.output = Some(text.clone());
            }
            "list_jobs" => {
                view.desc = "List the jobs".into();
                view.output = Some(text.clone());
            }
            "get_console" => {
                view.desc = "See what ran in the Console".into();
                view.output = Some(text.clone());
            }
            "get_plots" => {
                view.desc = "Look at the plots".into();
                view.output = Some(text.clone());
            }
            "view_table" => {
                view.desc = format!(
                    "Look at {}",
                    input["name"].as_str().unwrap_or("a data frame")
                );
                view.output = Some(text.clone());
            }
            "get_help" => {
                view.desc = format!(
                    "Read the help for {}",
                    input["topic"].as_str().unwrap_or("a function")
                );
                view.output = Some(text.clone());
            }
            "list_packages" => {
                view.desc = "List the installed packages".into();
                view.output = Some(text.clone());
            }
            "delete_cell" => {
                view.desc = format!("Delete {}", input["cell"].as_str().unwrap_or("a cell"));
                view.file = s(&input["file"]);
                view.output = Some(text.clone());
            }
            "read_job_log" => {
                view.desc = match input["job"].as_str() {
                    Some(j) => format!("Read the log of job {j}"),
                    None => "Read the latest job's log".into(),
                };
                view.output = Some(text.clone());
            }
            _ => view.output = Some(text.clone()),
        }
        return view;
    }
    match tool {
        "Bash" => {
            view.desc = s(&input["description"])
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| first_line(input["command"].as_str().unwrap_or_default()));
            view.code = s(&input["command"]);
            view.language = Some("shell".into());
            let parts: Vec<&str> = [&out["stdout"], &out["stderr"]]
                .iter()
                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()))
                .collect();
            view.output = Some(parts.join("\n"));
        }
        "Read" => {
            view.file = s(&input["file_path"]);
            view.desc = base_name(input["file_path"].as_str().unwrap_or_default());
            let file = &out["file"];
            if let Some(content) = file["content"].as_str() {
                let start = file["startLine"].as_u64().unwrap_or(1);
                view.output = Some(
                    content
                        .split('\n')
                        .enumerate()
                        .map(|(i, line)| format!("{:>4}  {line}", start + i as u64))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                if let Some(n) = file["numLines"].as_u64() {
                    let last = start + n.saturating_sub(1);
                    view.detail = Some(match file["totalLines"].as_u64() {
                        Some(total) if total > n => format!("lines {start}–{last} of {total}"),
                        _ => format!("{n} lines"),
                    });
                }
            }
        }
        "Edit" | "MultiEdit" => {
            view.file = s(&input["file_path"]);
            view.desc = format!(
                "Edit {}",
                base_name(input["file_path"].as_str().unwrap_or_default())
            );
            let edits: Vec<&Value> = if tool == "Edit" {
                vec![input]
            } else {
                input["edits"]
                    .as_array()
                    .map_or(vec![], |e| e.iter().collect())
            };
            let diff: Vec<String> = edits
                .iter()
                .flat_map(|e| {
                    let old = e["old_string"].as_str().unwrap_or_default();
                    let new = e["new_string"].as_str().unwrap_or_default();
                    old.split('\n')
                        .map(|l| format!("-{l}"))
                        .chain(new.split('\n').map(|l| format!("+{l}")))
                        .collect::<Vec<_>>()
                })
                .collect();
            set_diff(&mut view, diff);
            view.detail = Some(format!(
                "+{} −{}",
                view.added.unwrap_or(0),
                view.removed.unwrap_or(0)
            ));
        }
        "Write" => {
            view.file = s(&input["file_path"]);
            view.desc = format!(
                "Write {}",
                base_name(input["file_path"].as_str().unwrap_or_default())
            );
            let content = input["content"].as_str();
            view.diff = Some(
                content
                    .unwrap_or_default()
                    .split('\n')
                    .map(|l| format!("+{l}"))
                    .collect(),
            );
            view.added = Some(line_count(content));
            view.removed = Some(0);
            view.detail = Some(format!("+{}", line_count(content)));
        }
        "NotebookEdit" => {
            view.file = s(&input["notebook_path"]);
            view.desc = format!(
                "Edit a cell in {}",
                base_name(input["notebook_path"].as_str().unwrap_or_default())
            );
            view.code = s(&input["new_source"]);
        }
        "Grep" | "Glob" => {
            view.desc = if tool == "Grep" {
                let mut d = format!(
                    "Search for “{}”",
                    input["pattern"].as_str().unwrap_or_default()
                );
                if let Some(path) = input["path"].as_str() {
                    d.push_str(&format!(" in {}", base_name(path)));
                }
                d
            } else {
                format!("Find {}", input["pattern"].as_str().unwrap_or("files"))
            };
            let names = || {
                out["filenames"].as_array().map_or(String::new(), |f| {
                    f.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            };
            view.output = Some(match out["content"].as_str() {
                Some(content) if tool == "Grep" => content.to_string(),
                _ => names(),
            });
            if let Some(n) = out["numFiles"].as_u64() {
                view.detail = Some(plural(n, "file", "files"));
            }
        }
        "WebSearch" => {
            view.desc = s(&input["query"])
                .filter(|q| !q.is_empty())
                .or(s(&out["query"]))
                .unwrap_or_default();
        }
        "WebFetch" => view.desc = s(&input["url"]).unwrap_or_default(),
        "Task" | "Agent" => {
            view.desc = s(&input["description"]).unwrap_or_default();
            let text = content_text(out);
            view.output = Some(if text.is_empty() {
                out.as_str().unwrap_or_default().to_string()
            } else {
                text
            });
        }
        "ToolSearch" => {
            let query = input["query"].as_str().unwrap_or_default();
            view.desc = match query.strip_prefix("select:") {
                Some(names) => {
                    let names: Vec<String> = names
                        .split(',')
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(|n| {
                            if n.starts_with("mcp__") {
                                display_name(n)
                            } else {
                                n.to_string()
                            }
                        })
                        .collect();
                    format!("Load {}", names.join(", "))
                }
                None => format!("Find tools for “{query}”"),
            };
            if let Some(matches) = out["matches"].as_array() {
                view.output = Some(
                    matches
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
        }
        "AskUserQuestion" => {
            let questions = input["questions"].as_array().cloned().unwrap_or_default();
            let answers = &out["answers"];
            view.desc = if questions.len() == 1 {
                s(&questions[0]["question"]).unwrap_or_default()
            } else {
                format!("Ask {} questions", questions.len())
            };
            let given: Vec<&str> = questions
                .iter()
                .filter_map(|q| answers[q["question"].as_str().unwrap_or_default()].as_str())
                .collect();
            if !given.is_empty() {
                view.detail = Some(given.join(", "));
            }
            view.output = Some(
                questions
                    .iter()
                    .map(|q| {
                        let question = q["question"].as_str().unwrap_or_default();
                        format!(
                            "{question}\n› {}",
                            answers[question].as_str().unwrap_or("no answer")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );
        }
        "ExitPlanMode" => {
            view.desc = "Share a plan".into();
            view.output = s(&input["plan"]);
        }
        "apply_patch" => {
            let changes = input["changes"].as_array().cloned().unwrap_or_default();
            let one = (changes.len() == 1).then(|| &changes[0]);
            view.file = one.and_then(|c| s(&c["path"]));
            view.desc = match one {
                Some(change) => patch_title(change),
                None => format!("Edit {} files", changes.len()),
            };
            let diff = changes
                .iter()
                .flat_map(|change| {
                    let mut lines = Vec::new();
                    if one.is_none() {
                        lines.push(format!(" {}", change["path"].as_str().unwrap_or_default()));
                    }
                    lines.extend(patch_lines(change));
                    lines
                })
                .collect();
            set_diff(&mut view, diff);
        }
        "reasoning" => {
            let text = s(&input["text"]).or(s(&out["text"])).unwrap_or_default();
            view.desc = first_line(&text).replace("**", "");
            view.output = Some(text);
        }
        "view_image" => {
            view.file = s(&input["path"]);
            view.desc = format!(
                "Look at {}",
                base_name(input["path"].as_str().unwrap_or_default())
            );
        }
        "permissions" => {
            view.desc = s(&input["reason"]).unwrap_or_default();
            view.output = Some(permission_lines(&input["permissions"]).join("\n"));
        }
        "TodoWrite" => {
            view.desc = "Update the to-do list".into();
            view.output = Some(
                input["todos"]
                    .as_array()
                    .map_or(vec![], |todos| {
                        todos
                            .iter()
                            .map(|t| {
                                let mark = match t["status"].as_str() {
                                    Some("completed") => "✓",
                                    Some("in_progress") => "›",
                                    _ => "○",
                                };
                                format!("{mark} {}", t["content"].as_str().unwrap_or_default())
                            })
                            .collect()
                    })
                    .join("\n"),
            );
        }
        _ => {
            if output.is_some() {
                let text = content_text(out);
                view.output = Some(if text.is_empty() {
                    serde_json::to_string_pretty(out).unwrap_or_default()
                } else {
                    text
                });
            } else if input.as_object().is_some_and(|o| !o.is_empty()) {
                view.output = serde_json::to_string_pretty(input).ok();
            }
        }
    }
    view
}

fn set_diff(view: &mut StepView, diff: Vec<String>) {
    view.added = Some(diff.iter().filter(|l| l.starts_with('+')).count());
    view.removed = Some(diff.iter().filter(|l| l.starts_with('-')).count());
    view.diff = Some(diff);
}

/// "run_code" for Sandhill's tools, "bash" or "read" for the agent's own,
/// "github · create_issue" for other servers'.
pub fn display_name(tool: &str) -> String {
    if let Some(name) = tool.strip_prefix(SANDHILL) {
        return name.into();
    }
    if let Some(rest) = tool.strip_prefix("mcp__") {
        return match rest.split_once("__") {
            Some((server, name)) => format!("{server} · {name}"),
            None => rest.into(),
        };
    }
    match tool {
        "WebSearch" => "web_search",
        "WebFetch" => "web_fetch",
        "TodoWrite" => "todo",
        "MultiEdit" => "edit",
        "NotebookEdit" => "edit_notebook",
        "ToolSearch" => "tools",
        "AskUserQuestion" => "question",
        "ExitPlanMode" => "plan",
        "apply_patch" => "edit",
        "reasoning" => "thinking",
        other => return other.to_lowercase(),
    }
    .into()
}

struct Ran {
    language: Option<String>,
    detail: Option<String>,
    file: Option<String>,
}

/// What Sandhill's run tools say first: "Ran in R session 1a2b (R 4.4.1,
/// kernel ark) · idle · in /data · ok · 0.5 s".
fn sandhill_run(head: &str) -> Ran {
    let words: Vec<&str> = head
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let language = words
        .iter()
        .find(|w| **w == "R" || **w == "Python")
        .map(|w| w.to_string());
    let seconds = head
        .rsplit_once("· ")
        .map(|(_, last)| last.trim())
        .filter(|last| {
            last.strip_suffix(" s")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
        })
        .map(str::to_string);
    let failed = head.contains("· error") || head.contains("· failed");
    let file = file_in(head);
    let parts: Vec<String> = [
        language.clone(),
        if failed {
            Some("error".into())
        } else {
            seconds
        },
    ]
    .into_iter()
    .flatten()
    .collect();
    Ran {
        language: language.map(|l| l.to_lowercase()),
        detail: (!parts.is_empty()).then(|| parts.join(" · ")),
        file,
    }
}

/// "to FILE below|after", else "of NAME.ext".
fn file_in(head: &str) -> Option<String> {
    let words: Vec<&str> = head.split_whitespace().collect();
    for w in words.windows(3) {
        if w[0] == "to" && (w[2] == "below" || w[2] == "after") {
            return Some(w[1].to_string());
        }
    }
    words
        .windows(2)
        .find(|w| w[0] == "of" && looks_like_file(w[1].trim_end_matches([',', ')'])))
        .map(|w| w[1].trim_end_matches([',', ')']).to_string())
}

fn patch_title(change: &Value) -> String {
    let name = base_name(change["path"].as_str().unwrap_or_default());
    if let Some(to) = change["move_path"].as_str() {
        return format!("Move {name} to {}", base_name(to));
    }
    match change["kind"].as_str() {
        Some("add") => format!("Add {name}"),
        Some("delete") => format!("Delete {name}"),
        _ => format!("Edit {name}"),
    }
}

/// A patch's lines with "+", "-" or " " in front; hunks after the first
/// start with " ⋯".
fn patch_lines(change: &Value) -> Vec<String> {
    let diff = change["diff"].as_str().unwrap_or_default();
    let lines = diff.strip_suffix('\n').unwrap_or(diff).split('\n');
    match change["kind"].as_str() {
        Some("add") => return lines.map(|l| format!("+{l}")).collect(),
        Some("delete") => return lines.map(|l| format!("-{l}")).collect(),
        _ => {}
    }
    let mut out = Vec::new();
    for line in lines {
        if line.starts_with("@@") {
            if !out.is_empty() {
                out.push(" ⋯".into());
            }
        } else if !line.starts_with("--- ") && !line.starts_with("+++ ") && !line.starts_with('\\')
        {
            out.push(if line.starts_with(['+', '-', ' ']) {
                line.to_string()
            } else {
                format!(" {line}")
            });
        }
    }
    out
}

/// What more access Codex asks for.
fn permission_lines(permissions: &Value) -> Vec<String> {
    if !permissions.is_object() {
        return vec![];
    }
    let mut lines = Vec::new();
    if permissions["network"]["enabled"].as_bool() == Some(true) {
        lines.push("Use the network".to_string());
    }
    let files = if permissions["fileSystem"].is_object() {
        &permissions["fileSystem"]
    } else {
        &permissions["file_system"]
    };
    for path in files["write"].as_array().into_iter().flatten() {
        lines.push(format!("Write to {}", path.as_str().unwrap_or_default()));
    }
    for path in files["read"].as_array().into_iter().flatten() {
        lines.push(format!("Read {}", path.as_str().unwrap_or_default()));
    }
    if lines.is_empty() {
        lines.push(serde_json::to_string_pretty(permissions).unwrap_or_default());
    }
    lines
}
