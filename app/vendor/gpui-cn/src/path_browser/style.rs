//! How a source writes its paths. A picker never assumes the host's rules:
//! a Windows client can browse a Linux server and a Linux client can browse
//! a Windows machine, so every source names its [`PathStyle`] and the picker
//! parses, joins, compares and sorts with that.

use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use gpui_kit::SharedString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Posix,
    Windows,
}

/// The rules of a source's paths: which characters separate the parts, how
/// a root looks, whether names differ by case, and which names are hidden.
///
/// [`posix`](Self::posix) is `/`-separated with one root, case-sensitive,
/// with dot names hidden. [`windows`](Self::windows) has drive roots such
/// as `C:\` and share roots such as `\\server\share\`, accepts both `/` and
/// `\` when a path is typed or pasted, writes `\`, ignores case, and hides
/// nothing by name. [`host`](Self::host) is the style of the machine the
/// application runs on, the default of a source.
///
/// ```
/// use gpui_cn::PathStyle;
///
/// let windows = PathStyle::windows();
/// let path = windows.path("c:/Users\\me/");
/// assert_eq!(path.as_str(), "c:\\Users\\me");
/// assert_eq!(path, windows.path("C:\\users\\ME"));
/// assert_eq!(windows.path("C:\\").parent(), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathStyle {
    kind: Kind,
    display_separator: char,
    case_sensitive: bool,
    computer_root: bool,
}

impl PathStyle {
    /// `/` paths with one root, case-sensitive.
    pub fn posix() -> Self {
        Self {
            kind: Kind::Posix,
            display_separator: '/',
            case_sensitive: true,
            computer_root: false,
        }
    }

    /// Drive and share roots, `\` as the written separator, with `/`
    /// accepted too, and names that ignore case.
    pub fn windows() -> Self {
        Self {
            kind: Kind::Windows,
            display_separator: '\\',
            case_sensitive: false,
            computer_root: false,
        }
    }

    /// The style of the machine the application runs on.
    pub fn host() -> Self {
        if cfg!(windows) {
            Self::windows()
        } else {
            Self::posix()
        }
    }

    /// Writes `/` instead of `\` in a Windows-style path. A POSIX style
    /// always writes `/`.
    pub fn with_display_separator(mut self, separator: char) -> Self {
        if self.kind == Kind::Windows && (separator == '/' || separator == '\\') {
            self.display_separator = separator;
        }
        self
    }

    /// Sets whether two names that differ only by case are different names.
    pub fn with_case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    /// Whether a Windows-style source lists its drives as a root above the
    /// drive roots, as "This PC" does. With it, Up from `C:\` goes to that
    /// listing, which the source answers for the path `\`. Without it, a
    /// drive root is its own parent.
    pub fn with_computer_root(mut self, computer_root: bool) -> Self {
        self.computer_root = self.kind == Kind::Windows && computer_root;
        self
    }

    /// Whether this is a Windows style.
    pub fn is_windows(&self) -> bool {
        self.kind == Kind::Windows
    }

    /// The separator the picker writes.
    pub fn separator(&self) -> char {
        self.display_separator
    }

    /// Whether `c` separates the parts of a path when it is typed.
    pub fn is_separator(&self, c: char) -> bool {
        c == '/' || (self.is_windows() && c == '\\')
    }

    /// Whether names that differ only by case are different names.
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Whether the source lists a root above the drives.
    pub fn has_computer_root(&self) -> bool {
        self.computer_root
    }

    /// Whether a name is hidden by the style's own rule: a leading dot on
    /// a POSIX style, nothing on a Windows style, where hidden is an
    /// attribute the source reads.
    pub fn is_hidden_name(&self, name: &str) -> bool {
        !self.is_windows() && name.starts_with('.')
    }

