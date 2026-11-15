// Sandhill adaptation (2026-11-06): construct GPUI Kit 0.9's
// non-exhaustive theme tokens through defaults, preserving upstream values.
use std::ops::Range;

use gpui_kit::{
    FontWeight, Hsla, Pixels, SharedString, Size,
    base::{
        ColorTokens, RadiusTokens, SemanticThemeTokens, ShadowTokens, TextStyleToken,
        ThemeAppearance, TypographyTokens,
    },
    px,
};

use super::{ThemeConfig, color::mix};

/// The base font size the typography scale is authored at.
const BASE_FONT_SIZE: f32 = 16.;

/// Everything a component reads: base's semantic tokens plus the few
/// roles gpui-cn components need that base does not define. Derived from a
/// [`ThemeConfig`] when the theme resolves. It is never edited by hand
/// except through [`Theme::update`](super::Theme::update).
///
/// Roles are added here only when a component reads them. The skill
/// color stays on the config until then.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ThemeTokens {
    /// Light or dark.
    pub appearance: ThemeAppearance,
    /// Whether a finger is the pointer. Components show no hover state
    /// and keep their row actions visible when it is.
    pub touch: bool,
    /// Colors, radius, spacing, typography, and shadow shared with base.
    pub base: SemanticThemeTokens,
    /// The persistent selected surface: a sidebar row, a segmented choice,
    /// a pressed toggle. Stronger than the hover surface (`accent`).
    pub selected: Hsla,
    /// Link text. The accent, lifted toward the ink on a dark surface so
    /// it reads as text. The focus ring (`ring`) keeps the accent itself.
    pub link: Hsla,
    /// The tooltip pill: a step above the window surface on dark, the ink
    /// on light.
    pub tooltip: Hsla,
    /// Text on a tooltip.
    pub tooltip_foreground: Hsla,
    /// The dim over the content behind a sheet, such as the sidebar on a
    /// phone: black at half opacity in both appearances, shadcn's
    /// `bg-black/50` sheet overlay.
    pub scrim: Hsla,
    /// The dim behind a dialog: black at 25% alpha in both appearances.
    /// Measured from the reference app on dark: the backdrop is #121212
    /// over the #181818 window, which is 25% black. The light value is
    /// derived from the same alpha and was not sampled.
    pub dialog_overlay: Hsla,
    /// The sidebar surface: a step above the window on dark, a hair below
    /// it on light, so the rail reads as a different plane.
    pub sidebar: Hsla,
    /// The hairline between the sidebar and the content beside it.
    pub sidebar_border: Hsla,
    /// The hover surface of a row on the sidebar: a step above the
    /// sidebar, short of the selected row, so a hovered row beside the
    /// selected one reads as two rows. The reference app paints both the
    /// same.
    pub sidebar_accent: Hsla,
    /// The selected row on the sidebar.
    pub sidebar_selected: Hsla,
    /// Group labels and other quiet text on the sidebar.
    pub sidebar_muted_foreground: Hsla,
    /// The text of controls and rows: 13px at the default font size, the
    /// control size of the reference app, between base's `xs` and `sm`.
    pub text_control: TextStyleToken,
    /// A section heading or a sidebar title: 15px medium.
    pub text_heading: TextStyleToken,
    /// A page title: 24px semibold.
    pub text_title: TextStyleToken,
    /// The sizes of controls, rows, and window chrome.
    pub metrics: MetricTokens,
    /// The strength a disabled control keeps: 55%, measured from the
    /// reference app's disabled button fill. A button applies it to its
    /// fill and a card to the whole. The reference app's disabled switch
    /// measures 60%; gpui-cn keeps one strength for every control.
    pub disabled_opacity: f32,
    /// The strength of a terminal's block cursor over the cell it covers:
    /// 55%, chosen so the glyph under the cursor stays
    /// readable without a second color.
    pub terminal_cursor_opacity: f32,
    /// The color of a switch that is on and a radio that is chosen: the
    /// accent, as the reference app paints a switch that is on (#539af8 on
    /// dark). Only the switch and the radio read it, as macOS paints both
    /// in `controlAccentColor`; no other component paints in the accent.
    pub switch_track_on: Hsla,
    /// The track of a switch that is off.
    pub switch_track_off: Hsla,
    /// The thumb of a switch: the lighter of the surface and the ink, so
    /// it is white in both appearances of the built-in themes, as the
    /// reference app paints it.
    pub switch_thumb: Hsla,
    /// Positive status, such as a success tag. The config's `success`.
    pub success: Hsla,
    /// Caution status, such as a warning tag. The config's `warning`.
    pub warning: Hsla,
    /// Neutral information, such as an info tag. The accent, so it is the
    /// theme's own blue.
    pub info: Hsla,
    /// The solid fill of a destructive mark, such as an alert count: the
    /// destructive color stepped toward the darker end of surface and ink
    /// until `solid_foreground` on it reaches 4.5:1, WCAG AA for text.
    pub destructive_solid: Hsla,
    /// The solid fill of a success mark, derived as `destructive_solid`.
    pub success_solid: Hsla,
    /// The solid fill of a warning mark, derived as `destructive_solid`.
    pub warning_solid: Hsla,
    /// The solid fill of an info mark, derived as `destructive_solid`.
    pub info_solid: Hsla,
    /// Text on a solid status fill: the lighter of surface and ink.
    pub solid_foreground: Hsla,
    /// The fill of a destructive status, such as a danger tag: one step
    /// from the surface toward the destructive color, scaled by contrast.
    pub destructive_tint: Hsla,
    /// The fill of a success status: the `destructive_tint` step toward
    /// `success`.
    pub success_tint: Hsla,
    /// The fill of a warning status: the `destructive_tint` step toward
    /// `warning`.
    pub warning_tint: Hsla,
    /// The fill of an info status: the `destructive_tint` step toward
    /// `info`.
    pub info_tint: Hsla,
    /// The track of a progress bar: `primary` at 20%, shadcn's
    /// `bg-primary/20`. The indicator is `primary` itself.
    pub progress_track: Hsla,
    /// The surface of a menu or popover: #2d2d2d on dark, sampled from the
    /// reference app's select menus, a larger step above the window than
    /// a card takes. The window surface on light, as shadcn's `popover`.
    pub popover: Hsla,
    /// Text on a popover: the ink itself. The reference app paints menu
    /// rows white on dark where the window's text is #dfdfdf.
    pub popover_foreground: Hsla,
    /// The hairline around a popover: #444444 on dark, sampled from the
    /// reference app's menu edge. The `border` token on light.
    pub popover_border: Hsla,
    /// The highlighted row of a menu: #3d3d3d on dark, sampled from the
    /// reference app's hovered menu row. On light the same step as a
    /// hovered sidebar row.
    pub popover_accent: Hsla,
    /// A separator inside a popover: #3e3e3e on dark, sampled from the
    /// reference app's menus. The `border` step on light.
    pub popover_separator: Hsla,
    /// A description under a menu row's label: #b5b5b5 on dark, sampled
    /// from the reference app, a step brighter than `muted_foreground`.
    /// The `muted_foreground` step on light.
    pub popover_muted_foreground: Hsla,
    /// The check beside a selected menu row and the chevron on a select
    /// trigger: #cacaca on dark, sampled from the reference app, the ink
    /// a step toward the surface.
    pub select_indicator: Hsla,
    /// The fill of a select trigger: #2a2a2a on dark as sampled from the
    /// reference app on the window surface (it is #343434 on a #232323
    /// card, the same step). The derivation gives #292929, one step off
    /// the sample, and the test asserts the derived value. The window surface on light, as shadcn's
    /// `bg-transparent` trigger.
    pub select_trigger: Hsla,
    /// The hairline around a select trigger: #3b3b3b on dark as sampled
    /// from the reference app. The derivation gives #3a3a3a, one step off
    /// the sample, and the test asserts the derived value. The `input` token on light, as shadcn's
    /// `border-input`.
    pub select_trigger_border: Hsla,
    /// The hairline between two unselected tabs: the muted text at 60%,
    /// which the tab reference app paints #707070 over its #181818 bar.
    pub tab_separator: Hsla,
    /// The fill of a text field: #2c2c2c on dark, sampled from the
    /// reference app's settings textarea at 2x on the #181818 window. The
    /// window surface on light, as shadcn's `bg-transparent` input.
    pub field: Hsla,
    /// The hairline around a text field at rest: #3b3b3b on dark, sampled
    /// from the reference app's select trigger, the same family. The
    /// `input` token on light, as shadcn's `border-input`.
    pub field_border: Hsla,
    /// The hairline around a text field while the caret is inside: #799ec8
    /// on dark, sampled from the reference app's focused textarea at 2x,
    /// the link color most of the way over the fill. One pixel, no spread.
    /// The ring itself on light, as shadcn's `border-ring`.
    pub field_focus_border: Hsla,
    /// The text of a multi-line field: 13px on an 18.5px line, the pitch
    /// measured from the reference app's settings textarea at 2x (37px),
    /// looser than `text_control` so lines of prose read apart.
    pub text_textarea: TextStyleToken,
    /// The slot a lifted pane of a dock leaves and the place it will land:
    /// the accent at 30%, the drop overlay of Ghostty's splits
    /// (`Color.accentColor.opacity(0.3)` in
    /// `macos/Sources/Features/Splits/TerminalSplitTreeView.swift`).
    pub dock_drop: Hsla,
    /// The ellipsis on a pane's grab handle at rest: the text color at
    /// 30%, Ghostty's `.primary.opacity(0.3)` in
    /// `macos/Sources/Ghostty/Surface View/SurfaceGrabHandle.swift`.
    pub dock_handle: Hsla,
    /// The ellipsis on a hovered grab handle: the text color at 80%,
    /// Ghostty's `.primary.opacity(0.8)` in the same file.
    pub dock_handle_hover: Hsla,
    /// The fill over a pane of a split that does not have focus: the
    /// background at 30%, Ghostty's default `unfocused-split-opacity` of 0.7
    /// with `unfocused-split-fill` left to the background
    /// (`src/config/Config.zig`).
    pub dock_unfocused: Hsla,
}

