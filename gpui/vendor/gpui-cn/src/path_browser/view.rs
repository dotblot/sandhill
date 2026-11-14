//! The parts of the two pickers' dialogs that are the same: the fixed list
//! box, its messages, a row's frame, and the "Load more" row.

use gpui_kit::{
    AnyElement, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement, Pixels,
    Role, StatefulInteractiveElement, Styled, TestSupportExt as _,
    base::{h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
};

use super::{
    CancelNewFolder, CommitNewFolder, Entry, Host, ListError, MoreState, NEW_FOLDER_CONTEXT,
    NewFolderRow,
};
use crate::{ActiveTheme as _, Button, ButtonSize, Icon, Input, Spinner, menu::MenuLook};

pub(crate) const EMPTY_FOLDERS: &str = "No folders found in this directory.";
pub(crate) const EMPTY_FILES: &str = "Nothing to show in this directory.";
pub(crate) const NO_MATCH: &str = "No folders match.";
pub(crate) const NO_FILE_MATCH: &str = "No matches.";
const LOAD_MORE: &str = "Load more";
const LOADING_MORE: &str = "Loading more";
const MORE_FAILED: &str = "Unable to load more. Try again.";

/// A message centered in the list box.
pub(crate) fn message(id: ElementId, look: &MenuLook, text: &'static str) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(look.muted_foreground)
        .child(text)
        .into_any_element()
}

/// Why a directory could not be listed, centered in the list box with the
/// action that can fix it: Retry after a lost connection or an error, and
/// "Sign in" when the source needs the user to log in and the application
/// gave the picker a handler for that.
pub(crate) fn failure<E: Entry, O: Host<E>>(
    id: &ElementId,
    look: &MenuLook,
    state: &Entity<O>,
    error: &ListError,
    can_sign_in: bool,
) -> AnyElement {
    let child = |name: &'static str| ElementId::NamedChild(id.clone().into(), name.into());
    let retry = state.clone();
    let sign_in = state.clone();
    v_flex()
        .id(child("message"))
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .px(look.padding * 2.)
        .text_color(look.muted_foreground)
        .child(div().text_center().child(error.message()))
        .when(error.is_retryable(), |this| {
            this.child(
                Button::new(child("retry"))
                    .outline()
                    .size(ButtonSize::Sm)
                    .label("Retry")
                    .on_click(move |_, _, cx| {
                        retry.update(cx, |state, cx| state.browser_mut().retry(cx));
                    }),
            )
        })
        .when(error.needs_sign_in() && can_sign_in, |this| {
            this.child(
                Button::new(child("sign-in"))
                    .outline()
                    .size(ButtonSize::Sm)
                    .label("Sign in")
                    .on_click(move |_, window, cx| {
                        let handler = sign_in.read(cx).browser().auth_handler();
                        if let Some(handler) = handler {
                            handler(window, cx);
                        }
                    }),
            )
        })
        .into_any_element()
}

/// A spinner centered in the list box.
pub(crate) fn loading(id: ElementId, look: &MenuLook) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(look.muted_foreground)
        .child(Spinner::new(id))
        .into_any_element()
}

/// The row that names a new folder, at the top of the list: a folder
/// icon and a field with the name, or a spinner while the source makes the
/// folder. A refusal shows under the row in the destructive color.
pub(crate) fn new_folder_row<E: Entry, O: Host<E>>(
    id: &ElementId,
    look: &MenuLook,
    state: &Entity<O>,
    row: &NewFolderRow,
    cx: &gpui_kit::App,
) -> AnyElement {
    let child = |name: &'static str| ElementId::NamedChild(id.clone().into(), name.into());
    let cancel = state.clone();
    let commit = state.clone();
    let error_color = cx.theme().destructive();
    let caption = cx.theme().base.typography.xs;
    v_flex()
        .id(child("new-folder"))
        .test_support()
        .w_full()
        .flex_shrink_0()
        .pb(look.padding)
        .key_context(NEW_FOLDER_CONTEXT)
        .on_action(move |_: &CancelNewFolder, window, cx| {
            cancel.update(cx, |state, cx| {
                state.browser_mut().dismiss_new_folder(window, cx);
            });
        })
        .on_action(move |_: &CommitNewFolder, _, cx| {
            commit.update(cx, |state, cx| {
                state.browser_mut().commit_new_folder(cx).ok();
            });
        })
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .px(look.row_padding)
                .child(if row.creating {
                    div()
                        .size_4()
                        .flex_shrink_0()
                        .text_color(look.muted_foreground)
                        .child(Spinner::new(child("new-folder-spinner")))
                        .into_any_element()
                } else {
                    Icon::from(gpui_kit::assets::IconName::Folder)
                        .size_4()
                        .flex_shrink_0()
                        .text_color(look.muted_foreground)
                        .into_any_element()
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&row.input).readonly(row.creating)),
                ),
        )
        .children(row.error.clone().map(|message| {
            div()
                .id(child("new-folder-error"))
                .test_support()
                .px(look.row_padding)
                .pt(look.padding)
                .text_size(caption.size)
                .line_height(caption.line_height)
                .text_color(error_color)
                .child(message)
        }))
        .into_any_element()
}

