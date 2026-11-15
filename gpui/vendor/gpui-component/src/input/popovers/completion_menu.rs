use std::{cell::Cell, ops::Range, rc::Rc, time::Duration};

use gpui::{
    Action, AnyElement, App, AppContext, Context, CursorStyle, DismissEvent, DispatchPhase, Empty,
    Entity, EntityId, EventEmitter, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    RenderOnce, SharedString, Size, StatefulInteractiveElement as _, Styled, StyledText,
    Subscription, WeakEntity, Window, canvas, deferred, div, prelude::FluentBuilder, px, rems,
};
use gpui_base::TestSupportExt as _;
use gpui_base::input::CompletionHighlighter;
use lsp_types::{CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind};

const MAX_MENU_HEIGHT: Pixels = px(240.);
const MAX_MENU_WIDTH: Pixels = px(520.);

/// Drop ranges a custom highlighter cannot safely paint, then order the rest.
///
/// A range past the label, or one that begins or ends inside a character,
/// would split a glyph. Empty ranges paint nothing, so they go too. What
/// remains is sorted and merged: GPUI builds text runs assuming highlights
/// are ordered and do not overlap, and an overlap makes the run longer than
/// the label.
fn retained_highlight_ranges(
    label: &str,
    ranges: impl IntoIterator<Item = Range<usize>>,
) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = ranges
        .into_iter()
        .filter(|range| {
            range.start < range.end
                && range.end <= label.len()
                && label.is_char_boundary(range.start)
                && label.is_char_boundary(range.end)
        })
        .collect();
    ranges.sort_by_key(|range| range.start);

    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

use crate::{
    ActiveTheme, IndexPath, Selectable, ThemeStyled as _, actions, h_flex,
    input::{
        self, EditorState,
        popovers::{
            CaretAnchoredPopup, DOC_TEXT, completion_docs, editor_popover, render_markdown,
            side_bounds, tag_fences,
        },
    },
    list::{List, ListDelegate, ListEvent, ListState},
    menu::MenuItemElement,
};

struct ContextMenuDelegate {
    query: SharedString,
    menu: Entity<CompletionMenu>,
    /// Sandhill: whose `CompletionDocs` fill in missing documentation.
    editor: EntityId,
    items: Vec<Rc<CompletionItem>>,
    selected_ix: usize,
    highlight: Rc<CompletionHighlighter>,
}

/// Sandhill: how long an item stays selected before its documentation is
/// asked for, so arrowing through the list asks only where it stops.
const DOCS_PAUSE: Duration = Duration::from_millis(120);

impl ContextMenuDelegate {
    fn set_items(&mut self, items: Vec<CompletionItem>) {
        self.items = items.into_iter().map(Rc::new).collect();
        self.selected_ix = 0;
    }

    fn selected_item(&self) -> Option<&Rc<CompletionItem>> {
        self.items.get(self.selected_ix)
    }

    /// Sandhill: asks the editor's `CompletionDocs` for the selected item's
    /// documentation when it came without any, once it has stayed selected
    /// for `DOCS_PAUSE`. An answer of none is kept as empty documentation,
    /// so the item isn't asked about again.
    fn ask_docs(&self, cx: &mut Context<ListState<Self>>) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        if item.documentation.is_some() {
            return;
        }
        let Some(docs) = completion_docs(self.editor) else {
            return;
        };
        cx.spawn(async move |list, cx| {
            cx.background_executor().timer(DOCS_PAUSE).await;
            let task = list
                .update(cx, |list, cx| {
                    let still = list
                        .delegate()
                        .selected_item()
                        .is_some_and(|selected| Rc::ptr_eq(selected, &item));
                    if still { docs(&item, cx) } else { None }
                })
                .ok()
                .flatten()?;
            let markdown = task.await.unwrap_or_default();
            list.update(cx, |list, cx| {
                let items = &mut list.delegate_mut().items;
                if let Some(slot) = items.iter_mut().find(|slot| Rc::ptr_eq(slot, &item)) {
                    let mut filled = (**slot).clone();
                    filled.documentation = Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }));
                    *slot = Rc::new(filled);
                    cx.notify();
                }
            })
            .ok()
        })
        .detach();
    }
}