    /// The name as the style compares it: lower case when case does not
    /// matter.
    pub fn fold<'a>(&self, name: &'a str) -> Cow<'a, str> {
        if self.case_sensitive {
            Cow::Borrowed(name)
        } else {
            Cow::Owned(name.to_lowercase())
        }
    }

    /// Orders two names the way a listing sorts them: without regard to
    /// case, then by the name itself, whatever the style's case rule.
    pub fn compare_names(&self, a: &str, b: &str) -> Ordering {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    }

    /// The path `text` names in this style.
    pub fn path(&self, text: impl AsRef<str>) -> SourcePath {
        SourcePath::new(text.as_ref(), *self)
    }

    /// The path text for a pasted `text`: a `file://` URL becomes a path,
    /// and a Windows-style path loses its surrounding quotes and spaces,
    /// takes the style's separator, and has a lone drive letter made a
    /// drive root. `%NAME%` in a Windows-style path is expanded through
    /// `env`, and stays as it is when `env` does not know the name. A
    /// POSIX-style path is left as it is: a Windows-looking path pasted
    /// there is not guessed at.
    pub fn normalize_pasted(&self, text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
        let unquoted = text.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let text = match file_url_path(unquoted, self) {
            Some(path) => path,
            None if unquoted.starts_with("file://") => return text.to_string(),
            None if self.is_windows() => text.trim().to_string(),
            None => return text.to_string(),
        };
        if !self.is_windows() {
            return text;
        }
        let text = text
            .trim_matches(|c| c == '"' || c == '\'' || char::is_whitespace(c))
            .to_string();
        let text = expand_variables(&text, env);
        let mut out: String = text
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' {
                    self.display_separator
                } else {
                    c
                }
            })
            .collect();
        if is_drive_only(&out) {
            out.push(self.display_separator);
        }
        out
    }

    /// A fragment pasted into the middle of other path text: a Windows
    /// style writes its separators, and nothing else changes.
    pub(crate) fn normalize_fragment(&self, text: &str) -> String {
        if !self.is_windows() {
            return text.to_string();
        }
        text.chars()
            .map(|c| {
                if self.is_separator(c) {
                    self.display_separator
                } else {
                    c
                }
            })
            .collect()
    }

    /// Splits path text into its root and its parts. The text may mix
    /// separators in a Windows style. Empty parts, from a doubled or a
    /// trailing separator, are dropped.
    pub(crate) fn parse(&self, text: &str) -> Parsed {
        let sep = self.display_separator;
        if !self.is_windows() {
            let rooted = text.starts_with('/');
            return Parsed {
                prefix: if rooted { "/".into() } else { String::new() },
                parts: fold_dots(text.split('/').map(str::to_string).collect()),
                rooted,
                computer: false,
            };
        }
        let mut chars = text.chars();
        let first = chars.next();
        let second = chars.next();
        let split = |rest: &str| -> Vec<String> {
            fold_dots(
                rest.split(|c| self.is_separator(c))
                    .map(str::to_string)
                    .collect(),
            )
        };
        if let (Some(a), Some(':')) = (first, second)
            && a.is_ascii_alphabetic()
        {
            return Parsed {
                prefix: format!("{a}:{sep}"),
                parts: split(&text[2..]),
                rooted: true,
                computer: false,
            };
        }
        if first.is_some_and(|c| self.is_separator(c))
            && second.is_some_and(|c| self.is_separator(c))
        {
            let mut parts: Vec<String> = text
                .split(|c| self.is_separator(c))
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
            if parts.len() == 1 {
                return Parsed {
                    prefix: format!("{sep}{sep}{}{sep}", parts[0]),
                    parts: Vec::new(),
                    rooted: true,
                    computer: false,
                };
            }
            if parts.is_empty() {
                return Parsed {
                    prefix: format!("{sep}{sep}"),
                    parts,
                    rooted: true,
                    computer: false,
                };
            }
            if parts.len() >= 2 {
                let server = parts.remove(0);
                let share = parts.remove(0);
                return Parsed {
                    prefix: format!("{sep}{sep}{server}{sep}{share}{sep}"),
                    parts: fold_dots(parts),
                    rooted: true,
                    computer: false,
                };
            }
        }
        if first.is_some_and(|c| self.is_separator(c)) {
            let parts = split(text);
            if parts.is_empty() {
                return Parsed {
                    prefix: sep.to_string(),
                    parts,
                    rooted: true,
                    computer: self.computer_root,
                };
            }
            return Parsed {
                prefix: sep.to_string(),
                parts,
                rooted: true,
                computer: false,
            };
        }
        Parsed {
            prefix: String::new(),
            parts: split(text),
            rooted: false,
            computer: false,
        }
    }

    /// The text of parsed path in the style's display form, with no
    /// trailing separator except after a root.
    pub(crate) fn display(&self, parsed: &Parsed) -> String {
        let sep = self.display_separator.to_string();
        format!("{}{}", parsed.prefix, parsed.parts.join(&sep))
    }

    /// The canonical form of path text: separators unified, case folded
    /// when case does not matter, no trailing separator. Two texts are the
    /// same path exactly when their keys are equal.
    pub(crate) fn key(&self, text: &str) -> String {
        let parsed = self.parse(text);
        let mut key = parsed.prefix.replace('\\', "/");
        key.push_str(&parsed.parts.join("/"));
        if self.case_sensitive {
            key
        } else {
            key.to_lowercase()
        }
    }
}

