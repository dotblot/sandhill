use std::path::PathBuf;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, ExternalPaths,
    Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StyleRefinement, Styled, Subscription, Window,
    assets::IconName,
    base::{
        Disableable, StyledExt as _, TestSupportExt as _,
        input::{InputEvent, TextareaState},
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::{
    ActiveTheme as _, Attachment, AttachmentStatus, AttachmentStrip, Button, ButtonSize,
    EffortMenu, ModelPicker, ModelPickerEvent, ModelPickerState, PermissionEvent, PermissionMenu,
    PermissionState, StatusSelectEvent, StatusSelectState, Textarea,
    collapse::{self, Measured, Need},
};

/// The most lines the prompt grows to before it scrolls.
const DEFAULT_MAX_LINES: usize = 10;

/// What a [`ComposerState`] reports.
#[derive(Clone)]
#[non_exhaustive]
pub enum ComposerEvent {
    /// The prompt was submitted: Enter in the text, or the send button.
    Submit {
        /// The prompt's text.
        text: SharedString,
        /// The attachments sent with it.
        attachments: Vec<Attachment>,
    },
    /// An attachment was added or removed, or its progress changed.
    AttachmentsChanged,
    /// The permission mode changed. The payload is its id.
    PermissionChanged(SharedString),
    /// The "Learn more" row of the permission menu was chosen.
    PermissionLearnMore,
    /// A status item was changed. `item` is the id the application gave
    /// the state to [`ComposerState::watch_status`], and `value` the option.
    StatusChanged {
        /// The status item.
        item: SharedString,
        /// The id of the chosen option.
        value: SharedString,
    },
    /// A model was chosen. The payload is its id.
    ModelChanged(SharedString),
    /// The effort level changed, from `0` to `5`.
    EffortChanged(u8),
    /// The add button was pressed.
    AddClicked,
    /// The microphone button was pressed.
    MicClicked,
    /// Files were dropped on the composer.
    FilesDropped(Vec<PathBuf>),
}

/// The text, the attachments, the permission mode, and the model of a
/// [`Composer`].
///
/// Owned by the view that shows the composer, which observes it so a change
/// renders. It owns the prompt's `TextareaState` and holds the permission
/// and model states, and reports what the user does as a
/// [`ComposerEvent`]. The application changes what it shows through the
/// methods here: add an attachment, move its upload progress, clear the
/// text.
///
/// Enter submits, and Shift+Enter starts a new line. A prompt can be
/// submitted when it has text or a ready attachment, no attachment is
/// still uploading, and the composer is not disabled. A submit clears the
/// text and the attachments unless
/// [`set_clear_on_submit`](Self::set_clear_on_submit) turned that off.
pub struct ComposerState {
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    permission: Entity<PermissionState>,
    models: Entity<ModelPickerState>,
    disabled: bool,
    max_lines: usize,
    clear_on_submit: bool,
    placeholder: SharedString,
    _subscriptions: Vec<Subscription>,
    _status: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for ComposerState {}

/// The prompt's focus, which holds the keyboard for the composer.
impl gpui_kit::Focusable for ComposerState {
    fn focus_handle(&self, cx: &App) -> gpui_kit::FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl ComposerState {
    /// A composer with the default permission modes, an empty model
    /// catalog, and the placeholder "Hi, what do you need today?".
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = SharedString::from("Hi, what do you need today?");
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(placeholder.clone())
                .auto_grow(1, DEFAULT_MAX_LINES)
                .submit_on_enter(true)
        });
        let permission = cx.new(PermissionState::new);
        let models = cx.new(|cx| ModelPickerState::new([], window, cx));
        let mut state = Self {
            input,
            attachments: Vec::new(),
            permission,
            models,
            disabled: false,
            max_lines: DEFAULT_MAX_LINES,
            clear_on_submit: true,
            placeholder,
            _subscriptions: Vec::new(),
            _status: Vec::new(),
        };
        state.watch(window, cx);
        state
    }

    /// Uses `models` for the model picker, in place of the empty one.
    pub fn with_models(
        mut self,
        models: Entity<ModelPickerState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        self.models = models;
        self.watch(window, cx);
        self
    }

    /// Uses `permission` for the permission menu, in place of the default
    /// modes.
    pub fn with_permission(
        mut self,
        permission: Entity<PermissionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        self.permission = permission;
        self.watch(window, cx);
        self
    }

    /// Subscribes to the entities this state owns or holds, replacing any
    /// earlier subscriptions, so a change in one renders the composer and
    /// is reported as a composer event.
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions = vec![
            cx.subscribe_in(
                &self.input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => {
                        this.submit(window, cx);
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe(&self.permission, |_, _, event: &PermissionEvent, cx| {
                match event {
                    PermissionEvent::Changed(id) => {
                        cx.emit(ComposerEvent::PermissionChanged(id.clone()))
                    }
                    PermissionEvent::LearnMore => cx.emit(ComposerEvent::PermissionLearnMore),
                }
                cx.notify();
            }),
            cx.subscribe(&self.models, |_, _, event: &ModelPickerEvent, cx| {
                match event {
                    ModelPickerEvent::Selected(id) => {
                        cx.emit(ComposerEvent::ModelChanged(id.clone()))
                    }
                    ModelPickerEvent::EffortChanged(level) => {
                        cx.emit(ComposerEvent::EffortChanged(*level))
                    }
                    _ => {}
                }
                cx.notify();
            }),
        ];
    }

    /// Forwards the changes of a status item as
    /// [`ComposerEvent::StatusChanged`] under `item`, and renders the
    /// composer when it changes.
    pub fn watch_status(
        &mut self,
        item: impl Into<SharedString>,
        state: &Entity<StatusSelectState>,
        cx: &mut Context<Self>,
    ) {
        let item = item.into();
        self._status.push(
            cx.subscribe(state, move |_, _, event: &StatusSelectEvent, cx| {
                let StatusSelectEvent::Changed(value) = event;
                cx.emit(ComposerEvent::StatusChanged {
                    item: item.clone(),
                    value: value.clone(),
                });
                cx.notify();
            }),
        );
    }

    /// The prompt's text state.
    pub fn input(&self) -> &Entity<TextareaState> {
        &self.input
    }

    /// The permission state.
    pub fn permission(&self) -> &Entity<PermissionState> {
        &self.permission
    }

    /// The model picker state.
    pub fn models(&self) -> &Entity<ModelPickerState> {
        &self.models
    }

    /// The prompt's text.
    pub fn text(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    /// Replaces the prompt's text.
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = SharedString::from(text.to_string());
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    /// Empties the prompt's text.
    pub fn clear_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text("", window, cx);
    }

    /// Puts the caret in the prompt.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// The placeholder.
    pub fn placeholder(&self) -> &SharedString {
        &self.placeholder
    }

    /// Replaces the placeholder shown while the prompt is empty.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if placeholder == self.placeholder {
            return;
        }
        self.placeholder = placeholder.clone();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
    }

    /// The most lines the prompt grows to before it scrolls.
    pub fn max_lines(&self) -> usize {
        self.max_lines
    }

    /// Sets the most lines the prompt grows to, at least one.
    pub fn set_max_lines(&mut self, lines: usize, cx: &mut Context<Self>) {
        let lines = lines.max(1);
        if lines == self.max_lines {
            return;
        }
        self.max_lines = lines;
        self.input
            .update(cx, |input, cx| input.set_auto_grow(1, lines, cx));
    }

    /// Whether the composer ignores input.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Disables or enables the composer: the text, the send button, and
    /// submitting.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if disabled == self.disabled {
            return;
        }
        self.disabled = disabled;
        self.input
            .update(cx, |input, cx| input.set_disabled(disabled, cx));
        cx.notify();
    }

    /// Whether a submit clears the text and the attachments.
    pub fn clears_on_submit(&self) -> bool {
        self.clear_on_submit
    }

    /// Whether a submit clears the text and the attachments. On by default;
    /// turn it off when the application clears them itself, once the
    /// prompt is accepted.
    pub fn set_clear_on_submit(&mut self, clear: bool) {
        self.clear_on_submit = clear;
    }

    /// The attachments, in order.
    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// Adds an attachment at the end. An attachment with the id of one
    /// already here replaces it in place.
    pub fn add_attachment(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        match self
            .attachments
            .iter_mut()
            .find(|existing| existing.id() == attachment.id())
        {
            Some(existing) => *existing = attachment,
            None => self.attachments.push(attachment),
        }
        cx.emit(ComposerEvent::AttachmentsChanged);
        cx.notify();
    }

    /// Removes the attachment with `id` and returns it.
    pub fn remove_attachment(&mut self, id: &str, cx: &mut Context<Self>) -> Option<Attachment> {
        let ix = self.attachments.iter().position(|a| a.id() == id)?;
        let removed = self.attachments.remove(ix);
        cx.emit(ComposerEvent::AttachmentsChanged);
        cx.notify();
        Some(removed)
    }

    /// Sets the upload progress of the attachment with `id`: a percentage,
    /// zero for queued, or `None` for done.
    pub fn set_attachment_progress(
        &mut self,
        id: &str,
        progress: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        if let Some(attachment) = self.attachments.iter_mut().find(|a| a.id() == id) {
            attachment.set_progress(progress);
            cx.emit(ComposerEvent::AttachmentsChanged);
            cx.notify();
        }
    }

    /// Removes every attachment.
    pub fn clear_attachments(&mut self, cx: &mut Context<Self>) {
        if !self.attachments.is_empty() {
            self.attachments.clear();
            cx.emit(ComposerEvent::AttachmentsChanged);
            cx.notify();
        }
    }

    /// Whether the prompt can be submitted now: it has text or an
    /// attachment, nothing is still uploading, and the composer is enabled.
    pub fn can_submit(&self, cx: &App) -> bool {
        let busy = self
            .attachments
            .iter()
            .any(|a| a.status() != AttachmentStatus::Ready);
        !self.disabled
            && !busy
            && (!self.text(cx).trim().is_empty() || !self.attachments.is_empty())
    }

    /// Submits the prompt if it can be, and says whether it did.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.can_submit(cx) {
            return false;
        }
        let text = self.text(cx);
        let attachments = self.attachments.clone();
        cx.emit(ComposerEvent::Submit { text, attachments });
        if self.clear_on_submit {
            self.set_text("", window, cx);
            self.clear_attachments(cx);
        }
        true
    }
}