#[derive(IntoElement)]
struct CompletionMenuItem {
    ix: usize,
    item: Rc<CompletionItem>,
    children: Vec<AnyElement>,
    selected: bool,
    query: SharedString,
    highlight: Rc<CompletionHighlighter>,
}

impl CompletionMenuItem {
    fn new(
        ix: usize,
        item: Rc<CompletionItem>,
        query: SharedString,
        highlight: Rc<CompletionHighlighter>,
    ) -> Self {
        Self {
            ix,
            item,
            children: vec![],
            selected: false,
            query,
            highlight,
        }
    }
}
impl Selectable for CompletionMenuItem {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for CompletionMenuItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}
impl RenderOnce for CompletionMenuItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let item = self.item;

        let deprecated = item.deprecated.unwrap_or(false);
        let highlights = retained_highlight_ranges(
            &item.label,
            (self.highlight)(self.query.as_ref(), item.as_ref()),
        )
        .into_iter()
        .map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(cx.theme().blue),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();

        MenuItemElement::new(self.ix)
            .aria_label(item.label.clone())
            .selected(self.selected)
            .text_sm()
            .py_0()
            .px_2()
            .h(rems(26. / 16.))
            .mb_0p5()
            .rounded(cx.theme().radius)
            .when(item.deprecated.unwrap_or(false), |this| this.line_through())
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .flex_none()
                            .child(kind_badge(item.kind, cx))
                            .child(StyledText::new(item.label.clone()).with_highlights(highlights)),
                    )
                    // Sandhill: a long signature is cut short rather than
                    // widening the menu over the window.
                    .when_some(item.detail.as_ref(), |this, detail| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                // Its full text would set the menu's width.
                                .max_w(px(280.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_right()
                                .text_color(cx.theme().muted_foreground)
                                .italic()
                                .when(deprecated, |this| this.line_through())
                                .child(detail.clone()),
                        )
                    }),
            )
            .children(self.children)
    }
}

