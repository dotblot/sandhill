//! The rows of a menu: items, submenus, group labels, and separators.

use std::{collections::HashSet, rc::Rc};

use gpui_kit::{
    Action, AnyElement, App, FocusHandle, IntoElement, OwnedMenuItem, SharedString, Window,
    base::Disableable,
};

use crate::Icon;

type SelectHandler = Rc<dyn Fn(&mut Window, &mut App)>;
pub(crate) type RowRenderer = Rc<dyn Fn(MenuRowState, &mut Window, &mut App) -> AnyElement>;

/// What an item does when it is chosen.
enum Activation {
    /// Nothing beyond the state's `Activated` event.
    Event,
    /// Dispatches an action to the action context, and shows the
    /// action's shortcut.
    Action(Box<dyn Action>),
    /// Calls the application.
    Handler(SelectHandler),
    /// Opens a URL in the system's browser or mail client.
    Link(SharedString),
}

impl Clone for Activation {
    fn clone(&self) -> Self {
        match self {
            Self::Event => Self::Event,
            Self::Action(action) => Self::Action(action.boxed_clone()),
            Self::Handler(handler) => Self::Handler(handler.clone()),
            Self::Link(url) => Self::Link(url.clone()),
        }
    }
}

/// An item's activation, taken out of the menu as it closes, so it runs
/// once the menu's state is no longer borrowed.
pub(crate) struct PendingActivation {
    activation: Activation,
    context: FocusHandle,
}

impl PendingActivation {
    /// Runs the item: an action goes to the action context.
    pub(crate) fn run(self, window: &mut Window, cx: &mut App) {
        match self.activation {
            Activation::Event => {}
            Activation::Action(action) => self.context.dispatch_action(action.as_ref(), window, cx),
            Activation::Handler(handler) => handler(window, cx),
            Activation::Link(url) => cx.open_url(&url),
        }
    }
}

/// What an item shows in the leading slot, before its label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Indicator {
    /// The icon, if any.
    #[default]
    None,
    /// A toggle: a check while on.
    Check(bool),
    /// One of a group the application keeps one of on: a dot while on.
    Radio(bool),
}

/// The state of a row a custom renderer draws; see [`MenuItem::render`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MenuRowState {
    pub(crate) highlighted: bool,
    pub(crate) disabled: bool,
    pub(crate) checked: bool,
}

impl MenuRowState {
    /// Whether the keyboard or the pointer is on the row.
    pub fn is_highlighted(&self) -> bool {
        self.highlighted
    }

    /// Whether the row's item is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Whether the row's toggle or radio is on.
    pub fn is_checked(&self) -> bool {
        self.checked
    }
}

/// A command in a menu.
///
/// The key names the row's element id and the
/// [`MenuEvent::Activated`](super::MenuEvent::Activated) payload, so it
/// comes from the domain and is unique in the menu, submenus included.
/// An item does one thing when chosen: it dispatches an
/// [`action`](Self::action), whose shortcut the row shows, calls
/// [`on_select`](Self::on_select), or opens a [`link`](Self::link). An
/// item with none of them only reports its key.
#[derive(Clone)]
#[non_exhaustive]
pub struct MenuItem {
    key: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    icon: Option<Icon>,
    disabled: bool,
    indicator: Indicator,
    destructive: bool,
    external_link_icon: bool,
    action_context: Option<FocusHandle>,
    render: Option<RowRenderer>,
    activation: Activation,
}

