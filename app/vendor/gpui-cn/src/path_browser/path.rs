//! The pure logic the folder and file pickers share: how the path text splits into a
//! directory and a query, how a query ranks the folders, and which folder
//! a typed separator goes into. It works on plain strings, so it needs no
//! window and no file system.

use super::{Entry, SourcePath, style::PathStyle};
use gpui_kit::SharedString;

pub(crate) use crate::fuzzy::Match;

/// Splits the path text at its last separator: the directory, with its
/// separator, and the query after it. Text with no separator has the
/// root as its directory, and a lone drive such as `C:` is that drive's
/// root with no query.
pub(crate) fn split_path(text: &str, style: &PathStyle) -> (String, String) {
    match text.rfind(|c| style.is_separator(c)) {
        Some(at) => (text[..=at].to_string(), text[at + 1..].to_string()),
        None if style.is_windows() && is_drive_only(text) => {
            (format!("{text}{}", style.separator()), String::new())
        }
        None => (style.separator().to_string(), text.to_string()),
    }
}

fn is_drive_only(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(a), Some(':'), None) if a.is_ascii_alphabetic()
    )
}

/// The text of the directory above `dir`, with a trailing separator. A
/// root is its own parent, except that a drive root goes up to the root
/// above the drives when the style has one.
pub(crate) fn parent_text(dir: &str, style: &PathStyle) -> String {
    let mut parsed = style.parse(dir);
    let sep = style.separator();
    if parsed.parts.is_empty() {
        let is_drive = parsed.prefix.len() == 3 && parsed.prefix.as_bytes()[1] == b':';
        if is_drive && style.has_computer_root() {
            return sep.to_string();
        }
        return if dir.is_empty() {
            sep.to_string()
        } else {
            dir.to_string()
        };
    }
    parsed.parts.pop();
    if parsed.parts.is_empty() && !parsed.rooted {
        return sep.to_string();
    }
    let mut text = style.display(&parsed);
    if !text.ends_with(sep) {
        text.push(sep);
    }
    text
}

/// `dir` with `name` inside it, and a trailing separator. The separator
/// is the one the user last typed in `dir`, so a path typed with `/` stays
/// in `/` in a Windows style, and the style's own when `dir` has none.
pub(crate) fn join_dir(dir: &str, name: &str, style: &PathStyle) -> String {
    let sep = dir
        .chars()
        .rev()
        .find(|c| style.is_separator(*c))
        .unwrap_or(style.separator());
    let mut text = dir.to_string();
    if !text.ends_with(|c| style.is_separator(c)) {
        text.push(sep);
    }
    text.push_str(name);
    text.push(sep);
    text
}

/// The path a directory's text names: no trailing separator, except for
/// the root.
pub(crate) fn directory_path(dir: &str, style: &PathStyle) -> SourcePath {
    if dir.is_empty() {
        return style.path(style.separator().to_string());
    }
    style.path(dir)
}

/// Whether the directory text starts at the home folder: `~` alone, or
/// `~` and a separator.
pub(crate) fn is_home_text(dir: &str, style: &PathStyle) -> bool {
    dir == "~"
        || dir
            .strip_prefix('~')
            .is_some_and(|rest| rest.starts_with(|c| style.is_separator(c)))
}

/// The path a directory's text names, with a leading `~` standing for
/// `home` when there is one.
pub(crate) fn resolve_dir(dir: &str, home: Option<&SourcePath>, style: &PathStyle) -> SourcePath {
    match home {
        Some(home) if is_home_text(dir, style) => {
            let rest = dir[1..].trim_matches(|c| style.is_separator(c));
            if rest.is_empty() {
                home.clone()
            } else {
                home.join(rest)
            }
        }
        _ => directory_path(dir, style),
    }
}

