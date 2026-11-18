//! Shortcut labels. Sandhill writes them the Mac's way (⇧⌘K); elsewhere
//! they read as that system writes them (Ctrl+Shift+K), matching the
//! bindings in `main.rs`, where ⌘ becomes Ctrl.

use gpui_kit::SharedString;

/// `text` with every Mac shortcut in it written for this system: unchanged
/// on a Mac.
pub fn keys(text: impl Into<SharedString>) -> SharedString {
    let text = text.into();
    if cfg!(target_os = "macos") {
        text
    } else {
        written_elsewhere(&text).into()
    }
}

/// ⌘ is Ctrl, ⌥ Alt and ⇧ Shift, in the order Ctrl, Alt, Shift. ⌃ is Ctrl
/// too, except beside ⌘, where it is Alt (⌃⌘↩ is bound to Ctrl+Alt+Enter,
/// as Ctrl+Enter runs the line). A modifier with no key after it (“Hold ⇧”)
/// is just its name.
fn written_elsewhere(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if !is_modifier(c) {
            out.push(c);
            chars.next();
            continue;
        }
        let (mut command, mut control, mut alt, mut shift) = (false, false, false, false);
        while let Some(&m) = chars.peek().filter(|m| is_modifier(**m)) {
            match m {
                '⌘' => command = true,
                '⌃' => control = true,
                '⌥' => alt = true,
                _ => shift = true,
            }
            chars.next();
        }
        let ctrl = command || control;
        let alt = alt || (command && control);
        let names = [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift")];
        let mut first = true;
        for (_, name) in names.iter().filter(|(on, _)| *on) {
            if !first {
                out.push('+');
            }
            out.push_str(name);
            first = false;
        }
        // The key: a word (Tab, a letter) or one symbol.
        match chars.peek().copied() {
            Some(k) if k.is_ascii_alphabetic() => {
                let mut word = String::new();
                while let Some(&k) = chars.peek().filter(|k| k.is_ascii_alphanumeric()) {
                    word.push(k);
                    chars.next();
                }
                if word.len() == 1 {
                    word.make_ascii_uppercase();
                }
                out.push('+');
                out.push_str(&word);
            }
            Some(k) if !k.is_whitespace() && k != ')' => {
                out.push('+');
                out.push_str(key_name(k));
                if key_name(k).is_empty() {
                    out.push(k);
                }
                chars.next();
            }
            _ => {}
        }
    }
    out
}

fn is_modifier(c: char) -> bool {
    matches!(c, '⌘' | '⌃' | '⌥' | '⇧')
}

/// Names for the Mac's key symbols; empty for keys written as themselves.
fn key_name(k: char) -> &'static str {
    match k {
        '↩' | '↵' => "Enter",
        '⌫' => "Backspace",
        '⌦' => "Delete",
        '⎋' => "Esc",
        '⇥' => "Tab",
        '↑' => "Up",
        '↓' => "Down",
        '←' => "Left",
        '→' => "Right",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::written_elsewhere;

    #[test]
    fn mac_shortcuts_read_as_ctrl_alt_shift() {
        for (mac, other) in [
            ("⌘K", "Ctrl+K"),
            ("⇧⌘K", "Ctrl+Shift+K"),
            ("⌥⌘N", "Ctrl+Alt+N"),
            ("⌘.", "Ctrl+."),
            ("⌘,", "Ctrl+,"),
            ("⌘⌫", "Ctrl+Backspace"),
            ("⌃⌘↩", "Ctrl+Alt+Enter"),
            ("⌥3", "Alt+3"),
            (
                "Go to a file or run a command ⌘K",
                "Go to a file or run a command Ctrl+K",
            ),
            ("Previous match (⇧⌘G)", "Previous match (Ctrl+Shift+G)"),
            ("Enclosing folder  ⌘↑", "Enclosing folder  Ctrl+Up"),
            (
                "Hold ⇧ to add it to the current sort",
                "Hold Shift to add it to the current sort",
            ),
            (
                "Its terminal shows the modes it offers: ⇧Tab switches.",
                "Its terminal shows the modes it offers: Shift+Tab switches.",
            ),
            ("", ""),
        ] {
            assert_eq!(written_elsewhere(mac), other, "{mac}");
        }
    }
}