/// Sandhill: a small lettered badge before each item saying what it is
/// (function, method, class, variable, constant, module, keyword…), in a
/// colour of its own. Items of no known kind keep the space empty, so the
/// labels stay lined up.
fn kind_badge(kind: Option<CompletionItemKind>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mark = kind.and_then(|kind| {
        Some(match kind {
            CompletionItemKind::FUNCTION | CompletionItemKind::CONSTRUCTOR => ("f", theme.magenta),
            CompletionItemKind::METHOD => ("m", theme.magenta),
            CompletionItemKind::CLASS
            | CompletionItemKind::STRUCT
            | CompletionItemKind::INTERFACE => ("C", theme.yellow),
            CompletionItemKind::ENUM => ("E", theme.yellow),
            CompletionItemKind::TYPE_PARAMETER => ("T", theme.yellow),
            CompletionItemKind::EVENT => ("e", theme.yellow),
            CompletionItemKind::MODULE => ("M", theme.cyan),
            CompletionItemKind::VARIABLE => ("v", theme.blue),
            CompletionItemKind::FIELD | CompletionItemKind::PROPERTY => ("p", theme.blue),
            CompletionItemKind::CONSTANT
            | CompletionItemKind::ENUM_MEMBER
            | CompletionItemKind::VALUE => ("c", theme.green),
            CompletionItemKind::KEYWORD => ("k", theme.red),
            CompletionItemKind::SNIPPET => ("s", theme.muted_foreground),
            CompletionItemKind::FILE => ("F", theme.muted_foreground),
            CompletionItemKind::FOLDER => ("D", theme.muted_foreground),
            CompletionItemKind::OPERATOR => ("o", theme.muted_foreground),
            _ => return None,
        })
    });
    let badge = div()
        .flex_none()
        .size(px(16.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD);
    match mark {
        Some((letter, color)) => badge
            .bg(color.opacity(0.16))
            .text_color(color)
            .child(letter)
            .into_any_element(),
        None => badge.into_any_element(),
    }
}

impl EventEmitter<DismissEvent> for ContextMenuDelegate {}

impl ListDelegate for ContextMenuDelegate {
    type Item = CompletionMenuItem;

    fn items_count(&self, _: usize, _: &gpui::App) -> usize {
        self.items.len()
    }

    fn render_item(
        &mut self,
        ix: crate::IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = self.items.get(ix.row)?;
        Some(CompletionMenuItem::new(
            ix.row,
            item.clone(),
            self.query.clone(),
            self.highlight.clone(),
        ))
    }

    fn set_selected_index(
        &mut self,
        ix: Option<crate::IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected_ix = ix.map(|i| i.row).unwrap_or(0);
        self.ask_docs(cx);
        cx.notify();
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(item) = self.selected_item() else {
            return;
        };

        self.menu.update(cx, |this, cx| {
            this.select_item(&item, window, cx);
        });
    }
}

/// A context menu for code completions and code actions.
pub struct CompletionMenu {
    offset: usize,
    editor: WeakEntity<EditorState>,
    list: Entity<ListState<ContextMenuDelegate>>,
    open: bool,

    /// The offset of the first character that triggered the completion.
    pub(crate) trigger_start_offset: Option<usize>,
    query: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl CompletionMenu {
    /// Creates a new `CompletionMenu` with the given offset and completion items.
    ///
    /// NOTE: This element should not call from EditorState::new, unless that will stack overflow.
    pub(crate) fn new(
        editor: Entity<EditorState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let view = cx.entity();
            let menu = ContextMenuDelegate {
                query: SharedString::default(),
                menu: view,
                editor: editor.entity_id(),
                items: vec![],
                selected_ix: 0,
                highlight: Rc::new(gpui_base::input::highlight_prefix),
            };

            let list = cx.new(|cx| ListState::new(menu, window, cx));

            let _subscriptions =
                vec![
                    cx.subscribe(&list, |this: &mut Self, _, ev: &ListEvent, cx| {
                        match ev {
                            ListEvent::Confirm(_) => {
                                this.dismiss(cx);
                            }
                            _ => {}
                        }
                        cx.notify();
                    }),
                ];

            Self {
                offset: 0,
                editor: editor.downgrade(),
                list,
                open: false,
                trigger_start_offset: None,
                query: SharedString::default(),
                _subscriptions,
            }
        })
    }

    fn select_item(&mut self, item: &CompletionItem, window: &mut Window, cx: &mut Context<Self>) {
        let item = item.clone();
        let range = self.trigger_start_offset.unwrap_or(self.offset)..self.offset;

        let editor = self.editor.clone();

        cx.spawn_in(window, async move |_, cx| {
            editor.update_in(cx, |editor, window, cx| {
                editor.insert_completion(&item, range, window, cx);
            })
        })
        .detach();

        self.dismiss(cx);
    }

    pub(crate) fn handle_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.open {
            return false;
        }

        cx.propagate();
        if input::Enter::is_primary(&*action) {
            self.on_action_enter(window, cx);
        } else if action.partial_eq(&input::Escape) {
            self.on_action_escape(window, cx);
        } else if action.partial_eq(&input::MoveUp) {
            self.on_action_up(window, cx);
        } else if action.partial_eq(&input::MoveDown) {
            self.on_action_down(window, cx);
        } else {
            return false;
        }

        true
    }

    fn on_action_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.list.read(cx).delegate().selected_item().cloned() else {
            return;
        };
        self.select_item(&item, window, cx);
    }

    fn on_action_escape(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
    }

    fn on_action_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |this, cx| {
            this.on_action_select_prev(&actions::SelectUp, window, cx)
        });
    }

    fn on_action_down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |this, cx| {
            this.on_action_select_next(&actions::SelectDown, window, cx)
        });
    }

    /// Synchronize a hidden editor menu without dismissing a newer response.
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.trigger_start_offset = None;
        cx.notify();
    }

    /// Dismiss the menu in response to a user action.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.hide(cx);
        let editor = self.editor.clone();
        cx.spawn(async move |_, cx| {
            let _ = editor.update(cx, |editor, cx| editor.dismiss_completion_overlay(cx));
        })
        .detach();
    }

    /// Sets the trigger start offset if it is not already set.
    pub(crate) fn update_query(&mut self, start_offset: usize, query: impl Into<SharedString>) {
        if self.trigger_start_offset.is_none() {
            self.trigger_start_offset = Some(start_offset);
        }
        self.query = query.into();
    }

    pub(crate) fn show(
        &mut self,
        offset: usize,
        items: impl Into<Vec<CompletionItem>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = items.into();
        self.offset = offset;
        self.open = true;
        self.list.update(cx, |this, cx| {
            let longest_ix = items
                .iter()
                .enumerate()
                .max_by_key(|(_, item)| {
                    item.label.len() + item.detail.as_ref().map(|d| d.len()).unwrap_or(0)
                })
                .map(|(ix, _)| ix)
                .unwrap_or(0);

            this.delegate_mut().query = self.query.clone();
            this.delegate_mut().set_items(items);
            this.set_selected_index(Some(IndexPath::new(0)), window, cx);
            this.set_item_to_measure_index(IndexPath::new(longest_ix), window, cx);
        });

        cx.notify();
    }

    /// Replace the label highlighter and repaint an open menu.
    pub(crate) fn set_highlight(
        &mut self,
        highlight: Rc<CompletionHighlighter>,
        cx: &mut Context<Self>,
    ) {
        self.list.update(cx, |list, cx| {
            if Rc::ptr_eq(&list.delegate().highlight, &highlight) {
                return;
            }
            list.delegate_mut().highlight = highlight;
            cx.notify();
        });
    }
}