/// The text of a directory under `home` in the `~/...` form, with a
/// trailing separator, and `text` itself when it is not under `home`. A
/// path is under `home` by the style's own comparison, so `C:/Users/Me`
/// is under `c:\users\me`.
pub(crate) fn collapse_home(text: &str, home: Option<&SourcePath>, style: &PathStyle) -> String {
    let Some(home) = home else {
        return text.to_string();
    };
    let home = style.parse(home.as_str());
    let at = style.parse(text);
    let sep = text
        .chars()
        .rev()
        .find(|c| style.is_separator(*c))
        .unwrap_or(style.separator());
    if home.parts.is_empty() || style.key(&home.prefix) != style.key(&at.prefix) {
        return text.to_string();
    }
    let under = at.parts.len() >= home.parts.len()
        && home
            .parts
            .iter()
            .zip(&at.parts)
            .all(|(a, b)| style.fold(a) == style.fold(b));
    if !under {
        return text.to_string();
    }
    let mut out = format!("~{sep}");
    for part in &at.parts[home.parts.len()..] {
        out.push_str(part);
        out.push(sep);
    }
    out
}

/// Ranks `entries` by how well `query` matches their names: a subsequence
/// of the name, ignoring case, scored so that runs and word starts rank
/// first. Ties go to the shorter name, then keep the listing's order. The folders that do not match
/// are left out.
#[cfg(test)]
pub(crate) fn filter<E: Entry>(entries: &[E], query: &str) -> Vec<Match> {
    rank(entries, &|_| true, query)
}

/// Every visible entry in the listing's order, with no matched ranges: what
/// an empty query shows.
pub(crate) fn all_visible<E: Entry>(entries: &[E], visible: &dyn Fn(&E) -> bool) -> Vec<Match> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| visible(entry))
        .map(|(index, _)| Match {
            index,
            score: 0,
            ranges: Vec::new(),
        })
        .collect()
}

/// [`filter`] over the entries `visible` lets through, ranked by
/// [`crate::fuzzy::rank`] on their names.
pub(crate) fn rank<E: Entry>(
    entries: &[E],
    visible: &dyn Fn(&E) -> bool,
    query: &str,
) -> Vec<Match> {
    let shown: Vec<(usize, &E)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| visible(entry))
        .collect();
    crate::fuzzy::rank(query, &shown, |(_, entry)| entry.name())
        .into_iter()
        .map(|found| Match {
            index: shown[found.index].0,
            ..found
        })
        .collect()
}

