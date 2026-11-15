//! One shared clock for looping animations: the spinner's turn, the
//! skeleton's pulse, and the indeterminate progress sweep.
//!
//! GPUI's `with_animation` asks for every display frame, and the gallery
//! redraws the whole window for each one, so a spinner on a 120Hz display
//! cost about 25% of a core. A loop here asks for a repaint at its own
//! capped rate instead. The clock is shared because one timer per loop
//! repaints the window once per loop, out of step, which cost more than
//! the display-rate frames did.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use gpui_kit::{App, EntityId, Global, Task, Window};

/// The app's loop clock: when the loops started, the views that asked for
/// a repaint since the last tick, and the one pending tick with its due
/// time.
struct LoopClock {
    start: Instant,
    views: HashSet<EntityId>,
    tick: Option<(Instant, Task<()>)>,
}

impl Global for LoopClock {}

/// Where a loop of `period` stands now, from 0 to 1, and a request that
/// the view rendering it repaints `1 / fps` from now.
///
/// Every loop reads the same start, so loops of one period stay in step.
/// The view is asked to repaint once; it asks again when it renders, so a
/// loop that leaves the screen stops after one tick and an idle window
/// asks for nothing. Callers skip this under reduced motion.
pub(crate) fn phase(period: Duration, fps: u32, window: &Window, cx: &mut App) -> f32 {
    let now = cx.background_executor().now();
    let due = now + Duration::from_secs(1) / fps;
    if !cx.has_global::<LoopClock>() {
        cx.set_global(LoopClock {
            start: now,
            views: HashSet::default(),
            tick: None,
        });
    }
    let clock = cx.global_mut::<LoopClock>();
    clock.views.insert(window.current_view());
    let elapsed = now.saturating_duration_since(clock.start);
    let phase = (elapsed.as_secs_f64() / period.as_secs_f64()).fract() as f32;
    // A faster loop moves the pending tick earlier; replacing the task
    // cancels the later one.
    if clock
        .tick
        .as_ref()
        .is_none_or(|(pending, _)| due < *pending)
    {
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(due - now).await;
            cx.update(tick);
        });
        cx.global_mut::<LoopClock>().tick = Some((due, task));
    }
    phase
}

/// Repaints every view that asked since the last tick, once each.
fn tick(cx: &mut App) {
    let clock = cx.global_mut::<LoopClock>();
    // This runs inside the pending task, so let it finish rather than
    // cancel it.
    if let Some((_, task)) = clock.tick.take() {
        task.detach();
    }
    for view in std::mem::take(&mut clock.views) {
        cx.notify(view);
    }
}
