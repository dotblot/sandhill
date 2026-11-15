use std::time::Duration;

use gpui_kit::base::{Easing, Transition};

/// Timing gpui-cn components animate with.
///
/// Components sample motion through `gpui-base` (`transition`, `spring`,
/// `Presence`, `Sequence`, ...), which snaps to the target when reduce
/// motion is on. These tokens only say how long and with what curve.
#[derive(Clone)]
#[non_exhaustive]
pub struct MotionTokens {
    /// Hover, pressed, and focus-ring changes.
    pub fast: Duration,
    /// Tooltip and popover enter and exit.
    pub normal: Duration,
    /// Sheet, dialog, and measured reveal.
    pub slow: Duration,
    /// The curve for something appearing or settling.
    pub enter: Easing,
    /// The curve for something leaving.
    pub exit: Easing,
    /// The curve for something that moves between two resting places in
    /// view: a sidebar sliding, a disclosure's rows folding. An
    /// ease-in-out, since both ends are on screen and neither should
    /// snap; the enter curve, which leaps and then settles, is for
    /// something arriving.
    pub fold: Easing,
    /// Delay between items in a staggered list.
    pub stagger: Duration,
    /// A dock pane moving to its new slot when the layout changes: 460ms,
    /// `vd.layoutMs` in the Trellis bundle.
    pub layout: Duration,
    /// A lifted dock pane shrinking into its floating card: 220ms,
    /// `appearMs` in the Trellis bundle.
    pub appear: Duration,
    /// How long the pointer rests on a new drop target before the dock
    /// previews it there: 150ms, `_v` in the Trellis bundle. A hysteresis,
    /// not motion, so [`MotionTokens::none`] keeps it.
    pub retarget: Duration,
    /// The curve of dock layout motion: ease-out quint, `1 - (1 - t)^5`,
    /// as Trellis moves its panels.
    pub layout_curve: Easing,
}

impl Default for MotionTokens {
    fn default() -> Self {
        Self {
            fast: Duration::from_millis(120),
            normal: Duration::from_millis(180),
            slow: Duration::from_millis(280),
            // The same curves gpui-component uses, so motion feels the same
            // across both layers in one application.
            enter: Easing::cubic_bezier(0.16, 1.0, 0.3, 1.0).expect("valid enter curve"),
            exit: Easing::cubic_bezier(0.4, 0.0, 1.0, 1.0).expect("valid exit curve"),
            // The standard ease-in-out, the pace of a macOS outline's
            // disclosure.
            fold: Easing::cubic_bezier(0.4, 0.0, 0.2, 1.0).expect("valid fold curve"),
            stagger: Duration::from_millis(40),
            layout: Duration::from_millis(460),
            appear: Duration::from_millis(220),
            retarget: Duration::from_millis(150),
            layout_curve: Easing::Custom(std::rc::Rc::new(ease_out_quint)),
        }
    }
}

/// Ease-out quint, `1 - (1 - t)^5`: fast off the mark and a long settle.
fn ease_out_quint(t: f32) -> f32 {
    1. - (1. - t).powi(5)
}

impl MotionTokens {
    /// A `slow` transition with the fold curve, for a region that folds.
    pub fn fold_transition(&self) -> Transition {
        Transition::new(self.slow).easing(self.fold.clone())
    }

    /// A `fast` transition with the fold curve, for a thumb that slides
    /// between the two ends of its track.
    pub fn slide_transition(&self) -> Transition {
        Transition::new(self.fast).easing(self.fold.clone())
    }

    /// A `normal` transition with the enter curve, for content that
    /// glides to a new resting place: a tab strip paging or taking a wheel
    /// step. The enter curve leaves fast and settles, the shape of a
    /// scroll that decelerates, so a run of steps reads as one motion.
    pub fn glide_transition(&self) -> Transition {
        Transition::new(self.normal).easing(self.enter.clone())
    }

    /// A `fast` transition with the enter curve.
    pub fn fast_transition(&self) -> Transition {
        Transition::new(self.fast).easing(self.enter.clone())
    }

    /// A `normal` transition with the enter curve.
    pub fn enter_transition(&self) -> Transition {
        Transition::new(self.normal).easing(self.enter.clone())
    }

    /// A `normal` transition with the exit curve.
    pub fn exit_transition(&self) -> Transition {
        Transition::new(self.normal).easing(self.exit.clone())
    }

    /// A `slow` transition with the enter curve.
    pub fn slow_transition(&self) -> Transition {
        Transition::new(self.slow).easing(self.enter.clone())
    }

    /// Motion tokens with every duration at zero, for a product that wants
    /// no animation regardless of the system preference.
    pub fn none() -> Self {
        Self {
            fast: Duration::ZERO,
            normal: Duration::ZERO,
            slow: Duration::ZERO,
            stagger: Duration::ZERO,
            layout: Duration::ZERO,
            appear: Duration::ZERO,
            ..Self::default()
        }
    }
}