/// What a control slot holds: the default control, nothing, or the
/// application's own element.
enum Slot {
    Default,
    Hidden,
    Custom(AnyElement),
}

impl Slot {
    fn custom(element: impl IntoElement) -> Self {
        Self::Custom(element.into_any_element())
    }
}

/// A prompt box: a card with the attachments above the text, and a row of
/// controls under it. The row holds an add button, a permission menu, a
/// space, a model picker, a microphone button, and a send button. An
/// optional [status tab](crate::ComposerStatusTab) sits behind the top of
/// the card.
///
/// Every default control can be replaced with the application's own, or
/// left out, and [`toolbar`](Self::toolbar) adds controls after the
/// permission menu. A file dropped on the card is reported as
/// [`ComposerEvent::FilesDropped`].
///
/// ```no_run
/// use gpui_cn::{Composer, ComposerState, ComposerStatusTab};
/// use gpui_kit::{AppContext as _, Context, Window};
///
/// fn prompt(window: &mut Window, cx: &mut Context<()>) -> Composer {
///     let state = cx.new(|cx| ComposerState::new(window, cx));
///     Composer::new("composer", &state)
///         .status(ComposerStatusTab::new("status").context(57.))
/// }
/// ```
///
/// The id derives the ids of the controls, so it must be stable across
/// frames. `Styled` refines the card.
///
/// The builders `disabled`, `max_lines` and `placeholder` are applied to the
/// state on every render, and each setter does nothing when the value is
/// already the state's, so the frame that applies them again changes nothing
/// and asks for no further frame.
///
/// The default icons (the mic, the shield, the file kinds, the git branch) are
/// not in the default `gpui_kit::assets::Assets`: register
/// [`ComposerAssets`](crate::ComposerAssets).
#[derive(IntoElement)]
#[non_exhaustive]
pub struct Composer {
    id: ElementId,
    state: Entity<ComposerState>,
    style: StyleRefinement,
    status: Option<AnyElement>,
    leading: Slot,
    permission: Slot,
    toolbar: Vec<AnyElement>,
    models: Slot,
    mic: Slot,
    send: Slot,
    show_mic: bool,
    placeholder: Option<SharedString>,
    max_lines: Option<usize>,
    disabled: Option<bool>,
}