impl Render for CompletionMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }

        if self.list.read(cx).delegate().items.is_empty() {
            self.open = false;
            return Empty.into_any_element();
        }

        let Some(editor) = self.editor.upgrade() else {
            return Empty.into_any_element();
        };
        // The anchor is resolved in the popup's deferred prepaint from the
        // input's current-frame geometry; the content below is built there.
        let configured_max = editor.read(cx).lsp().completion_menu.max_width;
        let language = editor.read(cx).language_name();
        let list = self.list.clone();
        let docs = self.list.clone();
        let menu = cx.weak_entity();

        deferred(
            CaretAnchoredPopup::new(self.editor.clone(), move |anchor, window, cx| {
                // `anchor` is in window coordinates, from the current frame.
                // Sandhill: no wider than MAX_MENU_WIDTH, leaving room for
                // the documentation beside it.
                let max_width = configured_max
                    .min(MAX_MENU_WIDTH)
                    .min(window.viewport_size().width - anchor.x);
                editor_popover("completion-menu", cx)
                    .popover_style(cx)
                    .text_sm()
                    // Each virtual row includes the menu's inter-item gap.
                    .pb_0p5()
                    .test_support()
                    .max_w(max_width)
                    .min_w(px(120.))
                    .child(List::new(&list).max_h(MAX_MENU_HEIGHT))
                    .on_mouse_down_out({
                        let menu = menu.clone();
                        move |event, _, cx: &mut App| {
                            // The documentation beside the menu is part of it.
                            if side_bounds().is_some_and(|b| b.contains(&event.position)) {
                                return;
                            }
                            if let Some(menu) = menu.upgrade() {
                                menu.update(cx, |menu, cx| menu.dismiss(cx));
                            }
                        }
                    })
                    .into_any_element()
            })
            .side(move |most, window, cx| {
                let delegate = docs.read(cx).delegate();
                let ix = delegate.selected_ix;
                let text = match delegate.selected_item()?.documentation.clone()? {
                    lsp_types::Documentation::String(s) => s,
                    lsp_types::Documentation::MarkupContent(mc) => mc.value,
                };
                if text.trim().is_empty() {
                    return None;
                }
                Some(doc_panel(ix, tag_fences(&text, &language), most, window, cx))
            }),
        )
        .with_priority(gpui_base::POPUP_PRIORITY)
        .into_any_element()
    }
}

thread_local! {
    /// Sandhill: the documentation panel's size, the same for every item so
    /// browsing never moves or resizes it; the user drags its corner.
    static DOC_SIZE: Cell<Size<Pixels>> = const {
        Cell::new(Size { width: px(440.), height: px(280.) })
    };
    /// A corner drag in progress: where it began and the size then.
    static DOC_DRAG: Cell<Option<(Point<Pixels>, Size<Pixels>)>> = const { Cell::new(None) };
}

const DOC_MIN: Size<Pixels> = Size {
    width: px(220.),
    height: px(100.),
};