/// The folder a typed separator goes into: the one named exactly like the
/// query, or else the best match. `None` when the query is empty or
/// nothing matches.
pub(crate) fn resolve_descend<E: Entry>(
    entries: &[E],
    matches: &[Match],
    query: &str,
    style: &PathStyle,
) -> Option<SharedString> {
    if query.is_empty() {
        return None;
    }
    entries
        .iter()
        .find(|entry| entry.is_folder() && style.fold(entry.name()) == style.fold(query))
        .or_else(|| {
            matches
                .iter()
                .filter_map(|found| entries.get(found.index))
                .find(|entry| entry.is_folder())
        })
        .map(|entry| entry.name().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestEntry(SharedString);

    impl Entry for TestEntry {
        fn name(&self) -> &SharedString {
            &self.0
        }

        fn hidden(&self) -> bool {
            self.0.starts_with('.')
        }

        fn is_folder(&self) -> bool {
            true
        }
    }

    fn entries(names: &[&str]) -> Vec<TestEntry> {
        names
            .iter()
            .map(|name| TestEntry(SharedString::from(name.to_string())))
            .collect()
    }

    fn names(entries: &[TestEntry], matches: &[Match]) -> Vec<String> {
        matches
            .iter()
            .map(|found| entries[found.index].name().to_string())
            .collect()
    }

    fn posix() -> PathStyle {
        PathStyle::posix()
    }

    fn windows() -> PathStyle {
        PathStyle::windows()
    }

    #[test]
    fn a_leading_tilde_names_the_home_when_there_is_one() {
        let style = posix();
        let home = Some(style.path("/Users/me"));
        let at = |text: &str, home: Option<&SourcePath>| {
            resolve_dir(text, home, &style).as_str().to_string()
        };
        assert_eq!(at("~/", home.as_ref()), "/Users/me");
        assert_eq!(at("~", home.as_ref()), "/Users/me");
        assert_eq!(at("~/code/", home.as_ref()), "/Users/me/code");
        assert_eq!(at("~/", None), "~", "no home: a plain name");
        assert_eq!(at("/~/", home.as_ref()), "/~", "only a leading tilde");
        assert!(!is_home_text("~me/", &style));
    }

    #[test]
    fn a_directory_under_home_takes_the_tilde_form() {
        let style = posix();
        let home = Some(style.path("/Users/me"));
        let at = |text: &str| collapse_home(text, home.as_ref(), &style);
        assert_eq!(at("/Users/me/"), "~/");
        assert_eq!(at("/Users/me/code/"), "~/code/");
        assert_eq!(at("/Users/"), "/Users/");
        assert_eq!(at("/Users/meow/"), "/Users/meow/", "not a child of home");
        assert_eq!(collapse_home("/Users/me/", None, &style), "/Users/me/");
    }

    #[test]
    fn home_is_compared_by_the_styles_case_rule_and_either_separator() {
        let style = windows();
        let home = Some(style.path("C:\\Users\\Me"));
        let at = |text: &str| collapse_home(text, home.as_ref(), &style);
        assert_eq!(
            at("c:/users/me/code/"),
            "~/code/",
            "the typed separator stays"
        );
        assert_eq!(at("c:\\users\\me\\code\\"), "~\\code\\");
        assert_eq!(at("C:\\Users\\Me\\"), "~\\");
        assert_eq!(at("D:\\Users\\Me\\"), "D:\\Users\\Me\\");
        assert_eq!(
            collapse_home("/Users/me/", Some(&posix().path("/Users/me")), &posix()),
            "~/"
        );
        let upper = Some(posix().path("/Users/Me"));
        assert_eq!(
            collapse_home("/users/me/", upper.as_ref(), &posix()),
            "/users/me/",
            "a POSIX style is case-sensitive"
        );
    }

    #[test]
    fn a_path_splits_at_its_last_separator() {
        let split = |text: &str| split_path(text, &posix());
        assert_eq!(split("/home/me/co"), ("/home/me/".into(), "co".into()));
        assert_eq!(split("/home/me/"), ("/home/me/".into(), "".into()));
        assert_eq!(split("/"), ("/".into(), "".into()));
        assert_eq!(split("/abc"), ("/".into(), "abc".into()));
        assert_eq!(split("abc"), ("/".into(), "abc".into()));
        assert_eq!(split(""), ("/".into(), "".into()));
    }

    #[test]
    fn windows_paths_split_at_either_separator() {
        let split = |text: &str| split_path(text, &windows());
        assert_eq!(
            split("C:\\Users\\me/co"),
            ("C:\\Users\\me/".into(), "co".into())
        );
        assert_eq!(split("C:\\"), ("C:\\".into(), "".into()));
        assert_eq!(
            split("C:"),
            ("C:\\".into(), "".into()),
            "a lone drive is a root"
        );
        assert_eq!(split("c"), ("\\".into(), "c".into()));
        assert_eq!(
            split_path("a\\b", &posix()),
            ("/".into(), "a\\b".into()),
            "a backslash is a name character on POSIX"
        );
    }

    #[test]
    fn the_parent_keeps_a_trailing_separator_and_the_root_stays() {
        let up = |text: &str, style: PathStyle| parent_text(text, &style);
        assert_eq!(up("/home/me/", posix()), "/home/");
        assert_eq!(up("/home/", posix()), "/");
        assert_eq!(up("/", posix()), "/");
        assert_eq!(up("C:\\Users\\", windows()), "C:\\");
        assert_eq!(up("C:/Users/me/", windows()), "C:\\Users\\");
        assert_eq!(up("C:\\", windows()), "C:\\");
        assert_eq!(up("\\\\srv\\share\\dir\\", windows()), "\\\\srv\\share\\");
        assert_eq!(
            up("\\\\srv\\share\\", windows()),
            "\\\\srv\\share\\",
            "a share stays"
        );
        assert_eq!(
            up("C:\\", PathStyle::windows().with_computer_root(true)),
            "\\",
            "above the drive, when the source lists drives"
        );
        assert_eq!(
            up("\\", PathStyle::windows().with_computer_root(true)),
            "\\"
        );
    }

    #[test]
    fn a_folder_joins_with_separators_on_both_sides() {
        assert_eq!(join_dir("/home/", "code", &posix()), "/home/code/");
        assert_eq!(join_dir("/", ".config", &posix()), "/.config/");
        assert_eq!(join_dir("C:\\", "Users", &windows()), "C:\\Users\\");
        assert_eq!(join_dir("C:/Users", "me", &windows()), "C:/Users/me/");
        assert_eq!(join_dir("C:Users", "me", &windows()), "C:Users\\me\\");
    }

    #[test]
    fn a_directory_path_drops_the_trailing_separator_except_at_the_root() {
        let at = |text: &str, style: PathStyle| directory_path(text, &style).as_str().to_string();
        assert_eq!(at("/home/me/", posix()), "/home/me");
        assert_eq!(at("/", posix()), "/");
        assert_eq!(at("C:\\", windows()), "C:\\");
        assert_eq!(at("C:\\Users\\", windows()), "C:\\Users");
        assert_eq!(at("C:/Users/me/", windows()), "C:\\Users\\me");
        assert_eq!(at("", windows()), "\\");
    }

    #[test]
    fn windows_texts_that_name_one_folder_are_the_same_directory() {
        let style = windows();
        let same = [
            "C:/Users/me",
            "C:\\Users\\me",
            "C:/Users\\me/",
            "c:\\USERS\\Me\\",
        ];
        let first = directory_path(same[0], &style);
        for text in same {
            assert_eq!(directory_path(text, &style), first, "{text}");
        }
        assert_ne!(directory_path("C:/Users/you", &style), first);
    }

    #[test]
    fn a_query_ranks_a_subsequence_and_marks_the_matched_characters() {
        let all = entries(&["code", ".cache", ".codex", ".config", "docs"]);
        let found = filter(&all, "co");
        assert_eq!(names(&all, &found), ["code", ".codex", ".config"]);
        let code = found.iter().find(|m| m.index == 0).unwrap();
        assert_eq!(code.ranges, vec![0..2]);
        let found = filter(&all, "cx");
        assert_eq!(names(&all, &found), [".codex"]);
        assert_eq!(found[0].ranges, vec![1..2, 5..6]);
    }

    #[test]
    fn matching_ignores_case_and_leaves_out_what_does_not_match() {
        let all = entries(&["Documents", "Downloads", "music"]);
        assert_eq!(names(&all, &filter(&all, "DOC")), ["Documents"]);
        assert!(filter(&all, "zzz").is_empty());
    }

    #[test]
    fn equal_scores_go_to_the_shorter_name_then_the_listing_order() {
        let all = entries(&["a1", "a2", "a3", "a"]);
        assert_eq!(names(&all, &filter(&all, "a")), ["a", "a1", "a2", "a3"]);
    }

    #[test]
    fn multibyte_names_give_byte_ranges() {
        let all = entries(&["\u{e9}t\u{e9}"]);
        let found = filter(&all, "t");
        assert_eq!(found[0].ranges, vec![2..3]);
    }

    #[test]
    fn a_separator_goes_into_the_exact_folder_or_else_the_best_match() {
        let all = entries(&["code", "codex", "docs"]);
        let found = filter(&all, "code");
        assert_eq!(
            resolve_descend(&all, &found, "code", &posix()).as_deref(),
            Some("code")
        );
        let found = filter(&all, "cdx");
        assert_eq!(
            resolve_descend(&all, &found, "cdx", &posix()).as_deref(),
            Some("codex")
        );
        let found = filter(&all, "zzz");
        assert_eq!(resolve_descend(&all, &found, "zzz", &posix()), None);
        assert_eq!(resolve_descend(&all, &filter(&all, ""), "", &posix()), None);
    }

    #[test]
    fn an_exact_name_wins_over_a_better_ranked_match() {
        let all = entries(&["codex", "co"]);
        let found = filter(&all, "co");
        assert_eq!(names(&all, &found)[0], "co");
        let all = entries(&["cooking", "co"]);
        let found = filter(&all, "co");
        assert_eq!(
            resolve_descend(&all, &found, "co", &posix()).as_deref(),
            Some("co")
        );
    }
}
