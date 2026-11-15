//! The rows a command palette is given: items, groups of them under a
//! heading, and separators.

use std::rc::Rc;

use gpui_kit::{Action, AnyElement, App, FocusHandle, IntoElement, SharedString, Window};

use crate::Icon;

/// The state of a row a custom renderer draws; see
/// [`CommandItem::render`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommandRow {
    pub(crate) highlighted: bool,
    pub(crate) disabled: bool,
}

impl CommandRow {
    /// Whether the keyboard or the pointer is on the row.
    pub fn is_highlighted(&self) -> bool {
        self.highlighted
    }

    /// Whether the row's command is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

pub(crate) type RowRenderer = Rc<dyn Fn(CommandRow, &mut Window, &mut App) -> AnyElement>;

/// One command: a key the palette reports it by, a label, and what it does.
#[non_exhaustive]
pub struct CommandItem {
    key: SharedString,
    label: SharedString,
    /// Boxed: an [`Icon`] carries a whole style, which would make every
    /// row kilobytes wide.
    icon: Option<Box<Icon>>,
    /// Words the query is looked for in besides the label.
    keywords: Vec<SharedString>,
    action: Option<Box<dyn Action>>,
    action_context: Option<FocusHandle>,
    disabled: bool,
    render: Option<RowRenderer>,
}

impl Clone for CommandItem {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            label: self.label.clone(),
            icon: self.icon.clone(),
            keywords: self.keywords.clone(),
            action: self.action.as_ref().map(|action| action.boxed_clone()),
            action_context: self.action_context.clone(),
            disabled: self.disabled,
            render: self.render.clone(),
        }
    }
}

impl CommandItem {
    /// A command with a key that is stable across renders and a label.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            key: key.into(),
            keywords: Vec::new(),
            label,
            icon: None,
            action: None,
            action_context: None,
            disabled: false,
            render: None,
        }
    }

    /// The icon before the label.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(Box::new(icon.into()));
        self
    }

    /// Words the query matches besides the label, such as "shell" for a
    /// terminal.
    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        for keyword in keywords {
            self.keywords.push(keyword.into());
        }
        self
    }

    /// Dispatches `action` when the command is chosen, and shows the
    /// action's shortcut at the end of the row.
    pub fn action(mut self, action: impl Action) -> Self {
        self.action = Some(Box::new(action));
        self
    }

    /// The element the action goes to, and whose key bindings its shortcut
    /// shows. Without one it goes up from the palette's search field.
    pub fn action_context(mut self, focus_handle: &FocusHandle) -> Self {
        self.action_context = Some(focus_handle.clone());
        self
    }

    /// Whether the command can be chosen. A disabled row shows muted and
    /// the keyboard passes over it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Draws the inside of the row in place of the icon, the label, and
    /// the shortcut. The palette keeps the row's frame: its padding, its
    /// highlight, and the pointer and the keyboard. The label is still
    /// what the query matches and what assistive technology reads. The
    /// renderer runs each time the row is drawn, so it must not change
    /// anything.
    pub fn render<E: IntoElement>(
        mut self,
        render: impl Fn(CommandRow, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render = Some(Rc::new(move |row, window, cx| {
            render(row, window, cx).into_any_element()
        }));
        self
    }

    /// The key the palette reports this command by.
    pub fn key(&self) -> &SharedString {
        &self.key
    }

    /// The text of the row.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Whether the command is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Whether the query matches the label or a keyword, as a fuzzy
    /// subsequence ignoring case. An empty query matches everything.
    pub(crate) fn matches(&self, query: &str) -> bool {
        self.match_score(query).is_some()
    }

    /// How well the query matches: the label's score, or half the best
    /// keyword's, which is a weaker hit. `None` for no match.
    pub(crate) fn match_score(&self, query: &str) -> Option<u32> {
        let label = crate::fuzzy::score_of(query, &self.label);
        let keyword = self
            .keywords
            .iter()
            .filter_map(|keyword| crate::fuzzy::score_of(query, keyword))
            .max()
            .map(|score| score / 2);
        label.max(keyword)
    }

    pub(crate) fn leading_icon(&self) -> Option<&Icon> {
        self.icon.as_deref()
    }

    pub(crate) fn dispatched_action(&self) -> Option<&dyn Action> {
        self.action.as_deref()
    }

    pub(crate) fn own_action_context(&self) -> Option<&FocusHandle> {
        self.action_context.as_ref()
    }

    pub(crate) fn renderer(&self) -> Option<&RowRenderer> {
        self.render.as_ref()
    }
}