impl Default for PathStyle {
    fn default() -> Self {
        Self::host()
    }
}

/// A path split into its root and parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// The root in display form: `/`, `C:\`, `\\server\share\`, or empty
    /// for a path with no root.
    pub(crate) prefix: String,
    pub(crate) parts: Vec<String>,
    pub(crate) rooted: bool,
    /// Whether this is the root above the drives.
    pub(crate) computer: bool,
}

impl Parsed {
    /// Whether there is nothing above this path.
    pub(crate) fn is_root(&self) -> bool {
        self.parts.is_empty()
    }
}

/// The parts of a path with the empty ones dropped, `.` dropped, and `..`
/// taking the part before it away. A `..` with nothing before it goes, so a
/// path never climbs above its root.
fn fold_dots(parts: Vec<String>) -> Vec<String> {
    let mut folded: Vec<String> = Vec::new();
    for part in parts {
        match part.as_str() {
            "" | "." => {}
            ".." => {
                folded.pop();
            }
            _ => folded.push(part),
        }
    }
    folded
}

fn is_drive_only(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(a), Some(':'), None) if a.is_ascii_alphabetic()
    )
}

/// `%NAME%` replaced with what `env` says, and left alone for a name it
/// does not know.
fn expand_variables(text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match env(&after[..end]) {
                Some(value) => {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The path a `file://` URL names in `style`, percent-decoded, or `None`
/// for text that is not one.
fn file_url_path(text: &str, style: &PathStyle) -> Option<String> {
    let rest = text.strip_prefix("file://")?;
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let path = percent_decode(path);
    if path.chars().any(char::is_control) {
        return None;
    }
    if style.is_windows() {
        let bytes = path.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':';
        if drive {
            return Some(path[1..].to_string());
        }
        if !host.is_empty() && host != "localhost" {
            return Some(format!("//{host}{path}"));
        }
    }
    Some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[at + 1]), hex(bytes[at + 2]))
        {
            out.push(high * 16 + low);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

/// A path in a source's own style: what a picker lists, what it asks a
/// source for, and what it reports as chosen.
///
/// The text is the path as the source writes it, with no trailing
/// separator except after a root. Two paths are equal when they name the
/// same place in their style, so `C:/Users/Me` equals `c:\users\me` in a
/// Windows style. A path made for the local disk converts with
/// [`to_path_buf`](Self::to_path_buf); a remote one is only text.
#[derive(Clone, Debug)]
pub struct SourcePath {
    text: SharedString,
    key: SharedString,
    style: PathStyle,
}

impl SourcePath {
    /// The path `text` names in `style`. Separators become the style's
    /// own, and a trailing separator goes unless it follows a root.
    pub fn new(text: &str, style: PathStyle) -> Self {
        let parsed = style.parse(text);
        Self {
            text: style.display(&parsed).into(),
            key: style.key(text).into(),
            style,
        }
    }

    /// The path as the source writes it.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The style of the path.
    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// The last part, or `None` for a root.
    pub fn file_name(&self) -> Option<&str> {
        let parsed = self.style.parse(&self.text);
        let last = parsed.parts.last()?;
        let at = self.text.len() - last.len();
        Some(&self.text[at..])
    }

    /// The path above this one, or `None` for a root. A Windows-style
    /// source with a computer root has it above a drive root.
    pub fn parent(&self) -> Option<SourcePath> {
        let mut parsed = self.style.parse(&self.text);
        if parsed.parts.is_empty() {
            if parsed.computer || !self.style.has_computer_root() {
                return None;
            }
            let is_drive = parsed.prefix.len() == 3 && parsed.prefix.as_bytes()[1] == b':';
            return is_drive.then(|| self.style.path(self.style.separator().to_string()));
        }
        parsed.parts.pop();
        Some(Self::new(&self.style.display(&parsed), self.style))
    }

    /// The path of `name` inside this one.
    pub fn join(&self, name: &str) -> SourcePath {
        let sep = self.style.separator();
        let mut text = self.text.to_string();
        if !text.ends_with(|c| self.style.is_separator(c)) {
            text.push(sep);
        }
        text.push_str(name);
        Self::new(&text, self.style)
    }

    /// Whether this is a root: `/`, a drive, a share, or the root above
    /// the drives.
    pub fn is_root(&self) -> bool {
        self.style.parse(&self.text).is_root()
    }

    /// The path as the local disk reads it. It is meaningful for a source
    /// that lists the local disk; for a remote source it is only the text.
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.text.as_ref() as &str)
    }
}

impl PartialEq for SourcePath {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for SourcePath {}

impl Hash for SourcePath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl fmt::Display for SourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Why a source could not list a directory. The picker shows the message
/// and, for a failure that waiting can fix, an action: Retry for a lost
/// connection or an error, and "Sign in" for a source that needs the user
/// to log in, which calls the application's own handler.
///
/// A source returns one from `list` for any failure. An `io::Error`
/// converts into the matching variant.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListError {
    /// The directory does not exist.
    NotFound,
    /// The user may not list the directory.
    PermissionDenied,
    /// The source needs the user to sign in. The text says why, and is
    /// shown to the user.
    AuthRequired(SharedString),
    /// The connection to the source was lost. The text says what happened,
    /// and is shown to the user.
    Disconnected(SharedString),
    /// Any other failure. The text is shown to the user.
    Other(SharedString),
}

impl ListError {
    /// The text the picker shows.
    pub fn message(&self) -> SharedString {
        match self {
            Self::NotFound => "That folder does not exist".into(),
            Self::PermissionDenied => "You cannot open that folder".into(),
            Self::AuthRequired(message) | Self::Disconnected(message) | Self::Other(message) => {
                message.clone()
            }
        }
    }

    /// Whether asking again can succeed, which gives the list a Retry
    /// action.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Disconnected(_) | Self::Other(_))
    }

    /// Whether the user must sign in, which gives the list a "Sign in"
    /// action.
    pub fn needs_sign_in(&self) -> bool {
        matches!(self, Self::AuthRequired(_))
    }
}

