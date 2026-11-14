//! Text that gives up its middle when it does not fit, so both ends stay
//! readable: a branch such as `origin/fix/psmode-curre...x-evidence`.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui_kit::{
    AbsoluteLength, App, AvailableSpace, Bounds, DefiniteLength, Element, ElementId, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, ShapedLine,
    SharedString, Size, Style, TextAlign, TextRun, Window,
};

/// The mark that stands for the removed middle.
const ELLIPSIS: &str = "...";

/// `text` cut in the middle until `fits` accepts it: the same number of
/// characters, give or take one, stay at each end, and the ellipsis joins
/// them. Text that already fits comes back whole. When even the ellipsis
/// does not fit, the ellipsis alone comes back. The marked byte `ranges`
/// that survive the cut come back moved to the shown text. `fits` sees
/// each candidate with its ranges, since marked characters can be wider.
pub(crate) fn cut(
    text: &str,
    ranges: &[Range<usize>],
    fits: impl Fn(&str, &[Range<usize>]) -> bool,
) -> (String, Vec<Range<usize>>) {
    if fits(text, ranges) {
        return (text.to_string(), ranges.to_vec());
    }
    let chars: Vec<char> = text.chars().collect();
    let mut offsets: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    offsets.push(text.len());
    let candidate = |kept: usize| {
        let head = kept.div_ceil(2);
        let tail = kept / 2;
        let mut out: String = chars[..head].iter().collect();
        out.push_str(ELLIPSIS);
        out.extend(chars[chars.len() - tail..].iter());
        let head_end = offsets[head];
        let tail_start = offsets[chars.len() - tail];
        let mut moved = Vec::new();
        for range in ranges {
            let kept_head = range.start..range.end.min(head_end);
            if kept_head.start < kept_head.end {
                moved.push(kept_head);
            }
            let kept_tail = range.start.max(tail_start)..range.end;
            if kept_tail.start < kept_tail.end {
                let shift = head_end + ELLIPSIS.len();
                moved
                    .push(kept_tail.start - tail_start + shift..kept_tail.end - tail_start + shift);
            }
        }
        (out, moved)
    };
    let (mut low, mut high) = (0, chars.len().saturating_sub(1));
    while low < high {
        let mid = (low + high).div_ceil(2);
        let (shown, moved) = candidate(mid);
        if fits(&shown, &moved) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    candidate(low)
}

/// A single line of text in the inherited text style that is cut in the
/// middle to fit the width its parent gives it. It takes the width it
/// needs, up to what is available, and never wraps.
///
/// The text is measured while the layout runs, so the cut follows the
/// width the row has, not a guess made when the row was built.
pub(crate) struct MiddleText {
    text: SharedString,
    marked: Vec<Range<usize>>,
    mark_color: Option<Hsla>,
}

impl MiddleText {
    /// A line of `text`.
    pub(crate) fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            marked: Vec::new(),
            mark_color: None,
        }
    }

    /// Marks the byte `ranges` of the text the way a search marks its
    /// matches: in `color`, at medium weight.
    pub(crate) fn marked(mut self, ranges: &[Range<usize>], color: Hsla) -> Self {
        self.marked = ranges.to_vec();
        self.mark_color = Some(color);
        self
    }
}

impl IntoElement for MiddleText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

type Shown = Rc<RefCell<Option<ShapedLine>>>;