impl Composer {
    /// A composer on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<ComposerState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            style: StyleRefinement::default(),
            status: None,
            leading: Slot::Default,
            permission: Slot::Default,
            toolbar: Vec::new(),
            models: Slot::Default,
            mic: Slot::Default,
            send: Slot::Default,
            show_mic: false,
            placeholder: None,
            max_lines: None,
            disabled: None,
        }
    }

    /// The strip behind the top of the card. By default there is none.
    pub fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    /// No strip behind the card: the bare card. This is the default.
    pub fn no_status(mut self) -> Self {
        self.status = None;
        self
    }

    /// The placeholder shown while the prompt is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// The most lines the prompt grows to before it scrolls. The default
    /// is ten.
    pub fn max_lines(mut self, lines: usize) -> Self {
        self.max_lines = Some(lines);
        self
    }

    /// Replaces the add button at the left of the controls.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Slot::custom(element);
        self
    }

    /// Leaves out the add button.
    pub fn no_leading(mut self) -> Self {
        self.leading = Slot::Hidden;
        self
    }

    /// Replaces the permission menu.
    pub fn permission_menu(mut self, element: impl IntoElement) -> Self {
        self.permission = Slot::custom(element);
        self
    }

    /// Leaves out the permission menu.
    pub fn no_permission_menu(mut self) -> Self {
        self.permission = Slot::Hidden;
        self
    }

    /// Adds a control after the permission menu. Each call adds one. The
    /// control keeps the width it has: it takes no part in the plan that
    /// shrinks the model and permission labels and turns controls into
    /// icons when the row is narrow.
    pub fn toolbar(mut self, element: impl IntoElement) -> Self {
        self.toolbar.push(element.into_any_element());
        self
    }

    /// Replaces the model picker.
    pub fn model_picker(mut self, element: impl IntoElement) -> Self {
        self.models = Slot::custom(element);
        self
    }

    /// Leaves out the model picker.
    pub fn no_model_picker(mut self) -> Self {
        self.models = Slot::Hidden;
        self
    }

    /// Whether the microphone button shows. Off by default.
    pub fn show_mic(mut self, show: bool) -> Self {
        self.show_mic = show;
        self
    }

    /// Replaces the microphone button.
    pub fn mic(mut self, element: impl IntoElement) -> Self {
        self.mic = Slot::custom(element);
        self
    }

    /// Replaces the send button. The replacement submits through
    /// [`ComposerState::submit`].
    pub fn send_button(mut self, element: impl IntoElement) -> Self {
        self.send = Slot::custom(element);
        self
    }
}

