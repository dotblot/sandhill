//! The rows of a select menu: items, group labels, and separators, and
//! the filter a search query applies to them.

use std::sync::Arc;

use gpui_kit::{AnyElement, App, IntoElement as _, SharedString, Window};

use crate::Icon;

/// Shared and thread-safe, so a search can build rows on a background
/// thread and send them back.
type LeadingBuilder = Arc<dyn Fn(&mut Window, &mut App) -> AnyElement + Send + Sync>;

/// What goes before an item's label.
#[derive(Clone)]
enum Leading {
    /// An icon, in the row's text color. Boxed: an icon carries a whole
    /// style refinement, and a row is cloned as it renders.
    Icon(Box<Icon>),
    /// Any element, built each frame.
    Element(LeadingBuilder),
}

/// The value of a [`SelectItem`]: the item's identity in the menu.
///
/// The key names the row's element id, so it is stable across frames and
/// unique in the menu. Strings and integers are values already; an
/// application's own enum or record implements this with its id.
///
/// ```
/// use gpui_cn::SelectValue;
/// use gpui_kit::SharedString;
///
/// #[derive(Clone, PartialEq)]
/// enum Effort {
///     Low,
///     High,
/// }
///
/// impl SelectValue for Effort {
///     fn key(&self) -> SharedString {
///         match self {
///             Self::Low => "low".into(),
///             Self::High => "high".into(),
///         }
///     }
/// }
/// ```
pub trait SelectValue: Clone + PartialEq + 'static {
    /// The stable name of the value's row.
    fn key(&self) -> SharedString;
}

impl SelectValue for SharedString {
    fn key(&self) -> SharedString {
        self.clone()
    }
}

impl SelectValue for String {
    fn key(&self) -> SharedString {
        self.clone().into()
    }
}

impl SelectValue for &'static str {
    fn key(&self) -> SharedString {
        (*self).into()
    }
}

macro_rules! integer_values {
    ($($int:ty),*) => {
        $(
            impl SelectValue for $int {
                fn key(&self) -> SharedString {
                    self.to_string().into()
                }
            }
        )*
    };
}

integer_values!(usize, u32, u64, i32, i64);

/// One choice in a [`Select`](crate::Select) menu.
///
/// The value is the item's identity: it keys the row's element id, the
/// selection, and the events. The label is what the row and the trigger
/// show. An item can carry a description under its label, an icon or any
/// element before it, extra words the search matches, and a disabled
/// state.
///
/// ```
/// use gpui_cn::{SelectItem, gpui_kit::assets::IconName};
///
/// let _ = SelectItem::new("finder", "Finder").icon(IconName::Folder);
/// let _ = SelectItem::new("never", "Never ask for approval")
///     .description("Blocked actions fail instead of requesting approval");
/// let _ = SelectItem::new("de", "Deutsch").keywords(["german"]);
/// let _ = SelectItem::new(42, "Item 42");
/// ```
#[derive(Clone)]
pub struct SelectItem<V> {
    value: V,
    label: SharedString,
    description: Option<SharedString>,
    leading: Option<Leading>,
    /// Words the query is looked for in besides the label.
    keywords: Vec<SharedString>,
    disabled: bool,
}

