//! The composer family takes its text from the theme's tokens, by role,
//! so it reads as one family with Select, Menu, Command, and Textarea.

use std::{fs, path::Path};

/// Every source file of the composer family.
fn sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/composer");
    fs::read_dir(dir)
        .expect("the composer sources")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let text = fs::read_to_string(entry.path()).expect("a readable source");
            (name, text)
        })
        .collect()
}

/// The code of a file, without its tests, doc comments, and comments.
fn code(text: &str) -> String {
    let body = text.split("#[cfg(test)]").next().unwrap_or(text);
    body.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn no_composer_text_has_a_size_or_line_height_of_its_own() {
    for (name, text) in sources() {
        let code = code(&text);
        for forbidden in [
            ".text_size(px(",
            ".text_size(rems(",
            ".line_height(px(",
            ".line_height(rems(",
            "text_composer",
            ".text_xs()",
            ".text_sm()",
            ".text_lg()",
            ".text_xl()",
        ] {
            assert!(
                !code.contains(forbidden),
                "{name} sets its own text with `{forbidden}`"
            );
        }
    }
}

#[test]
fn every_text_style_a_composer_file_sets_comes_from_a_theme_token() {
    let allowed = [
        "text_control",
        "text_textarea",
        "text_heading",
        "text_title",
        "badge_count_text",
        "typography",
    ];
    for (name, text) in sources() {
        for line in code(&text).lines() {
            if line.contains(".text_size(") || line.contains(".line_height(") {
                assert!(
                    allowed.iter().any(|token| line.contains(token))
                        || line.contains("text.size")
                        || line.contains("text.line_height")
                        || line.contains("look.text")
                        || line.contains("look.line_height")
                        || line.contains("look.text_size"),
                    "{name}: `{}` does not read a theme token",
                    line.trim()
                );
            }
        }
    }
}
