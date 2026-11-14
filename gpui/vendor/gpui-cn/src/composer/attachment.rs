use std::rc::Rc;

use gpui_kit::{
    App, ElementId, Hsla, ImageSource, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Pixels, RenderOnce, SharedString, StyleRefinement, Styled,
    StyledImage as _, TestSupportExt as _, Window,
    assets::IconName,
    base::{StyledExt as _, transition},
    div, img,
    prelude::FluentBuilder as _,
};

use crate::{ActiveTheme as _, Button, ButtonSize, Icon, Progress, Theme};

type DismissHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// What kind of file an attachment is, which decides its icon.
///
/// The kinds come from one table: each row holds the kind's
/// icon, its label, and the file extensions that map to it. A
/// new kind is a row, not a new `match` arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttachmentKind {
    /// A picture, shown as its own thumbnail.
    Image,
    /// Text and documents.
    Document,
    /// Tables and sheets.
    Spreadsheet,
    /// Slide decks.
    Presentation,
    /// Source code.
    Code,
    /// Moving pictures.
    Video,
    /// Anything else. The default.
    #[default]
    Other,
}

/// One row of the kind table.
struct KindRow {
    kind: AttachmentKind,
    label: &'static str,
    icon: IconName,
    extensions: &'static [&'static str],
}

/// The kind table. `Other` is last and has no extensions: it is what no
/// other row claims.
const KINDS: [KindRow; 7] = [
    KindRow {
        kind: AttachmentKind::Image,
        label: "Image",
        icon: IconName::Image,
        extensions: &[
            "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic", "avif",
        ],
    },
    KindRow {
        kind: AttachmentKind::Document,
        label: "Document",
        icon: IconName::FileText,
        extensions: &["txt", "md", "pdf", "doc", "docx", "rtf", "odt", "pages"],
    },
    KindRow {
        kind: AttachmentKind::Spreadsheet,
        label: "Spreadsheet",
        icon: IconName::FileSpreadsheet,
        extensions: &["csv", "tsv", "xls", "xlsx", "ods", "numbers"],
    },
    KindRow {
        kind: AttachmentKind::Presentation,
        label: "Presentation",
        icon: IconName::Presentation,
        extensions: &["ppt", "pptx", "odp", "key"],
    },
    KindRow {
        kind: AttachmentKind::Code,
        label: "Code",
        icon: IconName::FileCode,
        extensions: &[
            "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "h", "cpp", "swift", "rb",
            "sh", "json", "toml", "yaml", "yml", "html", "css",
        ],
    },
    KindRow {
        kind: AttachmentKind::Video,
        label: "Video",
        icon: IconName::Video,
        extensions: &["mp4", "mov", "mkv", "webm", "avi", "m4v"],
    },
    KindRow {
        kind: AttachmentKind::Other,
        label: "File",
        icon: IconName::File,
        extensions: &[],
    },
];

impl AttachmentKind {
    fn row(self) -> &'static KindRow {
        KINDS
            .iter()
            .find(|row| row.kind == self)
            .unwrap_or(&KINDS[KINDS.len() - 1])
    }

    /// The kind a file name belongs to, by its extension, compared without
    /// regard to case. A name with no known extension is `Other`.
    pub fn of_file_name(name: &str) -> Self {
        let extension = name.rsplit_once('.').map(|(_, extension)| extension);
        extension
            .and_then(|extension| {
                KINDS.iter().find(|row| {
                    row.extensions
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(extension))
                })
            })
            .map_or(Self::Other, |row| row.kind)
    }

    /// The icon of the kind.
    pub fn icon(self) -> IconName {
        self.row().icon
    }

    /// A short name for the kind, such as "Spreadsheet".
    pub fn label(self) -> &'static str {
        self.row().label
    }
}

/// Where an attachment is in its life.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum AttachmentStatus {
    /// Done, or never needed uploading.
    Ready,
    /// Waiting for its upload to start: progress is zero.
    Queued,
    /// Uploading, with the percentage done, above zero and up to 100. A
    /// file at 100 is still uploading until the application sets its
    /// progress to `None`, which is what lets Send wait for the last byte.
    Uploading(f32),
}

/// A progress percentage held to `0..=100`, with a value that is not a
/// number counting as zero, which is queued.
fn clamp_percent(value: f32) -> f32 {
    if value.is_nan() {
        0.
    } else {
        value.clamp(0., 100.)
    }
}