impl Disableable for Composer {
    /// Whether the composer ignores input. Pushes onto the state; leave it
    /// unset to control the state directly.
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }
}

impl Styled for Composer {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn child(parent: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(parent.clone().into(), name.into())
}

impl RenderOnce for Composer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id;
        let state = self.state;
        state.update(cx, |state, cx| {
            if let Some(disabled) = self.disabled {
                state.set_disabled(disabled, cx);
            }
            if let Some(lines) = self.max_lines {
                state.set_max_lines(lines, cx);
            }
            if let Some(placeholder) = self.placeholder.clone() {
                state.set_placeholder(placeholder, window, cx);
            }
        });
        let (input, attachments, permission, models, disabled, can_submit) = {
            let read = state.read(cx);
            (
                read.input.clone(),
                read.attachments.clone(),
                read.permission.clone(),
                read.models.clone(),
                read.disabled,
                read.can_submit(cx),
            )
        };
        let focused = input.read(cx).focus_handle(cx).contains_focused(window, cx);
        let theme = cx.theme();
        let (fill, border, focus_border, radius, accent, gap) = (
            theme.field,
            theme.field_border,
            theme.field_focus_border,
            theme.radius_xl(),
            theme.field_focus_border,
            theme.base.spacing.sm,
        );
        let (ink, muted) = (theme.foreground(), theme.muted_foreground());
        let disabled_opacity = theme.disabled_opacity;
        let radius_full = theme.radius_full();
        let has_status = self.status.is_some();