impl From<std::io::Error> for ListError {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        match error.kind() {
            ErrorKind::NotFound => Self::NotFound,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionRefused
            | ErrorKind::NotConnected
            | ErrorKind::BrokenPipe
            | ErrorKind::TimedOut => Self::Disconnected(error.to_string().into()),
            _ => Self::Other(error.to_string().into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn pasted(style: PathStyle, text: &str) -> String {
        style.normalize_pasted(text, &no_env)
    }

    #[test]
    fn a_windows_path_takes_either_separator_and_writes_its_own() {
        let windows = PathStyle::windows();
        for text in [
            "C:/Users/me",
            "C:\\Users\\me",
            "C:/Users\\me/",
            "C:\\Users/me\\",
        ] {
            assert_eq!(windows.path(text).as_str(), "C:\\Users\\me", "{text}");
        }
        let slashes = PathStyle::windows().with_display_separator('/');
        assert_eq!(slashes.path("C:\\Users\\me").as_str(), "C:/Users/me");
    }

    #[test]
    fn a_posix_path_keeps_a_backslash_as_a_name_character() {
        let posix = PathStyle::posix();
        assert_eq!(posix.path("/home/a\\b/").as_str(), "/home/a\\b");
        assert_eq!(posix.parse("/home/a\\b").parts, ["home", "a\\b"]);
        assert!(!posix.is_separator('\\'));
    }

    #[test]
    fn paths_that_name_one_place_are_equal_in_their_style() {
        let windows = PathStyle::windows();
        assert_eq!(windows.path("C:/Users/Me"), windows.path("c:\\users\\me\\"));
        assert_ne!(windows.path("C:/Users/Me"), windows.path("D:/Users/Me"));
        let posix = PathStyle::posix();
        assert_ne!(posix.path("/Users/Me"), posix.path("/users/me"));
        assert_eq!(
            PathStyle::posix().with_case_sensitive(false).path("/A"),
            PathStyle::posix().with_case_sensitive(false).path("/a")
        );
    }

    #[test]
    fn dots_fold_and_never_climb_above_the_root() {
        let posix = PathStyle::posix();
        assert_eq!(posix.path("/home/me/..").as_str(), "/home");
        assert_eq!(posix.path("/home/./me/../you").as_str(), "/home/you");
        assert_eq!(posix.path("/../..").as_str(), "/");
        assert_eq!(posix.path("/a/../../b").as_str(), "/b");
        let windows = PathStyle::windows();
        assert_eq!(windows.path("C:\\a\\..\\b").as_str(), "C:\\b");
        assert_eq!(windows.path("C:/a/../../..").as_str(), "C:\\");
        assert_eq!(
            windows.path("\\\\srv\\share\\..\\x").as_str(),
            "\\\\srv\\share\\x"
        );
        assert_eq!(posix.path("/home/me/..").join("..").as_str(), "/");
        assert_eq!(posix.path("/a").join("..").as_str(), "/");
    }

    #[test]
    fn a_partial_unc_path_is_a_root_not_a_rooted_name() {
        let windows = PathStyle::windows();
        let server = windows.path("\\\\srv\\");
        assert_eq!(server.as_str(), "\\\\srv\\");
        assert!(server.is_root());
        assert_eq!(server.parent(), None);
        assert_eq!(windows.path("\\\\").parent(), None);
        assert_eq!(windows.path("//srv").as_str(), "\\\\srv\\");
    }

    #[test]
    fn a_quoted_file_url_is_unquoted_first_and_a_control_character_refuses_it() {
        let posix = PathStyle::posix();
        let windows = PathStyle::windows();
        assert_eq!(pasted(posix, "\"file:///home/me\""), "/home/me");
        assert_eq!(pasted(windows, "'file:///C:/Users/me'"), "C:\\Users\\me");
        assert_eq!(pasted(posix, "file:///home/a%00b"), "file:///home/a%00b");
        assert_eq!(pasted(windows, "file:///C:/a%0Ab"), "file:///C:/a%0Ab");
    }

    #[test]
    fn roots_have_no_parent_and_parents_keep_the_root() {
        let windows = PathStyle::windows();
        assert_eq!(
            windows.path("C:\\Users\\me").parent().unwrap().as_str(),
            "C:\\Users"
        );
        assert_eq!(windows.path("C:\\Users").parent().unwrap().as_str(), "C:\\");
        assert_eq!(windows.path("C:\\").parent(), None);
        assert_eq!(
            windows
                .path("\\\\srv\\share\\dir")
                .parent()
                .unwrap()
                .as_str(),
            "\\\\srv\\share\\"
        );
        assert_eq!(
            windows.path("\\\\srv\\share").parent(),
            None,
            "a share root stays"
        );
        let posix = PathStyle::posix();
        assert_eq!(posix.path("/a/b").parent().unwrap().as_str(), "/a");
        assert_eq!(posix.path("/a").parent().unwrap().as_str(), "/");
        assert_eq!(posix.path("/").parent(), None);
    }

    #[test]
    fn a_computer_root_sits_above_the_drives_only_when_the_source_has_one() {
        let pc = PathStyle::windows().with_computer_root(true);
        let drives = pc.path("C:\\").parent().unwrap();
        assert_eq!(drives.as_str(), "\\");
        assert!(drives.is_root());
        assert_eq!(drives.parent(), None);
        assert_eq!(pc.path("\\\\srv\\share").parent(), None);
        assert_eq!(PathStyle::windows().path("C:\\").parent(), None);
    }

    #[test]
    fn join_and_file_name_follow_the_style() {
        let windows = PathStyle::windows();
        assert_eq!(windows.path("C:\\").join("Users").as_str(), "C:\\Users");
        assert_eq!(windows.path("C:\\Users").join("me").file_name(), Some("me"));
        assert_eq!(windows.path("C:\\").file_name(), None);
        assert_eq!(
            PathStyle::posix().path("/").join(".config").as_str(),
            "/.config"
        );
    }

    #[test]
    fn a_pasted_windows_path_is_cleaned_and_written_in_the_style() {
        let windows = PathStyle::windows();
        assert_eq!(pasted(windows, "\"C:\\Users\\me\""), "C:\\Users\\me");
        assert_eq!(pasted(windows, "  C:/Users/me  "), "C:\\Users\\me");
        assert_eq!(pasted(windows, "C:/Users\\me/docs"), "C:\\Users\\me\\docs");
        assert_eq!(pasted(windows, "C:"), "C:\\", "a lone drive is its root");
        assert_eq!(pasted(windows, "~/code"), "~\\code");
        assert_eq!(
            pasted(windows, "\\\\srv\\share\\dir"),
            "\\\\srv\\share\\dir"
        );
        assert_eq!(pasted(windows, "//srv/share/dir"), "\\\\srv\\share\\dir");
        assert_eq!(
            pasted(
                PathStyle::windows().with_display_separator('/'),
                "C:\\Users\\me"
            ),
            "C:/Users/me"
        );
    }

    #[test]
    fn variables_expand_only_through_the_sources_lookup() {
        let windows = PathStyle::windows();
        let env = |name: &str| (name == "USERPROFILE").then(|| "C:\\Users\\me".to_string());
        assert_eq!(
            windows.normalize_pasted("%USERPROFILE%\\code", &env),
            "C:\\Users\\me\\code"
        );
        assert_eq!(
            windows.normalize_pasted("%OTHER%\\code", &env),
            "%OTHER%\\code"
        );
        assert_eq!(
            pasted(windows, "%USERPROFILE%\\code"),
            "%USERPROFILE%\\code"
        );
    }

    #[test]
    fn a_file_url_becomes_a_path_in_the_style() {
        let windows = PathStyle::windows();
        assert_eq!(
            pasted(windows, "file:///C:/Users/me%20x"),
            "C:\\Users\\me x"
        );
        assert_eq!(
            pasted(windows, "file://srv/share/dir"),
            "\\\\srv\\share\\dir"
        );
        let posix = PathStyle::posix();
        assert_eq!(pasted(posix, "file:///home/me"), "/home/me");
        assert_eq!(pasted(posix, "file:///home/me%20x/a%2Fb"), "/home/me x/a/b");
    }

    #[test]
    fn a_pasted_path_in_a_posix_style_is_left_alone() {
        let posix = PathStyle::posix();
        assert_eq!(pasted(posix, "C:\\Users\\me"), "C:\\Users\\me");
        assert_eq!(pasted(posix, "\"/home/me\""), "\"/home/me\"");
        assert_eq!(pasted(posix, "/home/a\\b"), "/home/a\\b");
    }

    #[test]
    fn io_errors_map_to_the_matching_failure() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            ListError::from(Error::from(ErrorKind::NotFound)),
            ListError::NotFound
        );
        assert_eq!(
            ListError::from(Error::from(ErrorKind::PermissionDenied)),
            ListError::PermissionDenied
        );
        assert!(matches!(
            ListError::from(Error::from(ErrorKind::ConnectionReset)),
            ListError::Disconnected(_)
        ));
        assert!(ListError::from(Error::other("x")).is_retryable());
        assert!(!ListError::NotFound.is_retryable());
        assert!(ListError::AuthRequired("Sign in".into()).needs_sign_in());
    }

    #[test]
    fn names_sort_without_case_and_fold_by_the_styles_case_rule() {
        let windows = PathStyle::windows();
        let mut names = vec!["b", "A", "a", "B"];
        names.sort_by(|a, b| windows.compare_names(a, b));
        assert_eq!(names, ["A", "a", "B", "b"]);
        names.reverse();
        names.sort_by(|a, b| PathStyle::posix().compare_names(a, b));
        assert_eq!(names, ["A", "a", "B", "b"]);
        assert_eq!(windows.fold("ABC"), "abc");
        assert_eq!(PathStyle::posix().fold("ABC"), "ABC");
    }
}
