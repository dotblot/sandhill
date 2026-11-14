//! Making a folder from a picker: the typed error a source answers with, and
//! the one check every name passes before a source is asked.

use gpui_kit::SharedString;

use super::PathStyle;

/// The longest folder name the picker passes on, in characters.
const MAX_NAME: usize = 255;

/// Why a source could not make a folder. The picker shows the message under
/// the row that names the folder, and keeps the row editable.
///
/// A source returns one from `create_folder`. A name that is empty, has a
/// separator, or is `.` or `..` is refused by the picker before the source
/// is asked, with [`InvalidName`](Self::InvalidName).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreateFolderError {
    /// The source cannot make folders.
    Unsupported,
    /// A folder or file with that name is already there.
    Exists,
    /// The name cannot be a folder name. The text says why, and is shown
    /// to the user.
    InvalidName(SharedString),
    /// The user may not make folders there.
    PermissionDenied,
    /// The picker cannot make a folder yet: the directory is still
    /// loading, or another folder is being made.
    NotReady,
    /// Any other failure. The text is shown to the user.
    Other(SharedString),
}

impl CreateFolderError {
    /// The text the picker shows for the folder called `name`.
    pub fn message(&self, name: &str) -> SharedString {
        match self {
            Self::Unsupported => "This location cannot make folders".into(),
            Self::NotReady => "Wait for the folder to load".into(),
            Self::Exists => format!("A folder named {name} already exists").into(),
            Self::PermissionDenied => "You cannot make folders here".into(),
            Self::InvalidName(reason) | Self::Other(reason) => reason.clone(),
        }
    }
}

impl From<std::io::Error> for CreateFolderError {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        match error.kind() {
            ErrorKind::AlreadyExists => Self::Exists,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::InvalidInput | ErrorKind::InvalidFilename => {
                Self::InvalidName("That name is not allowed".into())
            }
            _ => Self::Other(error.to_string().into()),
        }
    }
}

/// Whether a listed name is one plain name: not empty, not `.` or `..`,
/// with no separator of the style and no control character. A listing that
/// names anything else is dropped, so a source cannot make a row that
/// climbs out of the folder or reaches into a deeper one.
pub(crate) fn is_plain_name(name: &str, style: &PathStyle) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| style.is_separator(c) || c.is_control())
}

/// The name a folder would get, or why it cannot: surrounding spaces go,
/// and what is left must be there, short, free of separators, nulls and
/// control characters, not `.` or `..`, and in a Windows style without the
/// characters Windows refuses.
pub(crate) fn validate_folder_name(
    name: &str,
    style: &PathStyle,
) -> Result<String, CreateFolderError> {
    let invalid = |reason: &str| Err(CreateFolderError::InvalidName(reason.to_string().into()));
    let name = name.trim();
    if name.is_empty() {
        return invalid("Enter a name for the folder");
    }
    if name == "." || name == ".." {
        return invalid("A folder cannot be named . or ..");
    }
    if name.chars().count() > MAX_NAME {
        return invalid("That name is too long");
    }
    if name.contains(['/', '\\']) {
        return invalid("A folder name cannot contain / or \\");
    }
    if name.chars().any(char::is_control) {
        return invalid("A folder name cannot contain control characters");
    }
    if style.is_windows() && name.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return invalid("A folder name cannot contain < > : \" | ? *");
    }
    if style.is_windows() {
        if name.ends_with('.') {
            return invalid("A folder name cannot end with a dot");
        }
        if is_reserved_windows_name(name) {
            return invalid("That name is reserved by Windows");
        }
    }
    Ok(name.to_string())
}

/// Whether Windows reserves `name`: a device name, with or without an
/// extension, in any case.
fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|digit| {
                digit.len() == 1 && digit != "0" && digit.as_bytes()[0].is_ascii_digit()
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_name_is_one_plain_name() {
        let posix = PathStyle::posix();
        let windows = PathStyle::windows();
        for name in ["", ".", "..", "a/b", "../x", "a\0b", "tab\t"] {
            assert!(!is_plain_name(name, &posix), "{name:?}");
        }
        assert!(
            is_plain_name("a\\b", &posix),
            "a backslash is a POSIX name character"
        );
        assert!(!is_plain_name("a\\b", &windows));
        assert!(is_plain_name("...", &posix));
        assert!(is_plain_name(".config", &posix));
    }

    #[test]
    fn a_good_name_is_trimmed() {
        let posix = PathStyle::posix();
        assert_eq!(
            validate_folder_name("  src  ", &posix).as_deref(),
            Ok("src")
        );
        assert_eq!(
            validate_folder_name(".config", &posix).as_deref(),
            Ok(".config")
        );
        assert_eq!(
            validate_folder_name("a b.c", &posix).as_deref(),
            Ok("a b.c")
        );
        assert_eq!(validate_folder_name("a:b", &posix).as_deref(), Ok("a:b"));
    }

    #[test]
    fn a_bad_name_says_why() {
        let posix = PathStyle::posix();
        for name in ["", "   ", ".", "..", "a/b", "a\\b", "a\0b", "a\nb"] {
            assert!(
                matches!(
                    validate_folder_name(name, &posix),
                    Err(CreateFolderError::InvalidName(_))
                ),
                "{name:?}"
            );
        }
        let long = "x".repeat(256);
        assert!(validate_folder_name(&long, &posix).is_err());
        assert!(validate_folder_name(&"x".repeat(255), &posix).is_ok());
    }

    #[test]
    fn windows_refuses_what_windows_refuses() {
        let windows = PathStyle::windows();
        for name in ["a:b", "a*b", "a?b", "a|b", "a<b", "a\"b"] {
            assert!(validate_folder_name(name, &windows).is_err(), "{name}");
        }
        assert!(validate_folder_name("My Files", &windows).is_ok());
        for name in [
            "CON", "con", "Nul.txt", "COM1", "lpt9.log", "AUX.", "name.", "a..", "PRN.x.y",
        ] {
            assert!(validate_folder_name(name, &windows).is_err(), "{name}");
        }
        for name in ["COM0", "COM10", "CONSOLE", "com", "LPT", "a.con"] {
            assert!(validate_folder_name(name, &windows).is_ok(), "{name}");
        }
        assert!(validate_folder_name("CON", &PathStyle::posix()).is_ok());
        assert!(validate_folder_name("name.", &PathStyle::posix()).is_ok());
    }

    #[test]
    fn the_message_names_the_folder() {
        assert_eq!(
            CreateFolderError::Exists.message("docs"),
            "A folder named docs already exists"
        );
        assert_eq!(
            CreateFolderError::Other("Disk full".into()).message("x"),
            "Disk full"
        );
    }

    #[test]
    fn io_errors_map_to_the_matching_failure() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            CreateFolderError::from(Error::from(ErrorKind::AlreadyExists)),
            CreateFolderError::Exists
        );
        assert_eq!(
            CreateFolderError::from(Error::from(ErrorKind::PermissionDenied)),
            CreateFolderError::PermissionDenied
        );
    }
}