/// The bordered box that holds the list, as tall as the theme's list
/// height in every state, so the dialog never changes size. The row that
/// names a new folder, when there is one, sits above the list inside the
/// box.
pub(crate) fn list_box(
    id: ElementId,
    look: &MenuLook,
    list_height: Pixels,
    top: Option<AnyElement>,
    body: AnyElement,
) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .h(list_height + look.padding * 2. + gpui_kit::px(2.))
        .p(look.padding)
        .rounded(look.radius)
        .border_1()
        .border_color(look.border)
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(top)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// The row that chooses the current folder. Its slot is always there, so
/// the dialog is as tall while a folder loads or fails as when it is
/// listed.
pub(crate) fn slot(look: &MenuLook, content: Option<AnyElement>) -> AnyElement {
    h_flex()
        .h(look.row_height)
        .flex_shrink_0()
        .children(content)
        .into_any_element()
}

/// The frame of a row: its height and padding, the highlight, the cursor.
pub(crate) fn row_frame(
    id: ElementId,
    look: &MenuLook,
    role: Role,
    highlighted: bool,
    selected: bool,
    pointer_cursors: bool,
) -> impl ParentElement + Styled + StatefulInteractiveElement + IntoElement {
    crate::menu::row_frame(look, highlighted || selected)
        .id(id)
        .test_support()
        .role(role)
        .h(look.row_height)
        .when(highlighted, |this| this.aria_active_descendant())
        .when(pointer_cursors, |this| this.cursor_pointer())
}

/// How the request for the next page stands, and why it failed.
pub(crate) struct MoreRow {
    pub(crate) state: MoreState,
    pub(crate) error: Option<ListError>,
    pub(crate) can_sign_in: bool,
}

/// The last row while more entries follow: it loads them on a click, and
/// asks again after a failure.
pub(crate) fn more_row<E: Entry, O: Host<E>>(
    id: &ElementId,
    look: &MenuLook,
    state: &Entity<O>,
    more: MoreRow,
    highlighted: bool,
    row: usize,
    pointer_cursors: bool,
) -> AnyElement {
    let click = state.clone();
    let MoreRow {
        state: more,
        error,
        can_sign_in,
    } = more;
    let needs_sign_in = error.as_ref().is_some_and(ListError::needs_sign_in) && can_sign_in;
    let hover = state.clone();
    row_frame(
        ElementId::NamedChild(id.clone().into(), "more".into()),
        look,
        Role::Button,
        highlighted,
        false,
        pointer_cursors,
    )
    .text_color(look.muted_foreground)
    .when(more == MoreState::Loading, |this| {
        this.child(Spinner::new(ElementId::NamedChild(
            id.clone().into(),
            "more-spinner".into(),
        )))
    })
    .child(match more {
        MoreState::Idle => LOAD_MORE.into(),
        MoreState::Loading => LOADING_MORE.into(),
        MoreState::Failed => error
            .as_ref()
            .map_or_else(|| MORE_FAILED.into(), ListError::message),
    })
    .when(more == MoreState::Failed && needs_sign_in, |this| {
        this.child(div().text_color(look.foreground).child("Sign in"))
    })
    .when(!look.touch, |this| {
        this.on_mouse_move(move |_, _, cx| highlight_on_hover::<E, O>(&hover, row, cx))
    })
    .on_click(move |_, window, cx| {
        if needs_sign_in && let Some(handler) = click.read(cx).browser().auth_handler() {
            handler(window, cx);
            return;
        }
        click.update(cx, |owner, cx| owner.browser_mut().load_more(cx));
    })
    .into_any_element()
}

/// Puts the keyboard highlight on the row the pointer is on.
pub(crate) fn highlight_on_hover<E: Entry, O: Host<E>>(
    state: &Entity<O>,
    row: usize,
    cx: &mut gpui_kit::App,
) {
    state.update(cx, |owner, cx| {
        if owner.browser().highlighted != Some(row) {
            owner.browser_mut().highlighted = Some(row);
            cx.notify();
        }
    });
}

/// A column that stacks a dialog's parts.
pub(crate) fn stack() -> gpui_kit::Div {
    v_flex().gap_2()
}
