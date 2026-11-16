use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 2048;
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Default, Deserialize)]
struct Metadata {
    name: Option<String>,
    description: Option<String>,
}

pub(super) fn list(kind: &str, cwd: &Path, home: Option<&Path>) -> Vec<Value> {
    let mut entries = BTreeMap::new();
    let roots = if kind == "claude" {
        vec![Some(cwd.join(".claude")), home.map(|h| h.join(".claude"))]
    } else {
        // agy's installed documentation identifies .agents and .gemini/config
        // as discovery roots. Claude/Codex paths appear only in import docs.
        vec![
            Some(cwd.join(".agents")),
            home.map(|h| h.join(".gemini/config")),
        ]
    };
    for root in roots.into_iter().flatten() {
        if kind == "claude" {
            scan(&root.join("commands"), false, &mut entries);
        }
        scan(&root.join("skills"), true, &mut entries);
    }
    if kind == "claude" {
        for (name, description) in [
            ("init", "Create project instructions for Claude Code"),
            ("review", "Review code changes"),
            (
                "security-review",
                "Review changes for security vulnerabilities",
            ),
        ] {
            entries
                .entry(name.into())
                .or_insert_with(|| super::item("command", name, description));
        }
    }
    entries.into_values().collect()
}

fn scan(root: &Path, skills: bool, entries: &mut BTreeMap<String, Value>) {
    let mut remaining = MAX_ENTRIES;
    visit(root, root, skills, 0, &mut remaining, entries);
}

fn visit(
    root: &Path,
    dir: &Path,
    skills: bool,
    depth: usize,
    remaining: &mut usize,
    entries: &mut BTreeMap<String, Value>,
) {
    let Ok(children) = fs::read_dir(dir) else {
        return;
    };
    // Bound directory work as well as the number of files we read. Do not
    // follow symlinks into unrelated agent configuration or recursive loops.
    let mut children: Vec<_> = children.take(*remaining).filter_map(Result::ok).collect();
    *remaining -= children.len();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let Ok(ty) = entry.file_type() else { continue };
        let path = entry.path();
        if ty.is_dir() && depth < if skills { 1 } else { MAX_DEPTH } {
            visit(root, &path, skills, depth + 1, remaining, entries);
        } else if ty.is_file()
            && ((!skills && path.extension().is_some_and(|ext| ext == "md"))
                || (skills && depth == 1 && entry.file_name() == "SKILL.md"))
        {
            let Some(text) = read(&path) else { continue };
            let (metadata, body) = metadata(&text);
            let fallback = if skills {
                dir.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                let Ok(relative) = path.strip_prefix(root) else {
                    continue;
                };
                relative
                    .with_extension("")
                    .iter()
                    .map(|s| s.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(":")
            };
            let name = if skills {
                metadata
                    .name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&fallback)
            } else {
                &fallback
            };
            let item = super::item(
                if skills { "skill" } else { "command" },
                name,
                metadata.description.as_deref().unwrap_or_else(|| {
                    if skills {
                        ""
                    } else {
                        body.lines().find(|s| !s.trim().is_empty()).unwrap_or("")
                    }
                }),
            );
            if let Some(name) = item["name"].as_str().filter(|s| !s.is_empty()) {
                entries.entry(name.to_string()).or_insert(item);
            }
        }
    }
}

fn read(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= MAX_BYTES).then_some(text)
}

fn metadata(text: &str) -> (Metadata, &str) {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim) != Some("---") {
        return (Metadata::default(), text);
    }
    let start = text.find('\n').map_or(text.len(), |n| n + 1);
    let mut end = start;
    for line in lines {
        if matches!(line.trim(), "---" | "...") {
            return (
                serde_yaml_ng::from_str(&text[start..end]).unwrap_or_default(),
                &text[end + line.len()..],
            );
        }
        end += line.len();
    }
    (Metadata::default(), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_quotes_blocks_and_body_fallback() {
        let (parsed, body) = metadata(
            "---\r\nname: 'my-skill'\r\ndescription: >-\r\n  One line\r\n  from two.\r\n---\r\n\n# Body\n",
        );
        assert_eq!(parsed.name.as_deref(), Some("my-skill"));
        assert_eq!(parsed.description.as_deref(), Some("One line from two."));
        assert_eq!(body.trim(), "# Body");
        let (parsed, _) =
            metadata("---\nname: \"quoted\\\"name\"\ndescription: 'It''s useful: yes'\n---\n");
        assert_eq!(parsed.name.as_deref(), Some("quoted\"name"));
        assert_eq!(parsed.description.as_deref(), Some("It's useful: yes"));
        assert!(metadata("plain markdown").0.description.is_none());
        assert!(metadata("---\n[invalid\n---\nbody").0.description.is_none());
    }

    #[test]
    fn project_entries_win_and_scans_stay_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        for base in [&project, &home] {
            fs::create_dir_all(base.join(".claude/commands/nested")).unwrap();
            fs::create_dir_all(base.join(".claude/skills/folder")).unwrap();
            fs::write(
                base.join(".claude/commands/nested/review.md"),
                "\n Review body\n",
            )
            .unwrap();
            fs::write(
                base.join(".claude/skills/folder/SKILL.md"),
                "---\nname: preferred\ndescription: inspect data\n---\nsecret body",
            )
            .unwrap();
        }
        fs::write(
            project.join(".claude/commands/nested/review.md"),
            "---\ndescription: Project review\n---\nDo not return this body",
        )
        .unwrap();
        fs::write(
            project.join(".claude/commands/huge.md"),
            "x".repeat(MAX_BYTES as usize + 1),
        )
        .unwrap();
        fs::write(home.join(".claude/commands/user.md"), "\n User command\n").unwrap();
        fs::create_dir_all(project.join(".claude/skills/fallback")).unwrap();
        fs::write(
            project.join(".claude/skills/fallback/SKILL.md"),
            "body only",
        )
        .unwrap();
        let deep = project
            .join(".claude/commands")
            .join("d/".repeat(MAX_DEPTH + 1));
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("hidden.md"), "too deep").unwrap();
        std::os::unix::fs::symlink(
            home.join(".claude/commands/user.md"),
            project.join(".claude/commands/linked.md"),
        )
        .unwrap();
        let items = list("claude", &project, Some(&home));
        let command = items.iter().find(|i| i["name"] == "nested:review").unwrap();
        assert_eq!(command["description"], "Project review");
        assert_eq!(items.iter().filter(|i| i["name"] == "preferred").count(), 1);
        assert_eq!(
            items.iter().find(|i| i["name"] == "fallback").unwrap()["description"],
            ""
        );
        assert_eq!(
            items.iter().find(|i| i["name"] == "user").unwrap()["description"],
            "User command"
        );
        assert!(
            !items
                .iter()
                .any(|i| ["huge", "linked"].iter().any(|n| i["name"] == *n)
                    || i["name"].as_str().unwrap().ends_with("hidden"))
        );
        assert!(!items.iter().any(|i| i.get("prompt").is_some()));
    }
}
