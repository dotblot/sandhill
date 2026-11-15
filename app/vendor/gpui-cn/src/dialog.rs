//! A modal window over the page, as shadcn's `Dialog`.

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, StyleRefinement, Styled, Window,
    assets::IconName,
    base::{self, StyledExt as _, TestSupportExt as _, actions::Cancel, h_flex, v_flex},
    div,
};

use crate::{
    ActiveTheme as _, Button, ButtonSize, Icon,
    menu::{MenuLook, MenuMotion},
};

type OpenChange = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Where a dialog's focus lives: the surface that traps it, and the
/// element that had it before the dialog opened.
struct Focus {
    surface: FocusHandle,
    previous: Option<FocusHandle>,
    opened: bool,
}

/// A modal window over the page, as shadcn's `Dialog`: a dimmed backdrop
/// and a centered surface with a title, a description, a body, and a
/// footer.
///
/// `gpui-base` owns the behavior: the dialog is a modal layer that traps
/// focus, and Escape or a press on the backdrop asks to close it. This
/// component draws it as a menu's panel: the popover surface, its
/// hairline, the large radius, and the shadow, with the theme's dialog
/// width and padding. Focus moves into the dialog when it opens, to the
/// element given to [`track_focus`](Self::track_focus) or the surface,
/// and goes back to where it was when the dialog closes.
///
/// ```
/// use gpui_cn::{Button, Dialog};
/// use gpui_kit::{IntoElement, ParentElement as _, div};
///
/// fn confirm(open: bool) -> impl IntoElement {
///     Dialog::new("confirm")
///         .open(open)
///         .title("Delete the file?")
///         .description("This cannot be undone.")
///         .footer(Button::new("confirm-ok").primary().label("Delete"))
///         .child(div().child("report.pdf"))
/// }
/// ```
///
/// The application owns the open state: [`open`](Self::open) sets it, and
/// [`on_open_change`](Self::on_open_change) reports Escape, a backdrop
/// press, and the close button, which should set it. The id must be
/// stable across frames.
///
/// `Styled` refines the surface after the tokens, so `.w(..)` sets the
/// width.
#[derive(IntoElement)]
pub struct Dialog {
    id: ElementId,
    open: bool,
    title: Option<SharedString>,
    description: Option<SharedString>,
    children: Vec<AnyElement>,
    footer: Option<AnyElement>,
    show_close_button: bool,
    on_open_change: Option<OpenChange>,
    focus: Option<FocusHandle>,
    top: Option<Pixels>,
    style: StyleRefinement,
}

impl Dialog {
    /// A closed dialog with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            open: false,
            title: None,
            description: None,
            children: Vec::new(),
            footer: None,
            show_close_button: true,
            on_open_change: None,
            focus: None,
            top: None,
            style: StyleRefinement::default(),
        }
    }

    /// Shows or hides the dialog.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Called when Escape, a press on the backdrop, or the close button
    /// asks to close the dialog. It gets `false`.
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    /// The title, in the theme's large semibold text.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The description under the title, in muted text.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The row of actions under the body, lined up to the trailing edge.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// Whether the close button shows in the top corner. On by default.
    pub fn show_close_button(mut self, show: bool) -> Self {
        self.show_close_button = show;
        self
    }

    /// Places the surface this far below the top edge of the window, in
    /// place of the vertical center.
    pub(crate) fn top(mut self, offset: Pixels) -> Self {
        self.top = Some(offset);
        self
    }

    /// The element that takes focus when the dialog opens, such as a
    /// field in it. Without one the surface takes it.
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

impl Styled for Dialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let child = |name: &'static str| ElementId::NamedChild(self.id.clone().into(), name.into());
        let focus: Entity<Focus> = window.use_keyed_state(child("focus"), cx, |_, cx| Focus {
            surface: cx.focus_handle(),
            previous: None,
            opened: false,
        });
        let surface = focus.read(cx).surface.clone();
        move_focus(&focus, self.open, self.focus.as_ref(), window, cx);

        let theme = cx.theme();
        let look = MenuLook::of(theme, window.rem_size());
        let overlay = theme.dialog_overlay;
        let width = theme.metrics.dialog_width;
        let padding = theme.metrics.dialog_padding;
        let title_style = theme.base.typography.lg;
        let Some(motion) =
            MenuMotion::sample(child("presence"), self.open, false, &look, window, cx)
        else {
            return div().into_any_element();
        };

        let close = self.show_close_button.then(|| {
            let target = surface.clone();
            div().absolute().top_3().right_3().child(
                Button::new(child("close"))
                    .ghost()
                    .size(ButtonSize::Sm)
                    .icon(Icon::from(IconName::Close))
                    .accessibility_label("Close")
                    .on_click(move |_, window, cx| {
                        target.dispatch_action(&Cancel, window, cx);
                    }),
            )
        });
        let header = (self.title.is_some() || self.description.is_some()).then(|| {
            v_flex()
                .gap_2()
                .children(self.title.map(|title| {
                    div()
                        .text_size(title_style.size)
                        .line_height(title_style.line_height)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title)
                }))
                .children(
                    self.description
                        .map(|text| div().text_color(look.muted_foreground).child(text)),
                )
        });
        let popup = v_flex()
            .id(child("popup"))
            .test_support()
            .relative()
            .occlude()
            .w(width)
            .max_w_full()
            .gap_4()
            .p(padding)
            .rounded(look.radius)
            .bg(look.surface)
            .border_1()
            .border_color(look.border)
            .shadow(look.shadow(motion.shadow_strength))
            .opacity(motion.opacity)
            .text_size(look.text_size)
            .line_height(look.line_height)
            .text_color(look.foreground)
            .refine_style(&self.style)
            .children(close)
            .children(header)
            .children(self.children)
            .children(
                self.footer
                    .map(|footer| h_flex().justify_end().gap_2().child(footer)),
            );

        let on_open_change = self.on_open_change;
        let mut dialog = base::Dialog::new(cx)
            .focus_handle(surface)
            .open(true)
            .backdrop(
                div()
                    .id(child("backdrop"))
                    .test_support()
                    .absolute()
                    .size_full()
                    .bg(overlay.opacity(motion.opacity)),
            )
            .popup(popup)
            .p_4();
        if let Some(offset) = self.top {
            dialog = dialog.items_start().pt(offset);
        }
        if let Some(handler) = on_open_change {
            dialog = dialog.on_open_change(move |open, _, window, cx| handler(open, window, cx));
        }
        dialog.into_any_element()
    }
}

/// Moves focus into the dialog as it opens, and back to what had it as it
/// closes, once each. The trap's own focus goes to the surface unless the
/// dialog names an element.
fn move_focus(
    focus: &Entity<Focus>,
    open: bool,
    target: Option<&FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let (surface, previous, opened) = {
        let focus = focus.read(cx);
        (focus.surface.clone(), focus.previous.clone(), focus.opened)
    };
    if open && !opened {
        let before = window.focused(cx);
        target.unwrap_or(&surface).focus(window, cx);
        focus.update(cx, |focus, _| {
            focus.previous = before;
            focus.opened = true;
        });
    } else if !open && opened {
        if surface.contains_focused(window, cx)
            && let Some(previous) = previous
        {
            previous.focus(window, cx);
        }
        focus.update(cx, |focus, _| {
            focus.previous = None;
            focus.opened = false;
        });
    }
}