/// A file in a composer: what the strip shows for it.
///
/// The id comes from the domain and is unique in the composer. The
/// progress is a percentage: `None` for a file that is ready, zero for one
/// that is queued, and above zero while it uploads. Setting it back to
/// `None` finishes the upload. A queued tile shows its ring at zero and
/// can be removed, an uploading tile shows the counter and cannot, and a
/// ready tile can be removed. Send waits for every attachment to be ready.
///
/// ```
/// use gpui_cn::{Attachment, AttachmentKind};
///
/// let report = Attachment::new("a1", "report.xlsx").progress(55.);
/// assert_eq!(report.kind_of(), AttachmentKind::Spreadsheet);
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct Attachment {
    id: SharedString,
    name: SharedString,
    kind: AttachmentKind,
    progress: Option<f32>,
    image: Option<ImageSource>,
}

impl Attachment {
    /// A ready attachment. The kind comes from the name's extension.
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        let name = name.into();
        Self {
            id: id.into(),
            kind: AttachmentKind::of_file_name(&name),
            name,
            progress: None,
            image: None,
        }
    }

    /// The kind, in place of the one the name's extension gives.
    pub fn kind(mut self, kind: AttachmentKind) -> Self {
        self.kind = kind;
        self
    }

    /// The upload progress as a percentage, clamped to `0..=100`.
    pub fn progress(mut self, progress: impl Into<Option<f32>>) -> Self {
        self.progress = progress.into().map(clamp_percent);
        self
    }

    /// The picture an image attachment shows as its thumbnail. It makes the
    /// attachment an image.
    pub fn image(mut self, source: impl Into<ImageSource>) -> Self {
        self.image = Some(source.into());
        self.kind = AttachmentKind::Image;
        self
    }

    /// The id.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// The file name.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// The kind.
    pub fn kind_of(&self) -> AttachmentKind {
        self.kind
    }

    /// The upload progress, if the file is not ready.
    pub fn progress_percent(&self) -> Option<f32> {
        self.progress
    }

    /// Replaces the upload progress, as [`progress`](Self::progress).
    pub fn set_progress(&mut self, progress: Option<f32>) {
        self.progress = progress.map(clamp_percent);
    }

    /// Where the attachment is in its life.
    pub fn status(&self) -> AttachmentStatus {
        match self.progress {
            None => AttachmentStatus::Ready,
            Some(value) if value <= 0. => AttachmentStatus::Queued,
            Some(value) => AttachmentStatus::Uploading(value),
        }
    }
}

/// The look of a tile, read from the theme in one borrow.
struct Look {
    side: Pixels,
    text: gpui_kit::base::TextStyleToken,
    radius: Pixels,
    radius_full: Pixels,
    busy_opacity: f32,
    fill: Hsla,
    border: Hsla,
    tint: Hsla,
    name: Hsla,
    percent: Hsla,
    frost: Hsla,
    frost_glyph: Hsla,
}

/// One tile of the attachment strip: an image thumbnail, or a bordered
/// file tile with a tinted kind icon and the name. A tile that uploads
/// draws a circular [`Progress`](crate::Progress) at its top left and a
/// percent counter at its top right; when the upload ends, the counter and
/// the ring fade out as the dismiss button fades in.
///
/// ```
/// use gpui_cn::{Attachment, AttachmentTile};
///
/// let _ = AttachmentTile::new("tile", Attachment::new("a1", "notes.md"))
///     .on_dismiss(|id, _, _| println!("remove {id}"));
/// ```
///
/// The id keys motion and the dismiss button, so it must be stable across
/// frames.
///
/// The default icons (the mic, the shield, the file kinds, the git branch) are
/// not in the default `gpui_kit::assets::Assets`: register
/// [`ComposerAssets`](crate::ComposerAssets).
#[derive(IntoElement)]
#[non_exhaustive]
pub struct AttachmentTile {
    id: ElementId,
    attachment: Attachment,
    style: StyleRefinement,
    on_dismiss: Option<DismissHandler>,
}

impl AttachmentTile {
    /// A tile for `attachment` with a stable id.
    pub fn new(id: impl Into<ElementId>, attachment: Attachment) -> Self {
        Self {
            id: id.into(),
            attachment,
            style: StyleRefinement::default(),
            on_dismiss: None,
        }
    }