impl<V: SelectValue> std::fmt::Debug for SelectItem<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectItem")
            .field("key", &self.value.key())
            .field("label", &self.label)
            .field("description", &self.description)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl<V: SelectValue> SelectItem<V> {
    /// An item with a stable value and the label it shows.
    pub fn new(value: V, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            value,
            keywords: Vec::new(),
            label,
            description: None,
            leading: None,
            disabled: false,
        }
    }

    /// A second line under the label, in the muted color.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// An icon before the label, at the icon size, in the row's text
    /// color. The trigger shows the selected item's icon too.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.leading = Some(Leading::Icon(Box::new(icon.into())));
        self
    }

    /// Any element before the label, built each frame: a color swatch, an
    /// avatar, an application icon. It replaces the icon in the row; the
    /// trigger shows it too. The builder is `Send + Sync` so rows can come
    /// from a search on another thread.
    pub fn leading<E: gpui_kit::IntoElement>(
        mut self,
        build: impl Fn(&mut Window, &mut App) -> E + Send + Sync + 'static,
    ) -> Self {
        self.leading = Some(Leading::Element(Arc::new(move |window, cx| {
            build(window, cx).into_any_element()
        })));
        self
    }

    /// Words the search matches besides the label, such as a language's
    /// English name beside its own.
    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        for keyword in keywords {
            self.keywords.push(keyword.into());
        }
        self
    }

    /// Whether the item can be chosen. A disabled item shows in the muted
    /// color and the keyboard skips it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The item's identity.
    pub fn value(&self) -> &V {
        &self.value
    }

    /// The stable name of the item's row: its value's key.
    pub fn key(&self) -> SharedString {
        self.value.key()
    }

    /// The text the row and the trigger show.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The line under the label, if any.
    pub fn description_text(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    /// The icon before the label, if the leading element is one.
    pub fn icon_source(&self) -> Option<&Icon> {
        match &self.leading {
            Some(Leading::Icon(icon)) => Some(icon),
            _ => None,
        }
    }

    /// Whether the item can be chosen.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The element before the label: the custom one, else the icon.
    pub(crate) fn render_leading(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        match self.leading.as_ref()? {
            Leading::Icon(icon) => Some((**icon).clone().into_any_element()),
            Leading::Element(build) => Some(build(window, cx)),
        }
    }

    /// Whether the query matches the label or a keyword, as a fuzzy
    /// subsequence ignoring case. An empty query matches everything.
    pub fn matches(&self, query: &str) -> bool {
        self.match_score(query).is_some()
    }

    /// How well the query matches: the label's score, or half the best
    /// keyword's score, since a keyword is a weaker hit. `None` for no
    /// match, and 0 for an empty query.
    pub fn match_score(&self, query: &str) -> Option<u32> {
        let label = crate::fuzzy::score_of(query, &self.label);
        let keyword = self
            .keywords
            .iter()
            .filter_map(|keyword| crate::fuzzy::score_of(query, keyword))
            .max()
            .map(|score| score / 2);
        label.max(keyword)
    }
}

/// A row of a select menu.
#[derive(Clone, Debug)]
pub enum SelectEntry<V: SelectValue> {
    /// A choice.
    Item(SelectItem<V>),
    /// A heading over the items that follow it, in the muted color.
    Label(SharedString),
    /// A hairline between two groups.
    Separator,
}

impl<V: SelectValue> SelectEntry<V> {
    /// A heading over the items that follow it.
    pub fn label(text: impl Into<SharedString>) -> Self {
        Self::Label(text.into())
    }

    /// The item, if this row is one.
    pub fn item(&self) -> Option<&SelectItem<V>> {
        match self {
            Self::Item(item) => Some(item),
            _ => None,
        }
    }
}

impl<V: SelectValue> From<SelectItem<V>> for SelectEntry<V> {
    fn from(item: SelectItem<V>) -> Self {
        Self::Item(item)
    }
}

