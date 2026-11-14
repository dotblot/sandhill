//! A command palette in a dialog over the page, as shadcn's
//! `CommandDialog`.

use std::rc::Rc;

use gpui_kit::{
    App, Entity, Focusable as _, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Styled as _, Subscription, Window,
};

use crate::{ActiveTheme as _, Command, CommandEvent, CommandState, Dialog, menu::MenuLook};

type OpenChange = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Closes the dialog when a command is chosen. It lives in the window's
/// keyed state, so the subscription is made once and the handler is the
/// latest one the application gave.
struct Closer {
    handler: Option<OpenChange>,
    open: bool,
    _subscription: Subscription,
}

/// A [`Command`] palette in a [`Dialog`], as shadcn's `CommandDialog`
/// and the palettes opened with Cmd+K.
///
/// The dialog has no title and no close button: Escape and a press on the
/// backdrop close it. It sits at the center horizontally and at the
/// theme's `command_dialog_top` below the top edge, so it rests in the
/// upper third of the window as Spotlight does, and does not move when
/// its height changes. It is the theme's `command_dialog_width` wide, and
/// narrower in a window with no room for it, with the dialog layer's
/// margin on both sides.
///
/// The search field takes focus when the dialog opens, and focus returns
/// to where it was when it closes. The box is as tall as a palette's
/// list can be, in every state, so the dialog does not jump while the
/// number of matches changes; the list scrolls inside it. Choosing a
/// command closes the dialog after the state reports
/// [`CommandEvent::Confirmed`].
///
/// ```
/// use gpui_cn::{CommandDialog, CommandState};
/// use gpui_kit::{Entity, IntoElement};
///
/// fn palette(state: &Entity<CommandState>, open: bool) -> impl IntoElement {
///     CommandDialog::new("palette", state)
///         .open(open)
///         .empty("Nothing matches.")
/// }
/// ```
///
/// The application owns the open state: [`open`](Self::open) sets it, and
/// [`on_open_change`](Self::on_open_change) reports Escape, a backdrop
/// press, and a chosen command, which should clear it. The commands, the
/// query, and the placeholder live in the [`CommandState`]. The id must
/// be stable across frames.
#[derive(IntoElement)]
pub struct CommandDialog {
    id: gpui_kit::ElementId,
    state: Entity<CommandState>,
    open: bool,
    empty: Option<SharedString>,
    on_open_change: Option<OpenChange>,
}

impl CommandDialog {
    /// A closed palette dialog on `state`, with a stable id.
    pub fn new(id: impl Into<gpui_kit::ElementId>, state: &Entity<CommandState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            open: false,
            empty: None,
            on_open_change: None,
        }
    }

    /// Shows or hides the dialog.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// The text shown while no command matches. The default is the
    /// palette's own.
    pub fn empty(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = Some(text.into());
        self
    }

    /// Called with `false` when Escape, a press on the backdrop, or a
    /// chosen command asks to close the dialog.
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for CommandDialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let child = |name: &'static str| {
            gpui_kit::ElementId::NamedChild(self.id.clone().into(), name.into())
        };
        let state = self.state.clone();
        let closer = window.use_keyed_state(child("closer"), cx, |window, cx| Closer {
            handler: None,
            open: false,
            _subscription: cx.subscribe_in(
                &state,
                window,
                |this: &mut Closer, _, event: &CommandEvent, window, cx| {
                    if this.open
                        && matches!(
                            event,
                            CommandEvent::Confirmed(_) | CommandEvent::Submitted(_)
                        )
                        && let Some(handler) = this.handler.clone()
                    {
                        handler(false, window, cx);
                    }
                },
            ),
        });
        let (handler, open) = (self.on_open_change.clone(), self.open);
        closer.update(cx, |closer, _| {
            closer.handler = handler;
            closer.open = open;
        });

        let theme = cx.theme();
        let look = MenuLook::of(theme, window.rem_size());
        let (width, top) = (
            theme.metrics.command_dialog_width,
            theme.metrics.command_dialog_top,
        );
        let focus = self.state.focus_handle(cx);
        let mut command = Command::new(child("command"), &self.state)
            .bordered(false)
            .w_full()
            .max_w_full()
            .h(look.palette_height());
        if let Some(empty) = self.empty {
            command = command.empty(empty);
        }
        let on_open_change = self.on_open_change;
        let mut dialog = Dialog::new(self.id.clone())
            .open(self.open)
            .show_close_button(false)
            .top(top)
            .w(width)
            .p_0()
            .overflow_hidden()
            .track_focus(&focus)
            .child(command);
        if let Some(handler) = on_open_change {
            dialog = dialog.on_open_change(move |open, window, cx| handler(open, window, cx));
        }
        dialog
    }
}