/// The sizes gpui-cn components are built from.
///
/// Control and row sizes are on the rem scale, so they follow the UI font
/// size. Window chrome and layout widths are fixed pixels: the platform
/// draws its window controls at one size, and a sidebar's width is a
/// layout decision, not a text one.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MetricTokens {
    /// Height of an `Xs` control: 20px.
    pub control_xs: Pixels,
    /// Height of an `Sm` control: 24px.
    pub control_sm: Pixels,
    /// Height of a `Default` control: 28px, the common height in the reference app.
    pub control_md: Pixels,
    /// Height of an `Lg` control: 32px, the largest in the reference app.
    pub control_lg: Pixels,
    /// Side padding of an `Xs` control: 6px.
    pub control_padding_xs: Pixels,
    /// Side padding of an `Sm` control: 8px.
    pub control_padding_sm: Pixels,
    /// Side padding of a `Default` control: 10px.
    pub control_padding_md: Pixels,
    /// Side padding of an `Lg` control: 18px.
    pub control_padding_lg: Pixels,
    /// Height of a small list row: 28px.
    pub row_sm: Pixels,
    /// Height of a list row, a tooltip, a sidebar row: 30px.
    pub row: Pixels,
    /// Height of a large row with an avatar or two lines: 48px.
    pub row_lg: Pixels,
    /// Height of a title bar: 38px, measured from the tab reference app.
    /// The sidebar reference app uses 46px.
    pub title_bar: Pixels,
    /// The gap between a title bar's top and the line its content centers
    /// on: 2px, measured from the tab reference app, which keeps its bar
    /// content level with the window controls below the window's rounded
    /// top edge.
    pub title_bar_content_offset: Pixels,
    /// Where the macOS window controls sit: 14px from the top and left,
    /// measured from the tab reference app, which centers them on the
    /// title bar's content line.
    pub window_controls_position: Pixels,
    /// The room the macOS window controls take at a title bar's left edge,
    /// including the gap after them: 88px.
    pub window_controls_inset: Pixels,
    /// The width of one Windows or Linux window control: 46px.
    pub window_control_width: Pixels,
    /// The width a sidebar opens at: 300px.
    pub sidebar_width: Pixels,
    /// The widths a sidebar can be dragged to: 220px to 480px.
    pub sidebar_width_range: Range<Pixels>,
    /// The width of a sidebar collapsed to icons: 48px.
    pub icon_sidebar_width: Pixels,
    /// The window width under which a sidebar floats over the content as
    /// a sheet instead of sitting beside it: 768px, shadcn's mobile
    /// breakpoint.
    pub sidebar_sheet_breakpoint: Pixels,
    /// The width of a sidebar shown as a sheet: 288px, shadcn's
    /// `SIDEBAR_WIDTH_MOBILE` of 18rem.
    pub sidebar_sheet_width: Pixels,
    /// The pointer target of a resize handle, centered on the edge: 8px.
    pub resize_handle: Pixels,
    /// The spread of the keyboard focus ring outside a control: 3px.
    pub focus_ring: Pixels,
    /// The height of a single-line text field: 32px, measured from the
    /// reference app's search and settings text fields at 2x, a step
    /// taller than its 28px buttons and select triggers, so `control_lg`.
    /// On touch it is `control_md`, the 44pt hit target.
    pub field_height: Pixels,
    /// The side padding of a single-line text field: 12px, measured from
    /// the reference app's search field at 2x. A multi-line field keeps
    /// `control_padding_md`, the 10px its settings textarea measures.
    pub field_padding_x: Pixels,
    /// The padding above and below the lines of a multi-line field: 8px,
    /// measured from the reference app's settings textarea at 2x, the
    /// same step as `control_padding_sm`.
    pub field_padding_y: Pixels,
    /// The width of a scrollbar's thumb at rest: 7px, the knob of a macOS
    /// overlay scroller, which the reference app's web view shows.
    pub scrollbar_thumb: Pixels,
    /// The width of the thumb while the pointer is on the bar or drags
    /// it: 11px, the expanded knob of a macOS overlay scroller.
    pub scrollbar_thumb_active: Pixels,
    /// The gap between the thumb and the edge of the scroll region: 2px.
    pub scrollbar_inset: Pixels,
    /// The width of a switch's track: 32px, measured from the reference
    /// app at 2x. On touch it is the 51px of a UISwitch.
    pub switch_track_width: Pixels,
    /// The height of a switch's track: 20px from the reference app, 31px
    /// of a UISwitch on touch.
    pub switch_track_height: Pixels,
    /// The diameter of a switch's thumb: 16px from the reference app, 27px
    /// of a UISwitch on touch.
    pub switch_thumb_size: Pixels,
    /// The gap between the thumb and the track's edge: 2px on both. On
    /// touch it is fixed with the track, so the thumb always fits.
    pub switch_thumb_inset: Pixels,
    /// The height of a slider's track: 4px, the thin rail of a macOS slider.
    pub slider_track: Pixels,
    /// The diameter of a slider's thumb: 16px, shadcn's `size-4`.
    pub slider_thumb: Pixels,
    /// The diameter of a tick on a slider with stops: 4px, as thick as the
    /// track, so a dot on the rail reads as a stop and not a bump.
    pub slider_tick: Pixels,
    /// The diameter of a radio's ring: 16px, shadcn's `size-4`.
    pub radio_size: Pixels,
    /// The diameter of a radio's dot: 6px, three quarters of shadcn's
    /// `size-2`, which sits better in the filled disc this radio paints.
    pub radio_dot: Pixels,
    /// The diameter of a progress ring: 16px, the context meter of the
    /// composer's status tab. Not measured from the reference app.
    pub ring_size: Pixels,
    /// The stroke of a progress ring: 2px, the same source.
    pub ring_stroke: Pixels,
    /// The side of an attachment tile: 56px (composer design spec; not
    /// measured from the reference app).
    pub attachment_tile: Pixels,
    /// The width of the model picker's panel: 361px, measured from the
    /// reference app's picker at 2x (722px from edge to edge).
    pub model_picker_width: Pixels,
    /// The height of the model picker's panel: 346px, the same screenshot
    /// (693px). The panel never changes height: a short result leaves room,
    /// an empty one centers its message in the list, and a long one scrolls
    /// inside it.
    pub model_picker_height: Pixels,
    /// The width of the provider rail at the picker's left: 44px, the same
    /// screenshot (89px).
    pub model_picker_rail: Pixels,
    /// The height of a two-line model row: 47px, the same screenshot, where
    /// rows are 94px apart.
    pub model_row: Pixels,
    /// The height of the picker's search row, from the panel's top edge to
    /// the underline: 42px, the same screenshot (83px).
    pub model_header: Pixels,
    /// The height of a composer's status tab: 38px, measured from the reference
    /// composer screenshots at 2x (76px from the top of the tab to the top
    /// of the card).
    pub status_tab_height: Pixels,
    /// The gap between the items of a status tab: 28px, the same
    /// screenshots (56px between two items).
    pub status_tab_gap: Pixels,
    /// The gap between a status item's icon and its text: 8px, the same
    /// screenshots (15px, rounded up to the step).
    pub status_label_gap: Pixels,
    /// How far the status tab is inset from each side of the card: 14px,
    /// the same screenshots (27px).
    pub status_tab_inset: Pixels,
    /// The gap between a select trigger and its menu: 2px, measured from
    /// the reference app at 2x.
    pub menu_gap: Pixels,
    /// The narrowest a menu opens: 128px, shadcn's `min-w-[8rem]`. A menu
    /// is never narrower than its trigger either.
    pub menu_min_width: Pixels,
    /// The narrowest a menu with a search field opens: 240px, the width
    /// of the reference app's language list, so the field has room for
    /// its placeholder.
    pub menu_search_min_width: Pixels,
    /// The widest a menu grows to fit its rows: 360px, the width the
    /// reference app wraps a row's description at.
    pub menu_max_width: Pixels,
    /// The tallest a menu grows before it scrolls: 390px, the reference
    /// app's language list.
    pub menu_max_height: Pixels,
    /// The width of a dialog: 480px. Measured from the reference app's
    /// folder dialog at 2.2x, where the 32px field is 71px and the dialog
    /// is 1040px wide (473px), rounded to shadcn's usual step.
    pub dialog_width: Pixels,
    /// The padding around a dialog's content: 20px. Measured from the same
    /// screenshot (about 18.5px), rounded to the 20px step.
    pub dialog_padding: Pixels,
    /// The width of a command dialog: 512px, shadcn's `max-w-lg` (32rem).
    /// This value comes from shadcn, not from the reference app.
    pub command_dialog_width: Pixels,
    /// How far a command dialog sits below the top of the window: 120px,
    /// so it rests in the upper third as Spotlight does, and does not move
    /// when its content changes height. This is gpui-cn's own choice: only
    /// the width comes from shadcn, and neither comes from the reference
    /// app.
    pub command_dialog_top: Pixels,
    /// The height of the folder picker's list: 6.5 rows, so the last row
    /// peeks over the edge and shows that the list scrolls. Measured from
    /// the reference app's folder dialog: the list is 181px tall at 2.2x
    /// beside 28px rows.
    pub folder_list_height: Pixels,
    /// The gap between the top of a tab item and its surface: 4px,
    /// measured from the tab reference app. On touch the tab fills its bar.
    pub tab_surface_inset: Pixels,
    /// The width a tab prefers: 160px, measured from the tab reference app.
    pub tab_width: Pixels,
    /// The width a tab shrinks to before the strip scrolls: 112px, measured
    /// from the tab reference app.
    pub tab_min_width: Pixels,
    /// The curve at each bottom corner of the selected tab, where it merges
    /// into the content below: 7px, measured from the tab reference app.
    pub tab_shoulder: Pixels,
    /// The radius of a tab surface's top corners: 7px, measured from the tab
    /// reference app.
    pub tab_radius: Pixels,
    /// The gutter at an unselected tab's right edge that holds the separator:
    /// 7px, measured from the tab reference app.
    pub tab_separator_gutter: Pixels,
    /// The inset of the 1px separator from the tab item's top and bottom:
    /// 11px, measured from the tab reference app.
    pub tab_separator_inset: Pixels,
    /// The box of a tab's icon: 13px, measured from the tab reference app.
    pub tab_icon: Pixels,
    /// The gap between a tab's icon and its label: 6px, measured from the
    /// tab reference app.
    pub tab_content_gap: Pixels,
    /// The slot that holds the new-tab and scroll controls of a tab strip:
    /// the height of an `Sm` control, which the controls are. 24px at the
    /// default font size, measured from the tab reference app.
    pub tab_control: Pixels,
    /// The box of a tab's close control: the height of an `Xs` control,
    /// which the control is. 20px at the default font size, measured from
    /// the tab reference app.
    pub tab_close: Pixels,
    /// The height of a tag: 20px, shadcn's badge `h-5`.
    pub tag_height: Pixels,
    /// The size of an icon in a tag: 12px, shadcn's badge `size-3`.
    pub tag_icon: Pixels,
    /// The height and least width of a badge count: 16px, gpui-kit's
    /// medium badge.
    pub badge_count: Pixels,
    /// The text of a badge count: 10px, gpui-kit's badge text.
    pub badge_count_text: Pixels,
    /// How far a badge count sits past the corner of its element: 4px,
    /// so it overlaps the corner as gpui-kit's does.
    pub badge_count_offset: Pixels,
    /// The diameter of a badge dot: 6px, gpui-kit's dot.
    pub badge_dot: Pixels,
    /// The circle of a badge icon: 16px, gpui-kit's medium badge.
    pub badge_icon: Pixels,
    /// The diameter of a `Sm` avatar: 24px, shadcn's `size-6`.
    pub avatar_sm: Pixels,
    /// The diameter of a `Default` avatar: 32px, shadcn's `size-8`.
    pub avatar_md: Pixels,
    /// The diameter of a `Lg` avatar: 40px, shadcn's `size-10`.
    pub avatar_lg: Pixels,
    /// The initials size in a `Sm` avatar: 10px, so two capitals fit the
    /// 24px circle and stay clear of an overlapping neighbor.
    pub avatar_initials_sm: Pixels,
    /// The initials size in a `Default` avatar: 12px, shadcn's `text-xs`.
    pub avatar_initials_md: Pixels,
    /// The initials size in a `Lg` avatar: 14px, shadcn's `text-sm`.
    pub avatar_initials_lg: Pixels,
    /// How far each avatar in a group covers the one before it: 8px,
    /// shadcn's `-space-x-2`.
    pub avatar_group_overlap: Pixels,
    /// The ring that parts overlapped avatars: 2px, shadcn's `ring-2`.
    pub avatar_group_ring: Pixels,
    /// The room around a terminal's grid: 2px, Ghostty's default
    /// `window-padding-x` and `window-padding-y`, from
    /// `src/config/Config.zig` and `ghostty +show-config --default`. The
    /// room is painted in the terminal's background, as Ghostty's default
    /// `window-padding-color = background` does. Fixed, because the grid
    /// follows the code font size, not the UI font size.
    pub terminal_padding: Pixels,
    /// The width of a bar cursor and the height of an underline cursor in
    /// a terminal: 2px, chosen to stay visible at every code font size.
    pub terminal_cursor_thickness: Pixels,
    /// The width of the grab handle at the top center of a dock pane:
    /// 80px, `handleSize` in Ghostty's
    /// `macos/Sources/Ghostty/Surface View/SurfaceGrabHandle.swift`.
    pub dock_handle_width: Pixels,
    /// The height of the grab handle: 12px, Ghostty's `handleSize`. On
    /// touch it is the 44pt hit target.
    pub dock_handle_height: Pixels,
    /// The ellipsis in the grab handle: 12px, the handle's own height.
    pub dock_handle_icon: Pixels,
    /// The share of a pane's height, from its top edge, in which the
    /// pointer reveals the grab handle: 20%, Ghostty's
    /// `hoverHeightFactor`.
    pub dock_handle_reveal: f32,
    /// How far the pointer moves from a press on the grab handle before
    /// the pane lifts: 6px, the drag threshold of Trellis (`Tv=6` in its
    /// bundle).
    pub dock_drag_threshold: Pixels,
    /// The floating card a lifted pane becomes, clamped to the pane:
    /// 380x260, the compact drag size of Trellis (`ou={w:380,h:260}` in
    /// its bundle).
    pub dock_card: Size<Pixels>,
}