/// Commands under a heading. A group with no command left after the query
/// hides with its heading.
#[derive(Clone)]
#[non_exhaustive]
pub struct CommandGroup {
    heading: Option<SharedString>,
    items: Vec<CommandItem>,
}

impl CommandGroup {
    /// A group with no heading.
    pub fn new() -> Self {
        Self {
            heading: None,
            items: Vec::new(),
        }
    }

    /// The muted heading over the group's commands.
    pub fn heading(mut self, heading: impl Into<SharedString>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    /// Adds commands to the group.
    pub fn items(mut self, items: impl IntoIterator<Item = CommandItem>) -> Self {
        self.items.extend(items);
        self
    }
}

impl Default for CommandGroup {
    fn default() -> Self {
        Self::new()
    }
}

/// One entry of a palette.
#[derive(Clone)]
pub enum CommandEntry {
    /// A command outside any group.
    Item(CommandItem),
    /// Commands under a heading.
    Group(CommandGroup),
    /// A line between entries. One at either end, or next to another,
    /// once the query has filtered the rows, is not drawn.
    Separator,
}

impl From<CommandItem> for CommandEntry {
    fn from(item: CommandItem) -> Self {
        Self::Item(item)
    }
}

impl From<CommandGroup> for CommandEntry {
    fn from(group: CommandGroup) -> Self {
        Self::Group(group)
    }
}

/// A row as the palette draws it, once the query has filtered the
/// entries.
#[derive(Clone)]
pub(crate) enum Row {
    Heading(SharedString),
    Separator,
    Item(CommandItem),
}

/// The rows of `entries` that match `query`: groups with
/// no match drop with their heading, and separators keep only between
/// two runs of rows.
pub(crate) fn rows(entries: &[CommandEntry], query: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut pending_separator = false;
    let push = |block: Vec<Row>, rows: &mut Vec<Row>, separator: &mut bool| {
        if block.is_empty() {
            return;
        }
        if *separator && !rows.is_empty() {
            rows.push(Row::Separator);
        }
        *separator = false;
        rows.extend(block);
    };
    for entry in entries {
        match entry {
            CommandEntry::Separator => pending_separator = true,
            CommandEntry::Item(item) => {
                let block = if item.matches(query) {
                    vec![Row::Item(item.clone())]
                } else {
                    Vec::new()
                };
                push(block, &mut rows, &mut pending_separator);
            }
            CommandEntry::Group(group) => {
                let mut hits: Vec<(&CommandItem, u32)> = group
                    .items
                    .iter()
                    .filter_map(|item| Some((item, item.match_score(query)?)))
                    .collect();
                hits.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
                let items: Vec<Row> = hits
                    .into_iter()
                    .map(|(item, _)| Row::Item(item.clone()))
                    .collect();
                let block = if items.is_empty() {
                    items
                } else {
                    group
                        .heading
                        .clone()
                        .map(Row::Heading)
                        .into_iter()
                        .chain(items)
                        .collect()
                };
                push(block, &mut rows, &mut pending_separator);
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Heading(text) => format!("# {text}"),
                Row::Separator => "--".into(),
                Row::Item(item) => item.key().to_string(),
            })
            .collect()
    }

    #[test]
    fn a_query_ranks_the_commands_of_a_group_best_first() {
        let entries = vec![
            CommandGroup::new()
                .heading("Language")
                .items([
                    CommandItem::new("de", "Deutsch").keywords(["German"]),
                    CommandItem::new("en", "English"),
                ])
                .into(),
        ];
        assert_eq!(shape(&rows(&entries, "")), ["# Language", "de", "en"]);
        assert_eq!(shape(&rows(&entries, "en")), ["# Language", "en", "de"]);
    }

    #[test]
    fn a_query_matches_labels_and_keywords_and_drops_empty_groups() {
        let entries = vec![
            CommandItem::new("project", "Project").into(),
            CommandEntry::Separator,
            CommandGroup::new()
                .heading("Open")
                .items([
                    CommandItem::new("terminal", "Terminal").keywords(["Shell"]),
                    CommandItem::new("browser", "Browser"),
                ])
                .into(),
            CommandEntry::Separator,
            CommandItem::new("desktop", "Desktop").into(),
        ];
        assert_eq!(
            shape(&rows(&entries, "")),
            [
                "project", "--", "# Open", "terminal", "browser", "--", "desktop"
            ]
        );
        assert_eq!(shape(&rows(&entries, "shell")), ["# Open", "terminal"]);
        assert_eq!(
            shape(&rows(&entries, "o")),
            ["project", "--", "# Open", "browser", "--", "desktop"]
        );
        assert!(rows(&entries, "zzz").is_empty());
    }
}