/// The rows that show for a query: every item that matches, each label
/// that still heads an item, and each separator that still sits between
/// two rows. Returns indices into `entries`.
pub(crate) fn visible_entries<V: SelectValue>(
    entries: &[SelectEntry<V>],
    query: &str,
) -> Vec<usize> {
    let query = query.trim();
    let mut visible = Vec::with_capacity(entries.len());
    let mut pending_label: Option<usize> = None;
    let mut pending_separator: Option<usize> = None;
    let mut group: Vec<(usize, u32)> = Vec::new();
    // A group's matches go in best first, ties in the order given, under
    // its label, with a separator only between two groups that show.
    let flush = |group: &mut Vec<(usize, u32)>,
                 visible: &mut Vec<usize>,
                 pending_separator: &mut Option<usize>,
                 pending_label: &mut Option<usize>| {
        if group.is_empty() {
            return;
        }
        if let Some(separator) = pending_separator.take()
            && !visible.is_empty()
        {
            visible.push(separator);
        }
        if let Some(label) = pending_label.take() {
            visible.push(label);
        }
        group.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        visible.extend(group.drain(..).map(|(index, _)| index));
    };
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            SelectEntry::Item(item) => {
                if let Some(score) = item.match_score(query) {
                    group.push((index, score));
                }
            }
            SelectEntry::Label(_) => {
                flush(
                    &mut group,
                    &mut visible,
                    &mut pending_separator,
                    &mut pending_label,
                );
                pending_label = Some(index);
            }
            SelectEntry::Separator => {
                flush(
                    &mut group,
                    &mut visible,
                    &mut pending_separator,
                    &mut pending_label,
                );
                pending_label = None;
                pending_separator = Some(index);
            }
        }
    }
    flush(
        &mut group,
        &mut visible,
        &mut pending_separator,
        &mut pending_label,
    );
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<SelectEntry<&'static str>> {
        vec![
            SelectEntry::label("Type"),
            SelectItem::new("all", "All chats").into(),
            SelectItem::new("local", "Local").into(),
            SelectEntry::Separator,
            SelectEntry::label("Sort by"),
            SelectItem::new("updated", "Updated").into(),
            SelectItem::new("created", "Created")
                .keywords(["time"])
                .into(),
        ]
    }

    #[test]
    fn matching_is_fuzzy_ignores_case_and_reads_keywords() {
        let item = SelectItem::new("de", "Deutsch").keywords(["German"]);
        assert!(item.matches(""));
        assert!(item.matches("  "));
        assert!(item.matches("deu"));
        assert!(item.matches("GERM"));
        assert!(item.matches("dtsh"));
        assert!(item.matches("grmn"));
        assert!(!item.matches("french"));
    }

    #[test]
    fn a_query_ranks_the_rows_of_a_group_best_first() {
        let entries: Vec<SelectEntry<&'static str>> = vec![
            SelectEntry::label("Language"),
            SelectItem::new("de", "Deutsch").keywords(["German"]).into(),
            SelectItem::new("fr", "French").into(),
            SelectItem::new("en", "English").into(),
            SelectEntry::Separator,
            SelectEntry::label("Other"),
            SelectItem::new("x", "Zen").into(),
            SelectItem::new("y", "Enigma").into(),
        ];
        let labels = |query: &str| -> Vec<String> {
            visible_entries(&entries, query)
                .into_iter()
                .map(|index| match &entries[index] {
                    SelectEntry::Item(item) => item.label().to_string(),
                    SelectEntry::Label(text) => format!("# {text}"),
                    SelectEntry::Separator => "--".to_string(),
                })
                .collect()
        };
        assert_eq!(
            labels("en"),
            [
                "# Language",
                "English",
                "French",
                "Deutsch",
                "--",
                "# Other",
                "Enigma",
                "Zen"
            ],
            "label hits beat the keyword hit, groups stay"
        );
        assert_eq!(
            labels("").len(),
            entries.len(),
            "an empty query keeps the order"
        );
        assert_eq!(
            visible_entries(&entries, ""),
            (0..entries.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_empty_query_shows_every_row() {
        assert_eq!(visible_entries(&entries(), ""), vec![0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn a_query_keeps_only_the_labels_and_separators_that_still_frame_an_item() {
        let entries = entries();
        assert_eq!(visible_entries(&entries, "local"), vec![0, 2]);
        assert_eq!(visible_entries(&entries, "time"), vec![4, 6]);
        assert_eq!(visible_entries(&entries, "ated"), vec![4, 5, 6]);
        assert_eq!(
            visible_entries(&entries, "l"),
            vec![0, 2, 1],
            "Local starts with the letter, so it ranks above All chats"
        );
        assert_eq!(
            visible_entries(&entries, " LOC "),
            vec![0, 2],
            "case and space"
        );
        assert_eq!(visible_entries(&entries, "zzz"), Vec::<usize>::new());
    }

    #[test]
    fn a_separator_never_leads_or_trails() {
        let entries: Vec<SelectEntry<&'static str>> = vec![
            SelectEntry::Separator,
            SelectItem::new("a", "A").into(),
            SelectEntry::Separator,
            SelectEntry::Separator,
            SelectItem::new("b", "B").into(),
            SelectEntry::Separator,
        ];
        assert_eq!(visible_entries(&entries, ""), vec![1, 3, 4]);
    }

    #[test]
    fn rows_can_cross_threads() {
        fn assert_send<T: Send>(_: &T) {}
        let entries: Vec<SelectEntry<usize>> = vec![
            SelectItem::new(1, "One")
                .leading(|_, _| gpui_kit::div())
                .into(),
            SelectEntry::Separator,
        ];
        assert_send(&entries);
    }

    #[test]
    fn readers_read_back() {
        let item = SelectItem::new("v", "Label")
            .description("More")
            .disabled(true);
        assert_eq!(*item.value(), "v");
        assert_eq!(item.key(), "v");
        assert_eq!(item.label(), "Label");
        assert_eq!(item.description_text().map(|d| d.as_ref()), Some("More"));
        assert!(item.is_disabled());
        assert!(item.icon_source().is_none());
        assert!(SelectEntry::from(item.clone()).item().is_some());
        assert!(SelectEntry::<&str>::Separator.item().is_none());
        assert!(format!("{item:?}").contains("Label"));
        assert_eq!(7usize.key(), "7");
        assert_eq!(String::from("s").key(), "s");
    }
}