impl MetricTokens {
    /// The metrics at `ui_font_size`. The rem-scale sizes are scaled from
    /// the 16px defaults. With `touch`, controls and rows grow to the 44pt
    /// minimum hit target of Apple's Human Interface Guidelines, the
    /// default control a step past it, and the title bar takes the 44pt
    /// height of an iOS navigation bar.
    fn derive(ui_font_size: Pixels, touch: bool) -> Self {
        let factor = f32::from(ui_font_size) / BASE_FONT_SIZE;
        let scaled = |value: f32| px(value * factor);
        let (control_xs, control_sm, control_md, control_lg) = if touch {
            (px(36.), px(40.), px(44.), px(48.))
        } else {
            (scaled(20.), scaled(24.), scaled(28.), scaled(32.))
        };
        let (row_sm, row, row_lg) = if touch {
            (px(40.), px(44.), px(56.))
        } else {
            (scaled(28.), scaled(30.), scaled(48.))
        };
        let title_bar = px(if touch { 44. } else { 38. });
        let control_padding_sm = scaled(8.);
        Self {
            control_xs,
            control_sm,
            control_md,
            control_lg,
            control_padding_xs: scaled(6.),
            control_padding_sm,
            control_padding_md: scaled(if touch { 14. } else { 10. }),
            control_padding_lg: scaled(18.),
            row_sm,
            row,
            row_lg,
            title_bar,
            title_bar_content_offset: px(if touch { 0. } else { 2. }),
            window_controls_position: px(14.),
            window_controls_inset: px(88.),
            window_control_width: px(46.),
            sidebar_width: px(300.),
            sidebar_width_range: px(220.)..px(480.),
            icon_sidebar_width: px(48.),
            sidebar_sheet_breakpoint: px(768.),
            sidebar_sheet_width: px(288.),
            resize_handle: px(8.),
            focus_ring: px(3.),
            field_height: if touch { control_md } else { control_lg },
            field_padding_x: scaled(12.),
            field_padding_y: control_padding_sm,
            scrollbar_thumb: px(7.),
            scrollbar_thumb_active: px(11.),
            scrollbar_inset: px(2.),
            switch_track_width: if touch { px(51.) } else { scaled(32.) },
            switch_track_height: if touch { px(31.) } else { scaled(20.) },
            switch_thumb_size: if touch { px(27.) } else { scaled(16.) },
            switch_thumb_inset: if touch { px(2.) } else { scaled(2.) },
            slider_track: scaled(4.),
            slider_thumb: scaled(16.),
            slider_tick: scaled(4.),
            radio_size: scaled(16.),
            radio_dot: scaled(6.),
            ring_size: scaled(16.),
            ring_stroke: scaled(2.),
            attachment_tile: scaled(56.),
            model_picker_width: scaled(361.),
            model_picker_height: scaled(346.),
            model_picker_rail: scaled(44.),
            model_row: scaled(47.),
            model_header: scaled(42.),
            status_tab_height: scaled(38.),
            status_tab_inset: scaled(14.),
            status_tab_gap: scaled(28.),
            status_label_gap: scaled(8.),
            menu_gap: scaled(2.),
            menu_min_width: scaled(128.),
            menu_search_min_width: scaled(240.),
            menu_max_width: scaled(360.),
            menu_max_height: scaled(390.),
            dialog_width: scaled(480.),
            dialog_padding: scaled(20.),
            command_dialog_width: scaled(512.),
            command_dialog_top: scaled(120.),
            folder_list_height: row_sm * 6.5,
            tab_surface_inset: px(if touch { 0. } else { 4. }),
            tab_width: px(160.),
            tab_min_width: px(112.),
            tab_shoulder: px(7.),
            tab_radius: px(7.),
            tab_separator_gutter: px(7.),
            tab_separator_inset: px(11.),
            tab_icon: px(13.),
            tab_content_gap: px(6.),
            tab_control: control_sm,
            tab_close: control_xs,
            tag_height: scaled(20.),
            tag_icon: scaled(12.),
            badge_count: scaled(16.),
            badge_count_text: scaled(10.),
            badge_count_offset: scaled(4.),
            badge_dot: scaled(6.),
            badge_icon: scaled(16.),
            avatar_sm: scaled(24.),
            avatar_md: scaled(32.),
            avatar_lg: scaled(40.),
            avatar_initials_sm: scaled(10.),
            avatar_initials_md: scaled(12.),
            avatar_initials_lg: scaled(14.),
            avatar_group_overlap: scaled(8.),
            avatar_group_ring: px(2.),
            terminal_padding: px(2.),
            terminal_cursor_thickness: px(2.),
            dock_handle_width: px(80.),
            dock_handle_height: if touch { control_md } else { px(12.) },
            dock_handle_icon: px(12.),
            dock_handle_reveal: 0.2,
            dock_drag_threshold: px(6.),
            dock_card: Size::new(px(380.), px(260.)),
        }
    }
}