/// The selected item's documentation: prose in the interface font, code in
/// the code font with its language's colours, scrolling inside a panel of
/// the user's size that never covers more than `most`.
fn doc_panel(
    ix: usize,
    markdown: String,
    most: Size<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let wanted = DOC_SIZE.with(Cell::get);
    let size = Size {
        width: wanted.width.min(most.width),
        height: wanted.height.min(most.height),
    };
    let muted = cx.theme().muted_foreground;
    editor_popover("completion-doc", cx)
        .popover_style(cx)
        .relative()
        .p_0()
        .w(size.width)
        .h(size.height)
        .text_size(DOC_TEXT)
        .font_family(cx.theme().font_family.clone())
        .child(
            div()
                .id(("completion-doc-text", ix))
                .size_full()
                .overflow_y_scroll()
                .px_2()
                .py_1()
                .child(render_markdown(("doc", ix), markdown, window, cx)),
        )
        .child(
            div()
                .id("completion-doc-resize")
                .absolute()
                .right_0()
                .bottom_0()
                .size(px(14.))
                .flex()
                .items_end()
                .justify_end()
                .pr(px(2.))
                .text_size(px(9.))
                .text_color(muted)
                .cursor(CursorStyle::ResizeUpLeftDownRight)
                .child("◢")
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    DOC_DRAG.with(|drag| drag.set(Some((event.position, size))));
                    cx.stop_propagation();
                    window.refresh();
                }),
        )
        .child(
            canvas(
                |_, _, _| (),
                |_, _, window, _| {
                    window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let Some((start, from)) = DOC_DRAG.with(Cell::get) else {
                            return;
                        };
                        if !event.dragging() {
                            DOC_DRAG.with(|drag| drag.set(None));
                            return;
                        }
                        let delta = event.position - start;
                        let viewport = window.viewport_size();
                        DOC_SIZE.with(|doc| {
                            doc.set(Size {
                                width: (from.width + delta.x)
                                    .clamp(DOC_MIN.width, viewport.width.max(DOC_MIN.width)),
                                height: (from.height + delta.y)
                                    .clamp(DOC_MIN.height, viewport.height.max(DOC_MIN.height)),
                            })
                        });
                        cx.stop_propagation();
                        window.refresh();
                    });
                    window.on_mouse_event(|event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture
                            && event.button == MouseButton::Left
                            && DOC_DRAG.with(|drag| drag.take()).is_some()
                        {
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .size_0(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_ranges_are_ordered_and_do_not_overlap() {
        assert_eq!(
            retained_highlight_ranges("flag", [2..3, 0..1]),
            vec![0..1, 2..3]
        );
        assert_eq!(retained_highlight_ranges("flag", [0..3, 2..4]), vec![0..4]);
        assert_eq!(
            retained_highlight_ranges("flag", [0..1, 1..2, 5..9, 2..3]),
            vec![0..3]
        );
        assert_eq!(retained_highlight_ranges("你", [0..1, 0..3]), vec![0..3]);
    }

    #[gpui::test]
    fn hiding_an_old_menu_does_not_dismiss_a_new_completion_response(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::init);
        let window = cx.add_empty_window();
        let (editor, menu) = window.update(|window, cx| {
            let editor = cx.new(|cx| EditorState::new(window, cx).default_value("co"));
            let menu = CompletionMenu::new(editor.clone(), window, cx);
            let old_items = vec![CompletionItem {
                label: "const".into(),
                ..Default::default()
            }];
            editor.update(cx, |editor, cx| {
                editor.present_completion_items(0, "c", old_items.clone(), cx);
            });
            menu.update(cx, |menu, cx| {
                menu.update_query(0, "c");
                menu.show(1, old_items, window, cx);
                // The overlay observes the old request being hidden while a
                // replacement request is pending. Its deferred dismissal must
                // not act on the replacement response that arrives next.
                menu.hide(cx);
            });
            editor.update(cx, |editor, cx| {
                editor.present_completion_items(
                    0,
                    "co",
                    vec![CompletionItem {
                        label: "const".into(),
                        ..Default::default()
                    }],
                    cx,
                );
            });
            (editor, menu)
        });

        cx.run_until_parked();

        cx.read(|cx| {
            let completion = editor.read(cx).completion_menu_state();
            assert!(completion.open, "an old menu dismissed the new response");
            assert_eq!(completion.query, "co");
            assert_eq!(completion.items.len(), 1);
            assert_eq!(completion.items[0].label, "const");
        });
        drop(menu);
    }
}