    /// Called with the attachment's id when its dismiss button is pressed.
    /// Without a handler there is no button.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl Styled for AttachmentTile {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AttachmentTile {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let slide = Theme::global(cx).motion.slide_transition();
        let attachment = self.attachment;
        let status = attachment.status();
        let kind = attachment.kind;
        let look = {
            let theme = cx.theme();
            let metrics = &theme.metrics;
            Look {
                side: metrics.attachment_tile,
                text: theme.base.typography.xs,
                radius: theme.radius_lg(),
                radius_full: theme.radius_full(),
                busy_opacity: theme.disabled_opacity,
                fill: theme.field,
                border: theme.field_border,
                tint: theme.muted_foreground(),
                name: theme.muted_foreground(),
                percent: theme.foreground(),
                frost: theme.scrim,
                frost_glyph: theme.solid_foreground,
            }
        };
        let child_id =
            |name: &'static str| ElementId::NamedChild(self.id.clone().into(), name.into());
        let uploading = matches!(status, AttachmentStatus::Uploading(_));
        let busy = status != AttachmentStatus::Ready;
        let shown = transition(
            child_id("uploading"),
            if uploading { 1. } else { 0. },
            slide.clone(),
            window,
            cx,
        );
        let ring = transition(
            child_id("ring-shown"),
            if busy { 1. } else { 0. },
            slide.clone(),
            window,
            cx,
        );
        let image = attachment.image.clone();
        let has_image = image.is_some();
        let percent = match status {
            AttachmentStatus::Uploading(value) => value,
            AttachmentStatus::Queued => 0.,
            AttachmentStatus::Ready => 100.,
        };
        let dismiss = self.on_dismiss.filter(|_| !uploading).map(|on_dismiss| {
            let id = attachment.id.clone();
            let (fill, glyph) = if has_image {
                (look.frost, look.frost_glyph)
            } else {
                (look.border, look.name)
            };
            Button::new(child_id("dismiss"))
                .ghost()
                .size(ButtonSize::Xs)
                .icon(Icon::from(IconName::Close).size_3())
                .accessibility_label(format!("Remove {}", attachment.name))
                .absolute()
                .top_1()
                .right_1()
                .p_0()
                .size_4()
                .rounded(look.radius_full)
                .bg(fill)
                .text_color(glyph)
                .opacity(1. - shown)
                .on_click(move |_, window, cx| on_dismiss(&id, window, cx))
        });

        div()
            .id(self.id.clone())
            .test_support()
            .relative()
            .flex_shrink_0()
            .size(look.side)
            .text_size(look.text.size)
            .line_height(look.text.line_height)
            .refine_style(&self.style)
            .child(
                div()
                    .size_full()
                    .rounded(look.radius)
                    .overflow_hidden()
                    .bg(look.fill)
                    .border_1()
                    .border_color(look.border)
                    .map(|this| match image {
                        Some(source) => this.child(
                            img(source)
                                .size_full()
                                .rounded(look.radius)
                                .object_fit(ObjectFit::Cover)
                                .when(busy, |this| this.opacity(look.busy_opacity)),
                        ),
                        None => {
                            this.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_between()
                                    .py_1()
                                    .child(div().flex().flex_1().items_center().child(
                                        Icon::from(kind.icon()).size_5().text_color(look.tint),
                                    ))
                                    .child(
                                        div()
                                            .w_full()
                                            .px_1()
                                            .truncate()
                                            .text_center()
                                            .text_color(look.name)
                                            .when(busy, |this| this.opacity(look.busy_opacity))
                                            .child(attachment.name.clone()),
                                    ),
                            )
                        }
                    }),
            )
            .when(ring > 0.001, |this| {
                this.child(
                    div()
                        .id(child_id("arc"))
                        .test_support()
                        .absolute()
                        .top_1()
                        .left_1()
                        .opacity(ring)
                        .text_color(look.percent)
                        .child(
                            Progress::new(child_id("ring"))
                                .circular()
                                .value(percent)
                                .accessibility_label("Upload progress"),
                        ),
                )
                .when(shown > 0.001, |this| {
                    this.child(
                        div()
                            .id(child_id("percent"))
                            .test_support()
                            .absolute()
                            .top_1()
                            .right_1()
                            .opacity(shown)
                            .text_color(look.percent)
                            .child(format!("{percent:.0}%")),
                    )
                })
            })
            .children(dismiss)
    }
}

/// The row of tiles above a composer's text: the attachments in order,
/// wrapping onto further lines when they do not fit.
///
/// ```
/// use gpui_cn::{Attachment, AttachmentStrip};
///
/// let _ = AttachmentStrip::new("files")
///     .attachments([Attachment::new("a1", "notes.md")])
///     .on_dismiss(|id, _, _| println!("remove {id}"));
/// ```
///
/// A tile's id is derived from the strip's id and the attachment's own,
/// so the tiles keep their motion when the list changes.
#[derive(IntoElement)]
#[non_exhaustive]
pub struct AttachmentStrip {
    id: ElementId,
    attachments: Vec<Attachment>,
    style: StyleRefinement,
    on_dismiss: Option<DismissHandler>,
}

