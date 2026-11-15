//! The open menu of a select: the panel, the search field, the rows in a
//! virtual list, and what stands in for them while empty or loading.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement, Length,
    MouseButton, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window,
    base::{TestSupportExt as _, v_flex},
    div,
    prelude::FluentBuilder as _,
};

use super::{
    item::{SelectEntry, SelectItem, SelectValue},
    state::SelectState,
};
use crate::{
    ScrollArea,
    menu::{
        MenuLook, MenuMotion, MenuPanels, TextMenuBuilder, check_slot, label_block, line_slot,
        open_text_menu, row_frame, row_line, search_row, separator,
    },
};

pub(super) type ItemRenderer<V> =
    Rc<dyn Fn(&SelectItem<V>, SelectRow, &mut Window, &mut App) -> AnyElement>;
pub(super) type LabelRenderer = Rc<dyn Fn(&SharedString, &mut Window, &mut App) -> AnyElement>;
pub(super) type PartRenderer = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The state of a row a custom item renderer draws; see
/// [`Select::render_item`](super::Select::render_item).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SelectRow {
    highlighted: bool,
    selected: bool,
    disabled: bool,
    matched: Vec<Range<usize>>,
}

impl SelectRow {
    /// Whether the keyboard or the pointer is on the row.
    pub fn is_highlighted(&self) -> bool {
        self.highlighted
    }

    /// Whether the row's item is selected.
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Whether the row's item is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The byte ranges of the label that the search matched, to draw in
    /// the text color at medium weight. Empty without a query.
    pub fn matched(&self) -> &[Range<usize>] {
        &self.matched
    }
}

/// What the rows of one menu are drawn from, owned by the list's row
/// builder, which outlives the render that made it.
pub(super) struct Rows<V: SelectValue> {
    pub id: ElementId,
    pub state: Entity<SelectState<V>>,
    pub look: MenuLook,
    pub render_item: Option<ItemRenderer<V>>,
    pub render_label: Option<LabelRenderer>,
    pub pointer_cursors: bool,
}

/// The menu: the search field, the rows in a scroll region, or the empty
/// or loading element.
pub(super) struct Menu<V: SelectValue> {
    pub rows: Rows<V>,
    pub content_focus: FocusHandle,
    pub width: Option<Length>,
    pub trigger_width: Pixels,
    pub empty_text: SharedString,
    pub render_empty: Option<PartRenderer>,
    pub render_loading: Option<PartRenderer>,
    pub motion: MenuMotion,
    /// Shapes the search field's right-click menu, as a field's does.
    pub search_menu_builder: Option<TextMenuBuilder>,
    /// Whether a right click on the search field opens its menu.
    pub search_menu_enabled: bool,
}

impl<V: SelectValue> Menu<V> {
    pub fn render(self, window: &mut Window, cx: &mut App) -> AnyElement {
        let Menu {
            rows,
            content_focus,
            width,
            trigger_width,
            empty_text,
            render_empty,
            render_loading,
            motion,
            search_menu_builder,
            search_menu_enabled,
        } = self;
        let look = rows.look.clone();
        let look = &look;
        let id = rows.id.clone();
        let state = rows.state.clone();
        let (list, row_count, row_hint, searching, search, search_menu) = {
            let state = state.read(cx);
            (
                state.list().clone(),
                state.row_count(),
                state.row_hint(cx),
                state.is_searching(),
                state.search_input().cloned(),
                state.search_menu().cloned(),
            )
        };
        let search_menu_open = search_menu
            .as_ref()
            .is_some_and(|menu| menu.read(cx).is_open());
        let menu_id = ElementId::NamedChild(id.clone().into(), "menu".into());
        let rows_id = ElementId::NamedChild(id.clone().into(), "rows".into());
        let rows_max_height = look.rows_max_height(search.is_some());
        // While a handler is still answering, the loading element stands
        // in for the rows, so what shows is never the answer to an older
        // query beside a promise of the new one. Without one the last rows
        // stay until the answer, and an empty menu shows nothing rather
        // than "No results" it may take back.
        let loading = searching
            .then(|| render_loading.as_ref().map(|render| render(window, cx)))
            .flatten();
        let body = if let Some(loading) = loading {
            loading
        } else if row_count == 0 {
            match (searching, render_empty) {
                (true, _) => div().into_any_element(),
                (false, Some(render)) => render(window, cx),
                (false, None) => div()
                    .py_6()
                    .text_center()
                    .text_color(look.muted_foreground)
                    .child(empty_text)
                    .into_any_element(),
            }
        } else {
            ScrollArea::list(rows_id, &list, row_hint, move |row, window, cx| {
                rows.render(row, window, cx)
            })
            .max_h(rows_max_height)
            .into_any_element()
        };
        v_flex()
            .id(menu_id)
            .test_support()
            .track_focus(&content_focus)
            .tab_group()
            .map(|this| match width {
                Some(width) => this.w(width),
                None => {
                    let min_width = if search.is_some() {
                        look.search_min_width
                    } else {
                        look.min_width
                    };
                    this.min_w(trigger_width.max(min_width))
                        .max_w(look.max_width)
                }
            })
            // The fade sits on the panel itself, since GPUI shapes the
            // shadow from the element that carries it.
            .relative()
            .top(motion.offset)
            .opacity(motion.opacity)
            .p(look.padding)
            .rounded(look.radius)
            .bg(look.surface)
            .border_1()
            .border_color(look.border)
            .shadow(look.shadow(motion.shadow_strength))
            .text_size(look.text_size)
            .line_height(look.line_height)
            .text_color(look.foreground)
            .on_mouse_down_out({
                let state = state.clone();
                let search = search.clone();
                let search_menu = search_menu.clone();
                move |_, window, cx| {
                    // The search box's touch edit menu and its right-click
                    // menu float outside the panel; a press on either is
                    // the search's, not a close.
                    let edit_menu_open = search.as_ref().is_some_and(|search| {
                        search
                            .read(cx)
                            .touch_selection()
                            .is_some_and(|selection| selection.is_menu_open())
                    }) || search_menu
                        .as_ref()
                        .is_some_and(|menu| menu.read(cx).is_open());
                    if !edit_menu_open {
                        state.update(cx, |state, cx| state.close(window, cx));
                    }
                }
            })
            .when_some(search, |this, search| {
                this.child(
                    search_row(&search, look, window, cx)
                        .id(ElementId::NamedChild(id.clone().into(), "search".into()))
                        .test_support()
                        .when_some(search_menu.filter(|_| search_menu_enabled), |this, menu| {
                            let input = search.clone();
                            let opener = menu.clone();
                            this.on_mouse_up(MouseButton::Right, move |event, window, cx| {
                                open_text_menu(
                                    &opener,
                                    &input,
                                    event.position,
                                    search_menu_builder.as_ref(),
                                    window,
                                    cx,
                                );
                            })
                            .when(search_menu_open, |this| {
                                this.child(MenuPanels::new(
                                    ElementId::NamedChild(id.clone().into(), "search-menu".into()),
                                    &menu,
                                ))
                            })
                        }),
                )
                .child(separator(look))
            })
            .child(body)
            .into_any_element()
    }
}