impl MenuItem {
    /// An item with a stable key and the label it shows.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            description: None,
            icon: None,
            disabled: false,
            indicator: Indicator::None,
            destructive: false,
            external_link_icon: true,
            action_context: None,
            render: None,
            activation: Activation::Event,
        }
    }

    /// An icon before the label.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// A line under the label, in the menu's description color.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Makes the item a toggle: a check shows in the leading slot while
    /// `checked` is true, in place of the icon.
    pub fn checked(mut self, checked: bool) -> Self {
        self.indicator = Indicator::Check(checked);
        self
    }

    /// Makes the item one of a group of radio items: a dot shows in the
    /// leading slot while `selected` is true. The application keeps which
    /// one of the group is on.
    pub fn radio(mut self, selected: bool) -> Self {
        self.indicator = Indicator::Radio(selected);
        self
    }

    /// Paints the item in the destructive color, for a command that
    /// deletes or cannot be undone.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// Dispatches `action` when the item is chosen, to the element the
    /// menu acts on, and shows the action's shortcut.
    pub fn action(mut self, action: impl Action) -> Self {
        self.activation = Activation::Action(Box::new(action));
        self
    }

    /// The element this item's action goes to, and whose key bindings its
    /// shortcut shows, in place of the menu's.
    pub fn action_context(mut self, focus_handle: &FocusHandle) -> Self {
        self.action_context = Some(focus_handle.clone());
        self
    }

    /// Calls `handler` when the item is chosen, once the menu has closed.
    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.activation = Activation::Handler(Rc::new(handler));
        self
    }

    /// Opens `url` in the system's browser or mail client when chosen. The
    /// row shows an external-link icon after the label.
    pub fn link(mut self, url: impl Into<SharedString>) -> Self {
        self.activation = Activation::Link(url.into());
        self
    }

    /// Whether a link item shows its external-link icon. On by default.
    pub fn external_link_icon(mut self, visible: bool) -> Self {
        self.external_link_icon = visible;
        self
    }

    /// Draws the inside of the row: `render` gets the row's state and
    /// returns what goes between the row's padding, in place of the
    /// indicator, icon, label, description, and shortcut. The row itself
    /// (its height, highlight, hover, and click) stays the menu's.
    pub fn render<E: IntoElement>(
        mut self,
        render: impl Fn(MenuRowState, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render = Some(Rc::new(move |row, window, cx| {
            render(row, window, cx).into_any_element()
        }));
        self
    }

    /// The item's key.
    pub fn key(&self) -> &SharedString {
        &self.key
    }

    /// The text the item shows.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The line under the label.
    pub fn description_text(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    /// Whether the item ignores input.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Whether the item is a toggle or a radio item that is on.
    pub fn is_checked(&self) -> bool {
        matches!(
            self.indicator,
            Indicator::Check(true) | Indicator::Radio(true)
        )
    }

    /// Whether the item is painted as destructive.
    pub fn is_destructive(&self) -> bool {
        self.destructive
    }

    pub(crate) fn indicator(&self) -> Indicator {
        self.indicator
    }

    pub(crate) fn leading_icon(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }

    pub(crate) fn renderer(&self) -> Option<&RowRenderer> {
        self.render.as_ref()
    }

    /// Whether the row shows the external-link icon.
    pub(crate) fn shows_link_icon(&self) -> bool {
        self.external_link_icon && matches!(self.activation, Activation::Link(_))
    }

    /// The action the item dispatches, if it dispatches one.
    pub(crate) fn dispatched_action(&self) -> Option<&dyn Action> {
        match &self.activation {
            Activation::Action(action) => Some(action.as_ref()),
            _ => None,
        }
    }

    /// The item's own action context, if it has one.
    pub(crate) fn own_action_context(&self) -> Option<&FocusHandle> {
        self.action_context.as_ref()
    }

    /// What choosing the item runs: its action goes to the item's own
    /// action context, else to the menu's `context`.
    pub(crate) fn pending(&self, context: FocusHandle) -> PendingActivation {
        PendingActivation {
            activation: self.activation.clone(),
            context: self.action_context.clone().unwrap_or(context),
        }
    }
}

impl Disableable for MenuItem {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A row that opens a further menu beside it.
#[derive(Clone)]
#[non_exhaustive]
pub struct MenuSubmenu {
    key: SharedString,
    label: SharedString,
    icon: Option<Icon>,
    disabled: bool,
    entries: Vec<MenuEntry>,
}

impl MenuSubmenu {
    /// A submenu with a stable key and the label its row shows.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            icon: None,
            disabled: false,
            entries: Vec::new(),
        }
    }

    /// An icon before the label.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The rows of the menu it opens.
    pub fn entries(mut self, entries: impl IntoIterator<Item = impl Into<MenuEntry>>) -> Self {
        self.entries.extend(entries.into_iter().map(Into::into));
        self
    }

    /// The submenu's key.
    pub fn key(&self) -> &SharedString {
        &self.key
    }

    /// The text the row shows.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Whether the row ignores input.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub(crate) fn leading_icon(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }

    pub(crate) fn children(&self) -> &[MenuEntry] {
        &self.entries
    }

    /// The submenu with its rows tidied as a menu draws them.
    pub(crate) fn normalized(mut self) -> Self {
        self.entries = super::state::normalize(std::mem::take(&mut self.entries));
        self
    }
}