impl AttachmentStrip {
    /// An empty strip with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            attachments: Vec::new(),
            style: StyleRefinement::default(),
            on_dismiss: None,
        }
    }

    /// The attachments to show, in order.
    pub fn attachments(mut self, attachments: impl IntoIterator<Item = Attachment>) -> Self {
        self.attachments = attachments.into_iter().collect();
        self
    }

    /// The attachments the strip shows.
    pub fn items(&self) -> &[Attachment] {
        &self.attachments
    }

    /// Called with an attachment's id when its dismiss button is pressed.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl Styled for AttachmentStrip {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AttachmentStrip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let gap = cx.theme().base.spacing.sm;
        let strip_id = self.id.clone();
        let on_dismiss = self.on_dismiss;
        div()
            .id(self.id)
            .test_support()
            .flex()
            .flex_wrap()
            .gap(gap)
            .refine_style(&self.style)
            .children(self.attachments.into_iter().map(move |attachment| {
                let id = ElementId::NamedChild(strip_id.clone().into(), attachment.id.clone());
                let tile = AttachmentTile::new(id, attachment);
                match &on_dismiss {
                    Some(handler) => {
                        let handler = handler.clone();
                        tile.on_dismiss(move |id, window, cx| handler(id, window, cx))
                    }
                    None => tile,
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_choose_the_kind_without_regard_to_case() {
        assert_eq!(
            AttachmentKind::of_file_name("shot.PNG"),
            AttachmentKind::Image
        );
        assert_eq!(
            AttachmentKind::of_file_name("notes.md"),
            AttachmentKind::Document
        );
        assert_eq!(
            AttachmentKind::of_file_name("q3.xlsx"),
            AttachmentKind::Spreadsheet
        );
        assert_eq!(
            AttachmentKind::of_file_name("deck.pptx"),
            AttachmentKind::Presentation
        );
        assert_eq!(
            AttachmentKind::of_file_name("main.rs"),
            AttachmentKind::Code
        );
        assert_eq!(
            AttachmentKind::of_file_name("clip.mov"),
            AttachmentKind::Video
        );
        assert_eq!(
            AttachmentKind::of_file_name("blob.xyz"),
            AttachmentKind::Other
        );
        assert_eq!(
            AttachmentKind::of_file_name("README"),
            AttachmentKind::Other
        );
    }

    #[test]
    fn every_kind_has_one_row_and_the_table_ends_with_other() {
        for row in &KINDS {
            assert_eq!(
                KINDS.iter().filter(|other| other.kind == row.kind).count(),
                1
            );
        }
        assert_eq!(KINDS[KINDS.len() - 1].kind, AttachmentKind::Other);
        assert_eq!(AttachmentKind::Spreadsheet.label(), "Spreadsheet");
    }

    #[test]
    fn progress_decides_the_status() {
        let file = Attachment::new("a", "a.txt");
        assert_eq!(file.status(), AttachmentStatus::Ready);
        assert_eq!(file.clone().progress(0.).status(), AttachmentStatus::Queued);
        assert_eq!(
            file.clone().progress(55.).status(),
            AttachmentStatus::Uploading(55.)
        );
        assert_eq!(
            file.clone().progress(180.).progress_percent(),
            Some(100.),
            "clamped"
        );
        let mut file = file.progress(40.);
        file.set_progress(None);
        assert_eq!(file.status(), AttachmentStatus::Ready);
    }

    #[test]
    fn a_progress_that_is_not_a_number_is_queued_and_100_is_still_uploading() {
        let file = Attachment::new("a", "a.txt");
        for bad in [f32::NAN, f32::NEG_INFINITY, -5.] {
            assert_eq!(
                file.clone().progress(bad).status(),
                AttachmentStatus::Queued
            );
        }
        assert_eq!(
            file.clone().progress(f32::INFINITY).status(),
            AttachmentStatus::Uploading(100.)
        );
        let mut file = file;
        file.set_progress(Some(f32::NAN));
        assert_eq!(file.status(), AttachmentStatus::Queued);
        file.set_progress(Some(100.));
        assert_eq!(
            file.status(),
            AttachmentStatus::Uploading(100.),
            "ready only when None"
        );
    }

    #[test]
    fn an_image_source_makes_an_image_attachment() {
        let image = Attachment::new("a", "a.bin").image(ImageSource::from("x.png"));
        assert_eq!(image.kind_of(), AttachmentKind::Image);
    }
}