impl<V: SelectValue> Rows<V> {
    fn render(&self, row: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let Rows {
            id,
            state,
            look,
            render_item,
            render_label,
            pointer_cursors,
        } = self;
        let pointer_cursors = *pointer_cursors;
        let (entry, highlighted, selected, row_height, query) = {
            let state = state.read(cx);
            let Some(entry) = state.entry_at(row) else {
                return div().into_any_element();
            };
            let selected = entry
                .item()
                .is_some_and(|item| state.is_selected(item.value()));
            (
                entry.clone(),
                state.is_highlighted(row),
                selected,
                state.row_height(),
                state.query(cx),
            )
        };
        match entry {
            SelectEntry::Separator => separator(look),
            SelectEntry::Label(text) => match render_label {
                Some(render) => div()
                    .w_full()
                    .child(render(&text, window, cx))
                    .into_any_element(),
                None => div()
                    .w_full()
                    .h(look.row_height)
                    .px(look.row_padding)
                    .flex()
                    .items_center()
                    .text_color(look.muted_foreground)
                    .child(text)
                    .into_any_element(),
            },
            SelectEntry::Item(item) => {
                let disabled = item.is_disabled();
                let value = item.value().clone();
                let choose_state = state.clone();
                let hover_state = state.clone();
                let matched = crate::fuzzy::matched(&query, item.label()).unwrap_or_default();
                let content: Vec<AnyElement> = match render_item {
                    Some(render) => vec![render(
                        &item,
                        SelectRow {
                            highlighted,
                            selected,
                            disabled,
                            matched,
                        },
                        window,
                        cx,
                    )],
                    None => default_row_content(
                        &item,
                        &ElementId::NamedChild(id.clone().into(), item.key()),
                        selected,
                        &matched,
                        look,
                        window,
                        cx,
                    ),
                };
                row_frame(look, highlighted && !disabled)
                    .id(ElementId::NamedChild(id.clone().into(), item.key()))
                    .test_support()
                    // A fixed height is what a scroll to a far row counts
                    // on before the row is laid out.
                    .when_some(row_height, |this, height| this.h(height).min_h(height))
                    .text_color(if disabled {
                        look.muted_foreground
                    } else {
                        look.foreground
                    })
                    .map(|this| {
                        if !disabled && pointer_cursors {
                            this.cursor_pointer()
                        } else {
                            this.cursor_default()
                        }
                    })
                    .children(content)
                    // A pointer that moves over a row highlights it; a row that
                    // scrolls under a resting pointer does not, so the
                    // keyboard's highlight is not snatched back. A finger has
                    // no hover.
                    .when(!look.touch, |this| {
                        this.on_mouse_move(move |_, _, cx| {
                            hover_state.update(cx, |state, cx| state.highlight_row(row, cx));
                        })
                    })
                    .on_click(move |_, window, cx| {
                        choose_state.update(cx, |state, cx| state.choose(&value, window, cx));
                    })
                    .into_any_element()
            }
        }
    }
}

/// The inside of an item row as the reference app draws it: the leading
/// element, the label over its description, and the check.
fn default_row_content<V: SelectValue>(
    item: &SelectItem<V>,
    row_id: &ElementId,
    selected: bool,
    matched: &[Range<usize>],
    look: &MenuLook,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    let disabled = item.is_disabled();
    let part = |name: &'static str| ElementId::NamedChild(row_id.clone().into(), name.into());
    let leading = item.render_leading(window, cx).map(|leading| {
        line_slot(look)
            .id(part("leading"))
            .test_support()
            .child(leading)
    });
    let check = check_slot(look, selected, disabled);
    vec![
        row_line()
            .children(leading)
            .child(
                div()
                    .id(part("label"))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .child(label_block(
                        item.label().clone(),
                        matched,
                        item.description_text().cloned(),
                        look,
                    )),
            )
            .child(check)
            .into_any_element(),
    ]
}