impl Element for MiddleText {
    type RequestLayoutState = Shown;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, Shown) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style.line_height_in_pixels(window.rem_size());
        let shown: Shown = Rc::new(RefCell::new(None));
        let text = self.text.clone();
        let marked = self.marked.clone();
        let mark_color = self.mark_color;
        let out = shown.clone();
        let mut layout = Style::default();
        layout.min_size.width =
            DefiniteLength::Absolute(AbsoluteLength::Pixels(Pixels::ZERO)).into();
        let id = window.request_measured_layout(layout, move |known, available, window, _| {
            let shape = |candidate: &str, ranges: &[Range<usize>]| {
                let runs = runs(&style, candidate.len(), ranges, mark_color);
                window.text_system().shape_line(
                    SharedString::from(candidate.to_string()),
                    font_size,
                    &runs,
                    None,
                )
            };
            let limit = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            let (shown_text, shown_marks) = match limit {
                Some(limit) => cut(&text, &marked, |candidate, ranges| {
                    shape(candidate, ranges).width <= limit
                }),
                None => (text.to_string(), marked.clone()),
            };
            let line = shape(&shown_text, &shown_marks);
            let width = line.width;
            *out.borrow_mut() = Some(line);
            Size {
                width: known.width.unwrap_or(width),
                height: known.height.unwrap_or(line_height),
            }
        });
        (id, shown)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Shown,
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        shown: &mut Shown,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = window.text_style().line_height_in_pixels(window.rem_size());
        if let Some(line) = shown.borrow().as_ref() {
            let _ = line.paint(
                bounds.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

/// The runs of a line of `len` bytes: the inherited style, with `marked`
/// ranges in `color` at medium weight.
fn runs(
    style: &gpui_kit::TextStyle,
    len: usize,
    marked: &[Range<usize>],
    color: Option<Hsla>,
) -> Vec<TextRun> {
    let plain = style.to_run(len);
    let Some(color) = color.filter(|_| !marked.is_empty()) else {
        return vec![plain];
    };
    let mut runs = Vec::new();
    let mut at = 0;
    let mut push = |end: usize, mark: bool, runs: &mut Vec<TextRun>| {
        if end > at {
            let mut run = style.to_run(end - at);
            if mark {
                run.font.weight = FontWeight::MEDIUM;
                run.color = color;
            }
            runs.push(run);
            at = end;
        }
    };
    for range in marked {
        push(range.start.min(len), false, &mut runs);
        push(range.end.min(len), true, &mut runs);
    }
    push(len, false, &mut runs);
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn middle_ellipsis(text: &str, fits: impl Fn(&str) -> bool) -> String {
        cut(text, &[], |candidate, _| fits(candidate)).0
    }

    fn fits(width: usize) -> impl Fn(&str) -> bool {
        move |text: &str| text.chars().count() <= width
    }

    #[test]
    fn text_that_fits_is_whole() {
        assert_eq!(middle_ellipsis("main", fits(10)), "main");
    }

    #[test]
    fn a_long_name_keeps_both_ends() {
        let name = "origin/fix/psmode-current-evidence";
        let cut = middle_ellipsis(name, fits(20));
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.starts_with("origin/"), "{cut}");
        assert!(cut.ends_with("evidence"), "{cut}");
        assert!(cut.contains("..."));
    }

    #[test]
    fn it_never_returns_more_than_fits_and_degrades_to_the_mark() {
        for width in 3..30 {
            let cut = middle_ellipsis("origin/renovate/anthropic-ai-sdk-0.x", fits(width));
            assert!(cut.chars().count() <= width, "{width}: {cut}");
        }
        assert_eq!(middle_ellipsis("abcdefgh", fits(2)), "...");
    }

    #[test]
    fn marked_ranges_follow_the_cut() {
        let text = "abcdefghij";
        let (shown, marks) = cut(text, &[0..1, 4..6, 9..10], |candidate, _| {
            candidate.len() <= 7
        });
        assert_eq!(shown, "ab...ij");
        assert_eq!(marks, vec![0..1, 6..7]);
        let (whole, marks) = cut(text, &[2..3, 5..6], |_, _| true);
        assert_eq!((whole.as_str(), marks), (text, vec![2..3, 5..6]));
    }

    #[test]
    fn multibyte_text_is_cut_on_characters() {
        let cut = middle_ellipsis("\u{e9}t\u{e9}-\u{e9}t\u{e9}-\u{e9}t\u{e9}", fits(7));
        assert!(cut.chars().count() <= 7);
        assert!(cut.starts_with('\u{e9}') && cut.ends_with('\u{e9}'));
    }
}