/// Sizes that do not come from the config but still shape the tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Metrics {
    pub radius: Pixels,
    pub ui_font_size: Pixels,
    pub code_font_size: Pixels,
    /// Whether a finger is the pointer.
    pub touch: bool,
}

impl ThemeTokens {
    /// Derives the full token set from a config.
    ///
    /// `contrast` is read as a factor around 50: at 50 the ratios match the
    /// shadcn neutral palette. The built-in 45 (light) and 60 (dark) sit
    /// close to it. A dark surface gets a slightly lifted card and popover because a
    /// hairline alone does not separate them from the window.
    pub(super) fn derive(
        config: &ThemeConfig,
        appearance: ThemeAppearance,
        metrics: Metrics,
    ) -> Self {
        let dark = appearance == ThemeAppearance::Dark;
        let c = f32::from(config.contrast) / 50.;
        let surface = config.surface;
        let ink = config.ink;
        // OKLab steps vanish near black: 7% from #000 toward white is
        // #010101. The steps start from a surface at least as light as the
        // reference app's #181818, so a black surface keeps visible
        // separators and fills. The surface itself stays the background.
        let floor = super::color::lightness(super::color::hex("#181818"));
        let base = {
            let (from, to) = (
                super::color::lightness(surface),
                super::color::lightness(ink),
            );
            if dark && from < floor && to > floor {
                mix(surface, ink, (floor - from) / (to - from))
            } else {
                surface
            }
        };
        let toward_ink = |amount: f32| mix(base, ink, amount);

        // Every fill is one OKLab step from the surface toward the ink,
        // scaled by contrast. The constants reproduce the values sampled
        // from the reference app at its own contrast settings (45
        // light, 60 dark). Separators and control borders keep one
        // distance in both appearances. On a dark surface the text roles
        // step a little toward the surface, as the reference app does
        // (its default text is #dfdfdf on #181818), and the elevated
        // surfaces lift because a hairline alone does not separate them.
        let soft = toward_ink(0.044 * c);
        let selected = toward_ink(if dark { 0.0867 } else { 0.10 } * c);
        let border = toward_ink(0.07);
        let input = toward_ink(0.10);
        let foreground = if dark { mix(ink, surface, 0.12) } else { ink };
        let muted_foreground = mix(ink, surface, if dark { 0.412 } else { 0.38 });
        let card = if dark {
            toward_ink(0.0475 * c)
        } else {
            surface
        };
        // The reference app paints links #91c2fa on dark, paler than its
        // #539af8 accent, and the accent itself on light.
        let link = if dark {
            mix(config.accent, ink, 0.35)
        } else {
            config.accent
        };
        // The tooltip in the reference app is #1b1b1b with white text on the dark
        // surface: a step so small it reads as the surface itself.
        let (tooltip, tooltip_foreground) = if dark {
            (toward_ink(0.016), ink)
        } else {
            (ink, surface)
        };
        // The reference app draws its primary button a step short of the ink,
        // with the label a step short of the surface (#dfdfdf on #2d2d2d).
        let primary = mix(ink, surface, 0.12);
        let primary_foreground = mix(surface, ink, 0.11);
        // The sidebar in the reference app is #222222 beside #181818 on dark, the same
        // step as the soft fill, and #fdfdfd beside #ffffff on light. Its
        // rows and hairline step from the sidebar, not from the window, so
        // they stay visible on the lifted surface.
        let sidebar = if dark { soft } else { toward_ink(0.008) };
        let sidebar_step = |amount: f32| mix(sidebar, ink, amount);
        let sidebar_accent = sidebar_step(if dark { 0.055 } else { 0.025 });
        let sidebar_selected = sidebar_step(if dark { 0.095 } else { 0.05 });
        let sidebar_border = sidebar_step(if dark { 0.10 } else { 0.07 });
        let sidebar_muted_foreground = mix(ink, sidebar, if dark { 0.59 } else { 0.665 });
        let destructive_foreground = readable_on(config.semantic.destructive, surface, ink);
        // The reference app's switch: the accent when on, and when off a
        // gray a few steps from the surface. The thumb is white on both.
        let switch_track_on = config.accent;
        let switch_track_off = toward_ink(0.20);
        let switch_thumb = lighter_of(surface, ink);
        let progress_track = primary.opacity(0.2);
        // A status fill is a step toward its color, as `selected` is a step
        // toward the ink. The constants give the 20% (dark) and 10%
        // (light) tints of the destructive button at the built-in
        // contrasts, so a tag and a button of one color read as one.
        let solid_foreground = lighter_of(surface, ink);
        let darker = if solid_foreground == surface {
            ink
        } else {
            surface
        };
        let solid = |color: Hsla| {
            (0..=20)
                .map(|step| mix(color, darker, step as f32 * 0.04))
                .find(|fill| super::color::contrast_ratio(*fill, solid_foreground) >= 4.5)
                .unwrap_or(darker)
        };
        let tint = |color: Hsla| mix(surface, color, if dark { 0.1667 } else { 0.111 } * c);
        let popover = if dark { toward_ink(0.109) } else { surface };
        let popover_step = |amount: f32| mix(popover, ink, amount);
        let popover_border = if dark { popover_step(0.13) } else { border };
        let popover_accent = popover_step(if dark { 0.09 } else { 0.033 });
        let popover_separator = if dark { popover_step(0.095) } else { border };
        let popover_muted_foreground = mix(ink, popover, if dark { 0.32 } else { 0.38 });
        let select_indicator = mix(ink, popover, if dark { 0.227 } else { 0.38 });
        let select_trigger = if dark { toward_ink(0.09) } else { surface };
        let select_trigger_border = if dark { toward_ink(0.178) } else { input };
        let tab_separator = muted_foreground.opacity(0.6);
        let field = if dark { toward_ink(0.104) } else { surface };
        let field_border = if dark { toward_ink(0.181) } else { input };
        let field_focus_border = if dark {
            mix(field, link, 0.78)
        } else {
            config.accent
        };

        let mut base = SemanticThemeTokens::default();
        let colors = &mut base.colors;
        colors.background = surface;
        colors.foreground = foreground;
        colors.surface = card;
        colors.surface_foreground = foreground;
        colors.primary = primary;
        colors.primary_foreground = primary_foreground;
        colors.secondary = soft;
        colors.secondary_foreground = foreground;
        colors.muted = soft;
        colors.muted_foreground = muted_foreground;
        colors.accent = soft;
        colors.accent_foreground = foreground;
        colors.destructive = config.semantic.destructive;
        colors.destructive_foreground = destructive_foreground;
        colors.border = border;
        colors.input = input;
        colors.ring = config.accent;
        colors.selection = config.accent.alpha(0.3);

        base.typography = typography(config, metrics, base.typography);
        let factor = f32::from(metrics.ui_font_size) / BASE_FONT_SIZE;
        // On touch the text takes the sizes of Apple's Human Interface
        // Guidelines: body 17pt for controls and rows, headline 17pt
        // semibold, and title 1 at 28pt.
        let mut text_control = TextStyleToken::default();
        text_control.size = px(if metrics.touch { 17. } else { 13. } * factor);
        text_control.line_height = px(if metrics.touch { 22. } else { 16. } * factor);
        text_control.weight = FontWeight::NORMAL;
        let mut text_textarea = text_control;
        text_textarea.line_height = px(if metrics.touch { 22. } else { 18.5 } * factor);
        let mut text_heading = text_control;
        text_heading.size = px(if metrics.touch { 17. } else { 15. } * factor);
        text_heading.line_height = px(22. * factor);
        text_heading.weight = if metrics.touch {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        };
        let mut text_title = text_control;
        text_title.size = px(if metrics.touch { 28. } else { 24. } * factor);
        text_title.line_height = px(if metrics.touch { 34. } else { 32. } * factor);
        text_title.weight = FontWeight::SEMIBOLD;
        let shadow = ShadowTokens::elevations(if dark {
            gpui_kit::black().alpha(0.4)
        } else {
            ink.alpha(0.1)
        });
        base.radius = radius_scale(metrics.radius);
        base.shadow = shadow;

        Self {
            appearance,
            touch: metrics.touch,
            base,
            selected,
            link,
            tooltip,
            tooltip_foreground,
            scrim: gpui_kit::hsla(0., 0., 0., 0.5),
            dialog_overlay: gpui_kit::hsla(0., 0., 0., 0.25),
            sidebar,
            sidebar_border,
            sidebar_accent,
            sidebar_selected,
            sidebar_muted_foreground,
            text_control,
            text_heading,
            text_title,
            metrics: MetricTokens::derive(metrics.ui_font_size, metrics.touch),
            disabled_opacity: 0.55,
            terminal_cursor_opacity: 0.55,
            switch_track_on,
            switch_track_off,
            switch_thumb,
            success: config.semantic.success,
            warning: config.semantic.warning,
            info: config.accent,
            destructive_solid: solid(config.semantic.destructive),
            success_solid: solid(config.semantic.success),
            warning_solid: solid(config.semantic.warning),
            info_solid: solid(config.accent),
            solid_foreground,
            destructive_tint: tint(config.semantic.destructive),
            success_tint: tint(config.semantic.success),
            warning_tint: tint(config.semantic.warning),
            info_tint: tint(config.accent),
            progress_track,
            popover,
            popover_foreground: ink,
            popover_border,
            popover_accent,
            popover_separator,
            popover_muted_foreground,
            select_indicator,
            select_trigger,
            select_trigger_border,
            tab_separator,
            field,
            field_border,
            field_focus_border,
            text_textarea,
            dock_drop: config.accent.opacity(0.3),
            dock_handle: foreground.opacity(0.3),
            dock_handle_hover: foreground.opacity(0.8),
            dock_unfocused: surface.opacity(0.3),
        }
    }