impl Disableable for MenuSubmenu {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// One row of a menu.
#[derive(Clone)]
pub enum MenuEntry {
    /// A command.
    Item(MenuItem),
    /// A hairline between two groups.
    Separator,
    /// A muted heading over a group.
    Label(SharedString),
    /// A row that opens a further menu.
    Submenu(MenuSubmenu),
}

impl MenuEntry {
    /// A group heading.
    pub fn label(text: impl Into<SharedString>) -> Self {
        Self::Label(text.into())
    }

    /// The row's key, for an item or a submenu.
    pub fn key(&self) -> Option<&SharedString> {
        match self {
            Self::Item(item) => Some(item.key()),
            Self::Submenu(submenu) => Some(submenu.key()),
            Self::Separator | Self::Label(_) => None,
        }
    }

    /// Whether the keyboard and the pointer can land on the row.
    pub(crate) fn is_selectable(&self) -> bool {
        match self {
            Self::Item(item) => !item.is_disabled(),
            Self::Submenu(submenu) => !submenu.is_disabled(),
            Self::Separator | Self::Label(_) => false,
        }
    }

    pub(crate) fn submenu(&self) -> Option<&MenuSubmenu> {
        match self {
            Self::Submenu(submenu) => Some(submenu),
            _ => None,
        }
    }
}

impl MenuEntry {
    /// The row for an item of GPUI's application menu data, the one
    /// `App::set_menus` takes, so an application defines its menus once
    /// for the native bar and for a [`MenuBar`](super::MenuBar). An action
    /// item keys its row by the action's name, or by the name and the
    /// label when an earlier row of the same menu has that key already;
    /// a submenu keys its row by its name. A menu the system owns, such
    /// as Services, has no row.
    pub fn from_app_menu_item(item: OwnedMenuItem) -> Option<Self> {
        Self::from_app_item_keyed(item, &mut HashSet::new())
    }

    /// As [`from_app_menu_item`](Self::from_app_menu_item), keeping the
    /// keys unique across the rows `used` holds.
    pub(crate) fn from_app_item_keyed(
        item: OwnedMenuItem,
        used: &mut HashSet<SharedString>,
    ) -> Option<Self> {
        let mut unique = |key: SharedString, label: &str| {
            let key = if used.contains(&key) {
                SharedString::from(format!("{key}/{label}"))
            } else {
                key
            };
            used.insert(key.clone());
            key
        };
        match item {
            OwnedMenuItem::Separator => Some(Self::Separator),
            OwnedMenuItem::Submenu(menu) => {
                let key = unique(menu.name.clone(), &menu.name);
                let entries: Vec<MenuEntry> = menu
                    .items
                    .into_iter()
                    .filter_map(|item| Self::from_app_item_keyed(item, used))
                    .collect();
                Some(
                    MenuSubmenu::new(key, menu.name)
                        .disabled(menu.disabled)
                        .entries(entries)
                        .into(),
                )
            }
            OwnedMenuItem::SystemMenu(_) => None,
            OwnedMenuItem::Action {
                name,
                action,
                checked,
                disabled,
                ..
            } => {
                let key = unique(action.name().into(), &name);
                let mut item = MenuItem::new(key, name).disabled(disabled);
                if checked {
                    item = item.checked(true);
                }
                item.activation = Activation::Action(action);
                Some(item.into())
            }
        }
    }
}

impl From<MenuItem> for MenuEntry {
    fn from(item: MenuItem) -> Self {
        Self::Item(item)
    }
}

impl From<MenuSubmenu> for MenuEntry {
    fn from(submenu: MenuSubmenu) -> Self {
        Self::Submenu(submenu)
    }
}