        let events = |state: &Entity<ComposerState>, event: ComposerEvent| {
            let state = state.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                let event = event.clone();
                state.update(cx, |_, cx| cx.emit(event));
            }
        };

        let has_leading = !matches!(self.leading, Slot::Hidden);
        let has_mic = matches!(self.mic, Slot::Default if self.show_mic)
            || matches!(self.mic, Slot::Custom(_));
        let has_send = !matches!(self.send, Slot::Hidden);
        let leading = match self.leading {
            Slot::Default => Some(
                Button::new(child(&id, "add"))
                    .ghost()
                    .size(ButtonSize::Default)
                    .icon(IconName::Plus)
                    .text_color(ink)
                    .accessibility_label("Add")
                    .tooltip("Add")
                    .disabled(disabled)
                    .on_click(events(&state, ComposerEvent::AddClicked))
                    .into_any_element(),
            ),
            Slot::Hidden => None,
            Slot::Custom(element) => Some(element),
        };
        let model_state = models.clone();
        let has_default_models = matches!(self.models, Slot::Default);
        let permission_menu = matches!(self.permission, Slot::Default)
            .then(|| PermissionMenu::new(child(&id, "permission"), &permission));
        let model_picker = has_default_models.then(|| {
            ModelPicker::new(child(&id, "models"), &models).align(gpui_kit::base::Align::End)
        });
        let effort_menu = (has_default_models && model_state.read(cx).effort().is_some())
            .then(|| EffortMenu::new(child(&id, "effort"), &model_state));
        let toolbar_width = window
            .use_keyed_state(child(&id, "toolbar-width"), cx, |_, _| Measured::default())
            .read(cx)
            .clone();
        let fixed = {
            let control = cx.theme().metrics.control_md;
            Need {
                natural: control,
                min: control,
                icon_only: None,
            }
        };
        let mut needs = Vec::new();
        let mut at = |need: Need| {
            needs.push(need);
            needs.len() - 1
        };
        if has_leading {
            at(fixed);
        }
        at(Need {
            natural: px(0.),
            min: px(0.),
            icon_only: None,
        });
        let permission_at = permission_menu
            .as_ref()
            .map(|menu| at(menu.need(window, cx)));
        let model_need = model_picker.as_ref().map(|picker| picker.need(window, cx));
        let model_at = model_need.map(&mut at);
        let effort_at = effort_menu
            .as_ref()
            .and_then(|menu| menu.need(window, cx))
            .map(&mut at);
        if has_mic {
            at(fixed);
        }
        if has_send {
            at(fixed);
        }
        let order: Vec<usize> = [effort_at, permission_at, model_at]
            .into_iter()
            .flatten()
            .collect();
        let collapsed = collapse::plan(toolbar_width.width(), gap, gap, &needs, &order);
        let is_collapsed = |index: Option<usize>| {
            index.is_some_and(|index| collapsed.get(index).is_some_and(|fit| fit.is_icon()))
        };
        let permission = match self.permission {
            Slot::Default => permission_menu.map(|menu| {
                menu.icon_only(is_collapsed(permission_at))
                    .flex_shrink(1.)
                    .into_any_element()
            }),
            Slot::Hidden => None,
            Slot::Custom(element) => Some(element),
        };
        let models = match self.models {
            Slot::Default => model_picker.map(|picker| {
                // The popover's own root takes no style, so a box around it
                // is what shrinks in the row, down to the pill's minimum.
                let collapsed = is_collapsed(model_at);
                let min = model_need
                    .map(|need| match (collapsed, need.icon_only) {
                        (true, Some(icon_only)) => icon_only,
                        _ => need.min,
                    })
                    .unwrap_or_default();
                div()
                    .flex()
                    .flex_shrink(1.)
                    .min_w(min)
                    .child(picker.icon_only(collapsed))
                    .into_any_element()
            }),
            Slot::Hidden => None,
            Slot::Custom(element) => Some(element),
        };
        let effort = effort_menu.map(|menu| {
            menu.icon_only(is_collapsed(effort_at))
                .flex_shrink(1.)
                .into_any_element()
        });
        let mic = match self.mic {
            Slot::Default if self.show_mic => Some(
                Button::new(child(&id, "mic"))
                    .outline()
                    .size(ButtonSize::Default)
                    .icon(IconName::Mic)
                    .text_color(muted)
                    .rounded(radius_full)
                    .accessibility_label("Dictate")
                    .tooltip("Dictate")
                    .disabled(disabled)
                    .on_click(events(&state, ComposerEvent::MicClicked))
                    .into_any_element(),
            ),
            Slot::Default | Slot::Hidden => None,
            Slot::Custom(element) => Some(element),
        };
        let send = match self.send {
            Slot::Default => Some(
                Button::new(child(&id, "send"))
                    .primary()
                    .size(ButtonSize::Default)
                    .icon(IconName::ArrowUp)
                    .rounded(radius_full)
                    .accessibility_label("Send")
                    .tooltip("Send")
                    .disabled(!can_submit)
                    .on_click({
                        let state = state.clone();
                        move |_, window, cx| {
                            state.update(cx, |state, cx| {
                                state.submit(window, cx);
                            });
                        }
                    })
                    .into_any_element(),
            ),
            Slot::Hidden => None,
            Slot::Custom(element) => Some(element),
        };

        let strip = (!attachments.is_empty()).then(|| {
            let state = state.clone();
            AttachmentStrip::new(child(&id, "attachments"))
                .attachments(attachments)
                .on_dismiss(move |attachment, _, cx| {
                    let attachment = attachment.clone();
                    state.update(cx, |state, cx| {
                        state.remove_attachment(&attachment, cx);
                    });
                })
        });

        let drop_state = state.clone();
        let card = div()
            .id(child(&id, "card"))
            .test_support()
            .flex()
            .flex_col()
            .w_full()
            .gap(gap)
            .p_1()
            .rounded(radius)
            .bg(fill)
            .border_1()
            .border_color(if focused { focus_border } else { border })
            .when(disabled, |this| this.opacity(disabled_opacity))
            .when(has_status, |this| this.mt(-gpui_kit::px(1.)).relative())
            .refine_style(&self.style)
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(accent))
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                drop_state.update(cx, |_, cx| cx.emit(ComposerEvent::FilesDropped(paths)));
            })
            .children(strip)
            .child(
                Textarea::new(&input)
                    .id(child(&id, "text"))
                    .accessibility_label("Prompt")
                    .bg(gpui_kit::transparent_black())
                    .border_0()
                    .rounded_none(),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(gap)
                    .child(toolbar_width.probe())
                    .children(leading)
                    .children(permission)
                    .children(self.toolbar)
                    .child(div().flex_1())
                    .children(models)
                    .children(effort)
                    .children(mic)
                    .children(send),
            );

        div()
            .id(id)
            .test_support()
            .flex()
            .flex_col()
            .w_full()
            .children(self.status)
            .child(card)
    }
}
