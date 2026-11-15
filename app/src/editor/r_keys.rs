//! R-only insertion shortcuts; non-R inputs keep their native key handling.
use crate::workbench::Workbench;
use gpui_kit::component::input::EditorState;
use gpui_kit::*;

pub(crate) fn insertion(text: &str, cursor: usize, assignment: bool) -> String {
    let leading = cursor > 0
        && !text[..cursor]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
    let operator = if assignment {
        "<-"
    } else if crate::editor::statements::ggplot_chain(text, cursor) {
        "+"
    } else {
        "|>"
    };
    format!("{}{operator} ", if leading { " " } else { "" })
}

impl Workbench {
    pub(crate) fn r_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = &event.keystroke;
        let assignment = key.modifiers.alt
            && !key.modifiers.platform
            && !key.modifiers.control
            && !key.modifiers.shift
            && matches!(key.key.as_str(), "-" | "–");
        let pipe = key.modifiers.platform
            && key.modifiers.shift
            && !key.modifiers.alt
            && !key.modifiers.control
            && key.key.eq_ignore_ascii_case("m");
        if !assignment && !pipe {
            return false;
        }
        let editor: Option<Entity<EditorState>> = self
            .open
            .as_ref()
            .filter(|_| !self.objects.is_shown())
            .and_then(|open| {
                open.editors
                    .iter()
                    .enumerate()
                    .find(|(ix, editor)| {
                        open.notebook
                            .cell_language(*ix)
                            .is_some_and(|language| language.eq_ignore_ascii_case("r"))
                            && editor.read(cx).focus_handle(cx).is_focused(window)
                    })
                    .map(|(_, editor)| editor.clone())
            })
            .or_else(|| {
                let input = self.console_input.read(cx);
                (input.language_name().eq_ignore_ascii_case("r")
                    && input.focus_handle(cx).is_focused(window))
                .then(|| self.console_input.clone())
            });
        let Some(editor) = editor else {
            return false;
        };
        editor.update(cx, |editor, cx| {
            let text = editor.value();
            let value = insertion(&text, editor.selected_range().start, assignment);
            editor.replace(value, window, cx);
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::insertion;
    #[test]
    fn assignment_respects_existing_whitespace() {
        assert_eq!(insertion("x", 1, true), " <- ");
        assert_eq!(insertion("x ", 2, true), "<- ");
        assert_eq!(insertion("", 0, true), "<- ");
        assert_eq!(insertion("\n", 1, true), "<- ");
    }
    #[test]
    fn ggplot_chain_is_statement_local() {
        for source in [
            "ggplot(df)",
            "ggplot(df) +\n  geom_point()",
            "plot +\n  layer",
        ] {
            assert_eq!(insertion(source, source.len(), false), " + ", "{source}");
        }
        for source in [
            "ggplot(df)\nx",
            "print('ggplot(')",
            "x # ggplot(",
            "data |>\n  transform()",
        ] {
            assert_eq!(insertion(source, source.len(), false), " |> ", "{source}");
        }
    }
}