    /// Whether the tokens are the dark set.
    pub fn is_dark(&self) -> bool {
        self.appearance == ThemeAppearance::Dark
    }

    /// The color tokens shared with base.
    pub fn colors(&self) -> &ColorTokens {
        &self.base.colors
    }

    /// The window background.
    pub fn background(&self) -> Hsla {
        self.base.colors.background
    }

    /// Default text.
    pub fn foreground(&self) -> Hsla {
        self.base.colors.foreground
    }

    /// High-emphasis actions: the default button.
    pub fn primary(&self) -> Hsla {
        self.base.colors.primary
    }

    /// Text on a primary surface.
    pub fn primary_foreground(&self) -> Hsla {
        self.base.colors.primary_foreground
    }

    /// Lower-emphasis filled actions and supporting surfaces.
    pub fn secondary(&self) -> Hsla {
        self.base.colors.secondary
    }

    /// Text on a secondary surface.
    pub fn secondary_foreground(&self) -> Hsla {
        self.base.colors.secondary_foreground
    }

    /// Subtle surfaces.
    pub fn muted(&self) -> Hsla {
        self.base.colors.muted
    }

    /// Descriptions, placeholders, helper text.
    pub fn muted_foreground(&self) -> Hsla {
        self.base.colors.muted_foreground
    }

    /// Hover, focus, and active surfaces.
    pub fn accent(&self) -> Hsla {
        self.base.colors.accent
    }

    /// Text on an accent surface.
    pub fn accent_foreground(&self) -> Hsla {
        self.base.colors.accent_foreground
    }

    /// Destructive actions and error emphasis.
    pub fn destructive(&self) -> Hsla {
        self.base.colors.destructive
    }

    /// Text on a destructive surface.
    pub fn destructive_foreground(&self) -> Hsla {
        self.base.colors.destructive_foreground
    }

    /// Default borders and separators.
    pub fn border(&self) -> Hsla {
        self.base.colors.border
    }

    /// Form control borders.
    pub fn input(&self) -> Hsla {
        self.base.colors.input
    }

    /// Focus rings.
    pub fn ring(&self) -> Hsla {
        self.base.colors.ring
    }

    /// The keyboard focus ring a control paints outside its box: the ring
    /// color at half strength, shadcn's `ring/50`.
    pub fn focus_ring(&self) -> Hsla {
        self.ring().opacity(0.5)
    }

    /// Selected text background.
    pub fn selection(&self) -> Hsla {
        self.base.colors.selection
    }

    /// Interface font family.
    pub fn font_family(&self) -> &SharedString {
        &self.base.typography.sans
    }

    /// Code font family.
    pub fn mono_font_family(&self) -> &SharedString {
        &self.base.typography.mono
    }

    /// Radius for small controls: badges, checkboxes, menu items.
    pub fn radius_sm(&self) -> Pixels {
        self.base.radius.sm
    }

    /// Radius for buttons, inputs, and menu rows.
    pub fn radius_md(&self) -> Pixels {
        self.base.radius.md
    }

    /// The base radius: cards, popovers, dialogs.
    pub fn radius_lg(&self) -> Pixels {
        self.base.radius.lg
    }

    /// Radius for large surfaces: sheets and window panels.
    pub fn radius_xl(&self) -> Pixels {
        self.base.radius.xl
    }

    /// As round as the shape allows: circles and pills.
    pub fn radius_full(&self) -> Pixels {
        self.base.radius.full
    }
}

/// The shadcn radius scale from one base value.
fn radius_scale(radius: Pixels) -> RadiusTokens {
    let mut tokens = RadiusTokens::default();
    tokens.none = px(0.);
    tokens.sm = radius * 0.6;
    tokens.md = radius * 0.8;
    tokens.lg = radius;
    tokens.xl = radius * 1.4;
    tokens.full = px(9999.);
    tokens
}

/// Base's typography scale, moved to the configured sizes and families.
fn typography(
    config: &ThemeConfig,
    metrics: Metrics,
    mut tokens: TypographyTokens,
) -> TypographyTokens {
    let factor = f32::from(metrics.ui_font_size) / BASE_FONT_SIZE;
    let scale = |mut token: TextStyleToken| {
        token.size *= factor;
        token.line_height *= factor;
        token
    };
    let code_factor = f32::from(metrics.code_font_size) / f32::from(tokens.mono_md.size);
    if let Some(font) = &config.fonts.ui {
        tokens.sans = font.clone();
    }
    if let Some(font) = &config.fonts.code {
        tokens.mono = font.clone();
    }
    tokens.xs = scale(tokens.xs);
    tokens.sm = scale(tokens.sm);
    tokens.md = scale(tokens.md);
    tokens.lg = scale(tokens.lg);
    tokens.xl = scale(tokens.xl);
    tokens.mono_md.size = metrics.code_font_size;
    tokens.mono_md.line_height *= code_factor;
    tokens
}

/// The lighter of two colors.
fn lighter_of(a: Hsla, b: Hsla) -> Hsla {
    use super::color::lightness;
    if lightness(a) >= lightness(b) { a } else { b }
}

/// Picks `surface` or `ink` as text on a colored `background`.
///
/// Light text on a saturated mid-tone (the accent blue, a shadcn red) is the
/// convention even where a contrast formula would pick dark text, so the
/// lighter of the two is used unless the background is itself light.
fn readable_on(background: Hsla, surface: Hsla, ink: Hsla) -> Hsla {
    use super::color::lightness;
    const LIGHT_BACKGROUND: f32 = 0.72;
    let lighter = lighter_of(surface, ink);
    let darker = if lighter == surface { ink } else { surface };
    if lightness(background) > LIGHT_BACKGROUND {
        darker
    } else {
        lighter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::color::{hex, lightness, to_hex};

    fn metrics() -> Metrics {
        Metrics {
            radius: px(10.),
            ui_font_size: px(16.),
            code_font_size: px(13.),
            touch: false,
        }
    }

    fn light() -> ThemeTokens {
        ThemeTokens::derive(&ThemeConfig::light(), ThemeAppearance::Light, metrics())
    }

    #[test]
    fn a_black_surface_keeps_visible_separators() {
        let mut config = ThemeConfig::dark();
        config.surface = super::super::color::hex("#000000");
        let tokens = ThemeTokens::derive(&config, ThemeAppearance::Dark, metrics());
        let lightness = super::super::color::lightness;
        assert_eq!(tokens.background(), config.surface);
        assert!(
            lightness(tokens.border()) > lightness(super::super::color::hex("#181818")),
            "border {}",
            super::super::color::to_hex(tokens.border())
        );
    }

    #[test]
    fn touch_grows_controls_to_the_hig_hit_target_and_text_to_body_size() {
        let touch = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                touch: true,
                ..metrics()
            },
        );
        assert!(touch.touch);
        assert_eq!(touch.metrics.control_md, px(44.), "the HIG minimum");
        assert_eq!(touch.metrics.row, px(44.));
        assert_eq!(touch.metrics.control_xs, px(36.));
        assert_eq!(touch.metrics.title_bar, px(44.), "an iOS navigation bar");
        assert_eq!(touch.text_control.size, px(17.), "HIG body");
        assert_eq!(touch.text_title.size, px(28.), "HIG title 1");
        // The chrome that is not touched stays.
        assert_eq!(touch.metrics.sidebar_sheet_width, px(288.));
        // The switch is a UISwitch, fixed like the chrome, so the thumb
        // fits at every font size.
        assert_eq!(touch.metrics.switch_track_width, px(51.));
        assert_eq!(touch.metrics.switch_track_height, px(31.));
        assert_eq!(touch.metrics.switch_thumb_size, px(27.));
        assert_eq!(touch.metrics.switch_thumb_inset, px(2.));
        let large_touch = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                touch: true,
                ui_font_size: px(20.),
                ..metrics()
            },
        );
        assert_eq!(large_touch.metrics.switch_thumb_inset, px(2.));
        let mouse = light();
        assert!(!mouse.touch);
        assert_eq!(mouse.metrics.control_md, px(28.));
        assert_eq!(mouse.metrics.row, px(30.));
        assert_eq!(mouse.text_control.size, px(13.));
    }

    fn dark() -> ThemeTokens {
        ThemeTokens::derive(&ThemeConfig::dark(), ThemeAppearance::Dark, metrics())
    }

    #[test]
    fn surfaces_step_away_from_the_background_in_both_appearances() {
        let light = light();
        let bg = lightness(light.background());
        assert!(lightness(light.secondary()) < bg);
        assert!(lightness(light.border()) < lightness(light.secondary()));
        assert!(lightness(light.input()) < lightness(light.border()));
        assert!(lightness(light.selected) < lightness(light.secondary()));
        assert_eq!(
            light.base.colors.surface,
            light.background(),
            "light cards sit flat"
        );
        assert!(lightness(light.muted_foreground()) > lightness(light.foreground()));

        let dark = dark();
        let bg = lightness(dark.background());
        assert!(lightness(dark.secondary()) > bg);
        assert!(lightness(dark.border()) > bg);
        assert!(lightness(dark.input()) > lightness(dark.border()));
        assert!(
            lightness(dark.base.colors.surface) > bg,
            "dark cards are lifted"
        );
        assert!(lightness(dark.foreground()) < lightness(dark.primary()) + 0.001);
        assert!(lightness(dark.muted_foreground()) < lightness(dark.foreground()));
    }

    #[test]
    fn tokens_reproduce_the_reference_values() {
        // Sampled from the reference app at contrast 45 (light) and
        // 60 (dark).
        let light = light();
        assert_eq!(to_hex(light.secondary()), "#f5f5f5");
        assert_eq!(to_hex(light.selected), "#e8e8e8");
        assert_eq!(to_hex(light.border()), "#ededed");
        assert_eq!(to_hex(light.input()), "#e5e5e6");
        assert_eq!(to_hex(light.muted_foreground()), "#67696b");
        let dark = dark();
        assert_eq!(to_hex(dark.secondary()), "#222222");
        assert_eq!(to_hex(dark.base.colors.surface), "#232323");
        assert_eq!(to_hex(dark.selected), "#2c2c2c");
        assert_eq!(to_hex(dark.border()), "#252525");
        assert_eq!(to_hex(dark.foreground()), "#dfdfdf");
        assert_eq!(to_hex(dark.muted_foreground()), "#969696");
        assert_eq!(to_hex(dark.primary()), "#dfdfdf");
        assert_eq!(to_hex(dark.primary_foreground()), "#2d2d2d");
        assert_eq!(to_hex(dark.tooltip), "#1b1b1b");
        assert_eq!(to_hex(dark.tooltip_foreground), "#ffffff");
        assert_eq!(to_hex(light.tooltip), "#1a1c1f");
        assert_eq!(to_hex(light.foreground()), "#1a1c1f");
    }

    #[test]
    fn sidebar_tokens_reproduce_the_reference_values() {
        // Sampled from the reference app's sidebar and the hairline
        // between it and the content.
        let dark = dark();
        assert_eq!(to_hex(dark.sidebar), "#222222");
        assert_eq!(to_hex(dark.sidebar_selected), "#333333");
        assert_eq!(to_hex(dark.sidebar_accent), "#2c2c2c");
        assert!(lightness(dark.sidebar_accent) > lightness(dark.sidebar));
        assert!(lightness(dark.sidebar_accent) < lightness(dark.sidebar_selected));
        assert_eq!(to_hex(dark.sidebar_border), "#343434");
        assert_eq!(to_hex(dark.sidebar_muted_foreground), "#747474");
        let light = light();
        assert_eq!(to_hex(light.sidebar), "#fdfdfd");
        assert_eq!(to_hex(light.sidebar_selected), "#f0f0f0");
        assert_eq!(to_hex(light.sidebar_accent), "#f6f6f7");
        assert!(lightness(light.sidebar_accent) < lightness(light.sidebar));
        assert!(lightness(light.sidebar_accent) > lightness(light.sidebar_selected));
        assert_eq!(to_hex(light.sidebar_border), "#ebebeb");
        // The sample is antialiased 13px text; one step off is inside its noise.
        assert_eq!(to_hex(light.sidebar_muted_foreground), "#a9aaab");
    }

    #[test]
    fn contrast_scales_the_distance() {
        let mut low = ThemeConfig::light();
        low.contrast = 20;
        let mut high = ThemeConfig::light();
        high.contrast = 80;
        let low = ThemeTokens::derive(&low, ThemeAppearance::Light, metrics());
        let high = ThemeTokens::derive(&high, ThemeAppearance::Light, metrics());
        assert!(lightness(low.secondary()) > lightness(high.secondary()));
        assert!(lightness(low.selected) > lightness(high.selected));
        assert_eq!(
            low.border(),
            high.border(),
            "separators do not move with contrast"
        );
    }

    #[test]
    fn status_tints_step_toward_their_color() {
        let dark = ThemeTokens::derive(&ThemeConfig::dark(), ThemeAppearance::Dark, metrics());
        let light = ThemeTokens::derive(&ThemeConfig::light(), ThemeAppearance::Light, metrics());
        assert_eq!(dark.info, hex("#539af8"), "info is the accent");
        for theme in [&dark, &light] {
            for solid in [
                theme.destructive_solid,
                theme.success_solid,
                theme.warning_solid,
                theme.info_solid,
            ] {
                let ratio = crate::theme::contrast_ratio(solid, theme.solid_foreground);
                assert!(ratio >= 4.5, "{} is {ratio}", to_hex(solid));
            }
        }
        assert_eq!(to_hex(dark.destructive_solid), "#d23f39");
        assert_eq!(to_hex(dark.success_tint), "#25372a");
        assert_eq!(to_hex(dark.destructive_tint), "#412421");
        assert_eq!(to_hex(light.success_tint), "#ebf6ec");
        assert_eq!(to_hex(light.destructive_tint), "#fbebe8");
    }

    #[test]
    fn the_progress_track_is_the_primary_at_a_fifth() {
        let dark = dark();
        assert_eq!(to_hex(dark.primary()), "#dfdfdf");
        assert_eq!(to_hex(dark.progress_track), "#dfdfdf33");
        let light = light();
        assert_eq!(to_hex(light.progress_track), "#31333533");
    }

    #[test]
    fn switch_colors_reproduce_the_reference_values() {
        // The on track and the thumb are sampled from the reference app's
        // settings toggles at 2x on dark.
        let dark = dark();
        assert_eq!(to_hex(dark.switch_track_on), "#539af8");
        assert_eq!(to_hex(dark.switch_thumb), "#ffffff");
        assert_eq!(dark.switch_track_on, dark.ring());
        assert!(lightness(dark.switch_track_off) > lightness(dark.background()));
        assert!(lightness(dark.switch_track_off) < lightness(dark.switch_thumb));
        let light = light();
        assert_eq!(to_hex(light.switch_thumb), "#ffffff");
        assert_eq!(light.switch_track_on, light.ring());
        assert!(lightness(light.switch_track_off) < lightness(light.background()));
        assert_eq!(to_hex(light.focus_ring()), "#339cff80");
        assert_eq!(light.focus_ring().a, 0.5);
    }

    #[test]
    fn menu_colors_reproduce_the_reference_values() {
        let dark = dark();
        assert_eq!(to_hex(dark.popover), "#2d2d2d");
        assert_eq!(to_hex(dark.popover_border), "#444444");
        assert_eq!(to_hex(dark.popover_accent), "#3d3d3d");
        assert_eq!(to_hex(dark.popover_separator), "#3e3e3e");
        assert_eq!(to_hex(dark.popover_foreground), "#ffffff");
        assert_eq!(to_hex(dark.popover_muted_foreground), "#b5b5b5");
        assert_eq!(to_hex(dark.select_indicator), "#cacaca");
        assert_eq!(to_hex(dark.select_trigger), "#292929");
        assert_eq!(to_hex(dark.select_trigger_border), "#3a3a3a");
        assert!(lightness(dark.popover) > lightness(dark.base.colors.surface));
        let light = light();
        assert_eq!(light.popover, light.background());
        assert_eq!(light.popover_border, light.border());
        assert_eq!(light.popover_separator, light.border());
        assert_eq!(light.popover_foreground, light.foreground());
        assert_eq!(light.popover_muted_foreground, light.muted_foreground());
        assert_eq!(light.select_trigger, light.background());
        assert_eq!(light.select_trigger_border, light.input());
        assert_eq!(to_hex(light.popover_accent), "#f6f6f7", "a hovered row");
        assert!(lightness(light.popover_accent) < lightness(light.popover));
    }

    #[test]
    fn menu_metrics_scale_with_the_font_size() {
        let default = light();
        assert_eq!(default.metrics.menu_gap, px(2.));
        assert_eq!(default.metrics.menu_min_width, px(128.));
        assert_eq!(default.metrics.menu_search_min_width, px(240.));
        assert_eq!(default.metrics.menu_max_width, px(360.));
        assert_eq!(default.metrics.menu_max_height, px(390.));
        let large = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                radius: px(10.),
                ui_font_size: px(20.),
                code_font_size: px(15.),
                touch: false,
            },
        );
        assert_eq!(large.metrics.menu_max_height, px(487.5));
        assert_eq!(large.metrics.menu_gap, px(2.5));
    }

    #[test]
    fn dialog_tokens_reproduce_the_reference_values() {
        // The backdrop is #121212 over the #181818 dark window: black at
        // 25% alpha.
        let dark = dark();
        assert_eq!(to_hex(dark.dialog_overlay), "#00000040");
        assert_eq!(to_hex(dark.background()), "#181818");
        let over = (f32::from(0x18u8) * (1. - dark.dialog_overlay.a)).round() as u8;
        assert_eq!(over, 0x12);
        assert_eq!(to_hex(light().dialog_overlay), "#00000040");
        let default = light();
        assert_eq!(default.metrics.dialog_width, px(480.));
        assert_eq!(default.metrics.dialog_padding, px(20.));
        assert_eq!(default.metrics.command_dialog_width, px(512.));
        assert_eq!(default.metrics.command_dialog_top, px(120.));
        assert_eq!(default.metrics.folder_list_height, px(182.));
    }

    #[test]
    fn field_colors_reproduce_the_reference_values() {
        // The fill is sampled from the reference app's settings textarea
        // and the rest hairline from its select trigger, both at 2x on the
        // dark window surface.
        let dark = dark();
        assert_eq!(to_hex(dark.field), "#2c2c2c");
        assert_eq!(to_hex(dark.field_border), "#3b3b3b");
        assert_eq!(
            to_hex(dark.field_focus_border),
            "#799cca",
            "two steps from the #799ec8 sample, inside its noise"
        );
        assert_eq!(dark.text_textarea.line_height, px(18.5));
        assert_eq!(dark.text_textarea.size, px(13.));
        assert!(lightness(dark.field_border) > lightness(dark.field));
        assert!(lightness(dark.field) > lightness(dark.background()));
        let light = light();
        assert_eq!(light.field, light.background());
        assert_eq!(light.field_border, light.input());
        assert_eq!(light.field_focus_border, light.ring());
    }

    #[test]
    fn accent_and_semantic_colors_pass_through() {
        let light = light();
        assert_eq!(light.ring(), hex("#339cff"));
        assert_eq!(light.destructive(), hex("#ba2623"));
        assert_eq!(light.destructive_foreground(), hex("#ffffff"));
        assert_eq!(light.selection().a, 0.3);
        let dark = dark();
        assert_eq!(dark.ring(), hex("#539af8"));
        assert_eq!(
            to_hex(dark.link),
            "#91bffd",
            "links are paler than the ring on dark"
        );
        assert_eq!(light.link, light.ring(), "and the accent itself on light");
        assert_eq!(dark.destructive_foreground(), hex("#ffffff"));
        let mut pale = ThemeConfig::light();
        pale.semantic.destructive = hex("#ffe066");
        let pale = ThemeTokens::derive(&pale, ThemeAppearance::Light, metrics());
        assert_eq!(
            pale.destructive_foreground(),
            hex("#1a1c1f"),
            "ink reads on pale yellow"
        );
    }

    #[test]
    fn radius_scale_derives_from_one_value() {
        let tokens = light();
        assert_eq!(tokens.radius_sm(), px(6.));
        assert_eq!(tokens.radius_md(), px(8.));
        assert_eq!(tokens.radius_lg(), px(10.));
        assert_eq!(tokens.radius_xl(), px(14.));
        assert_eq!(tokens.radius_full(), px(9999.));
    }

    #[test]
    fn control_text_and_metrics_scale_with_the_font_size_but_chrome_does_not() {
        let default = light();
        assert_eq!(default.text_control.size, px(13.));
        assert_eq!(default.text_control.line_height, px(16.));
        assert_eq!(default.text_heading.size, px(15.));
        assert_eq!(default.text_title.size, px(24.));
        assert_eq!(default.text_title.weight, FontWeight::SEMIBOLD);
        assert_eq!(default.metrics.control_md, px(28.));
        assert_eq!(default.metrics.row, px(30.));
        assert_eq!(default.metrics.title_bar, px(38.));
        assert_eq!(default.metrics.title_bar_content_offset, px(2.));
        assert_eq!(default.metrics.window_controls_position, px(14.));
        let large = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                radius: px(10.),
                ui_font_size: px(20.),
                code_font_size: px(15.),
                touch: false,
            },
        );
        assert_eq!(large.text_control.size, px(16.25));
        assert_eq!(large.metrics.control_md, px(35.));
        assert_eq!(large.metrics.row, px(37.5));
        assert_eq!(large.metrics.title_bar, px(38.), "window chrome stays");
        assert_eq!(large.metrics.sidebar_width, px(300.), "layout stays");
        assert_eq!(default.metrics.switch_track_width, px(32.));
        assert_eq!(default.metrics.switch_track_height, px(20.));
        assert_eq!(default.metrics.switch_thumb_size, px(16.));
        assert_eq!(default.metrics.switch_thumb_inset, px(2.));
        assert_eq!(large.metrics.switch_track_width, px(40.));
        assert_eq!(large.metrics.switch_thumb_size, px(20.));
        assert_eq!(large.metrics.switch_thumb_inset, px(2.5));
        assert_eq!(default.metrics.avatar_md, px(32.));
        assert_eq!(default.metrics.tag_height, px(20.));
        assert_eq!(large.metrics.tag_height, px(25.));
        assert_eq!(default.metrics.badge_count, px(16.));
        assert_eq!(default.metrics.badge_dot, px(6.));
        assert_eq!(large.metrics.avatar_md, px(40.));
        assert_eq!(large.metrics.avatar_group_ring, px(2.), "a hairline stays");
        assert_eq!(default.metrics.terminal_padding, px(2.));
        assert_eq!(
            large.metrics.terminal_padding,
            px(2.),
            "the grid's room stays"
        );
        assert_eq!(large.metrics.terminal_cursor_thickness, px(2.));
        assert_eq!(large.terminal_cursor_opacity, 0.55);
    }

    #[test]
    fn the_tab_separator_is_the_muted_text_at_sixty_percent() {
        let dark = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        assert_eq!(dark.tab_separator.a, 0.6);
        assert_eq!(
            to_hex(dark.tab_separator.alpha(1.)),
            to_hex(dark.muted_foreground())
        );
        let light = crate::theme::test_tokens(&ThemeConfig::light(), ThemeAppearance::Light);
        assert_eq!(light.tab_separator.a, 0.6);
    }

    #[test]
    fn tab_metrics_are_the_reference_values_and_grow_on_touch() {
        // Measured from the tab reference app at 2x: a 38px bar whose tab
        // surface starts 4px down, 160px items, 7px shoulders and corners.
        let default = light();
        let tab = &default.metrics;
        assert_eq!(tab.tab_surface_inset, px(4.));
        assert_eq!(tab.tab_width, px(160.));
        assert_eq!(tab.tab_min_width, px(112.));
        assert_eq!(tab.tab_shoulder, px(7.));
        assert_eq!(tab.tab_radius, px(7.));
        assert_eq!(tab.tab_separator_gutter, px(7.));
        assert_eq!(tab.tab_separator_inset, px(11.));
        assert_eq!(tab.tab_icon, px(13.));
        assert_eq!(tab.tab_content_gap, px(6.));
        assert_eq!(tab.tab_control, px(24.));
        assert_eq!(tab.tab_close, px(20.));
        let large = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                ui_font_size: px(20.),
                ..metrics()
            },
        );
        assert_eq!(large.metrics.tab_surface_inset, px(4.), "chrome stays");
        assert_eq!(large.metrics.tab_control, px(30.), "holds an Sm control");
        assert_eq!(large.metrics.tab_close, px(25.), "holds an Xs control");
        assert_eq!(large.metrics.tab_width, px(160.));
        let touch = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                touch: true,
                ..metrics()
            },
        );
        assert_eq!(touch.metrics.tab_surface_inset, px(0.), "fills the bar");
        assert_eq!(touch.metrics.tab_control, touch.metrics.control_sm);
        assert_eq!(touch.metrics.tab_close, touch.metrics.control_xs);
        assert_eq!(touch.metrics.tab_width, px(160.));
    }

    #[test]
    fn typography_follows_the_font_sizes_and_families() {
        let mut config = ThemeConfig::light();
        config.fonts.ui = Some("Inter".into());
        config.fonts.code = Some("JetBrains Mono".into());
        let tokens = ThemeTokens::derive(
            &config,
            ThemeAppearance::Light,
            Metrics {
                radius: px(10.),
                ui_font_size: px(20.),
                code_font_size: px(15.),
                touch: false,
            },
        );
        assert_eq!(tokens.font_family(), &SharedString::from("Inter"));
        assert_eq!(
            tokens.mono_font_family(),
            &SharedString::from("JetBrains Mono")
        );
        assert_eq!(tokens.base.typography.md.size, px(20.));
        assert_eq!(tokens.base.typography.sm.size, px(17.5));
        assert_eq!(tokens.base.typography.md.line_height, px(30.));
        assert_eq!(tokens.base.typography.mono_md.size, px(15.));
        let defaults =
            ThemeTokens::derive(&ThemeConfig::light(), ThemeAppearance::Light, metrics());
        assert_eq!(defaults.font_family(), &TypographyTokens::default().sans);
        assert_eq!(defaults.base.typography.md.size, px(16.));
        assert_eq!(defaults.base.typography.mono_md.size, px(13.));
    }

    #[test]
    fn slider_metrics_are_the_shadcn_values_and_scale_with_the_font_size() {
        let default = light();
        assert_eq!(default.metrics.slider_track, px(4.), "a thin rail");
        assert_eq!(default.metrics.slider_thumb, px(16.), "shadcn size-4");
        assert_eq!(default.metrics.slider_tick, px(4.));
        let large = ThemeTokens::derive(
            &ThemeConfig::light(),
            ThemeAppearance::Light,
            Metrics {
                ui_font_size: px(20.),
                ..metrics()
            },
        );
        assert_eq!(large.metrics.slider_thumb, px(20.));
    }

    #[test]
    fn a_chosen_radio_and_an_on_switch_share_the_accent() {
        for (tokens, hex) in [(dark(), "#539af8"), (light(), "#339cff")] {
            assert_eq!(to_hex(tokens.switch_track_on), hex);
            assert_eq!(tokens.switch_track_on, tokens.ring());
        }
    }

    #[test]
    fn radio_metrics_are_the_shadcn_values() {
        let default = light();
        assert_eq!(default.metrics.radio_size, px(16.), "shadcn size-4");
        assert_eq!(
            default.metrics.radio_dot,
            px(6.),
            "three quarters of size-2"
        );
    }

    #[test]
    fn ring_metrics_scale_with_the_font_size() {
        let default = light();
        assert_eq!(default.metrics.ring_size, px(16.));
        assert_eq!(default.metrics.ring_stroke, px(2.));
    }

    #[test]
    fn the_attachment_tile_metric_and_the_model_picker_metrics_are_set() {
        let dark = dark();
        assert_eq!(dark.metrics.attachment_tile, px(56.));
        assert_eq!(dark.metrics.model_picker_width, px(361.));
        assert_eq!(dark.metrics.model_picker_height, px(346.));
        assert_eq!(dark.metrics.model_picker_rail, px(44.));
        assert_eq!(dark.metrics.model_row, px(47.));
        assert_eq!(dark.metrics.model_header, px(42.));
    }

    #[test]
    fn the_status_tab_metrics_are_the_reference_samples() {
        let m = &dark().metrics;
        assert_eq!(m.status_tab_height, px(38.));
        assert_eq!(m.status_tab_inset, px(14.));
        assert_eq!(m.status_tab_gap, px(28.));
        assert_eq!(m.status_label_gap, px(8.));
    }

    #[test]
    fn the_composer_reads_text_at_least_4_5_to_1_on_the_surfaces_it_uses() {
        use crate::theme::color::contrast_ratio;
        for tokens in [dark(), light()] {
            let muted = contrast_ratio(tokens.muted_foreground(), tokens.field);
            assert!(muted >= 4.5, "muted text on the card: {muted}");
            let warning = contrast_ratio(tokens.warning, tokens.field);
            assert!(warning >= 4.5, "the risky mode on the card: {warning}");
            let tab = contrast_ratio(tokens.foreground(), tokens.sidebar);
            assert!(tab >= 4.5, "text on the status tab: {tab}");
            let hairline = contrast_ratio(tokens.field_border, tokens.sidebar);
            assert!(
                hairline > 1.05,
                "the card's hairline reads on the tab: {hairline}"
            );
        }
    }

    #[test]
    fn dock_panes_take_ghosttys_handle_and_trellis_drag_sizes() {
        let dark = dark();
        assert_eq!(to_hex(dark.dock_drop.alpha(1.)), to_hex(dark.ring()));
        assert_eq!(dark.dock_drop.a, 0.3, "Ghostty's drop overlay");
        assert_eq!(dark.dock_handle.a, 0.3);
        assert_eq!(dark.dock_handle_hover.a, 0.8);
        assert_eq!(to_hex(dark.dock_unfocused.alpha(1.)), "#181818");
        assert_eq!(dark.dock_unfocused.a, 0.3, "Ghostty's unfocused split");
        assert_eq!(to_hex(dark.dock_handle.alpha(1.)), "#dfdfdf");
        let m = &dark.metrics;
        assert_eq!(m.dock_handle_width, px(80.));
        assert_eq!(m.dock_handle_height, px(12.));
        assert_eq!(m.dock_handle_reveal, 0.2);
        assert_eq!(m.dock_drag_threshold, px(6.));
        assert_eq!(m.dock_card, Size::new(px(380.), px(260.)));
        let touch = ThemeTokens::derive(
            &ThemeConfig::dark(),
            ThemeAppearance::Dark,
            Metrics {
                touch: true,
                ..metrics()
            },
        );
        assert_eq!(touch.metrics.dock_handle_height, px(44.), "the HIG target");
        assert_eq!(touch.metrics.dock_handle_width, px(80.));
    }
}
