//! The Explore a Table panel arrangement, independent of the window toolkit.

use serde::{Deserialize, Serialize};

pub(crate) const MIN_PANEL: f32 = 240.;
const MAX_COLUMNS: usize = 3;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Panel {
    Explorer,
    Plots,
    Guide,
    Ai,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Stack {
    bottom: Panel,
    split: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Column {
    pub(crate) top: Panel,
    width: f32,
    stack: Option<Stack>,
}

impl Column {
    fn single(top: Panel, width: f32) -> Self {
        Self {
            top,
            width,
            stack: None,
        }
    }

    pub(crate) fn bottom(self) -> Option<Panel> {
        self.stack.map(|stack| stack.bottom)
    }

    pub(crate) fn top_height(self, room: f32) -> f32 {
        let Some(stack) = self.stack else { return room };
        let min = MIN_PANEL.min(room / 2.);
        (stack.split * room).clamp(min, room - min)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(try_from = "SavedLayout")]
pub(crate) struct Layout {
    columns: Vec<Column>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_visible: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedLayout {
    columns: Vec<Column>,
    #[serde(default)]
    ai_visible: Option<bool>,
}

impl TryFrom<SavedLayout> for Layout {
    type Error = &'static str;

    fn try_from(saved: SavedLayout) -> Result<Self, Self::Error> {
        let mut seen = [false; 4];
        if !(2..=MAX_COLUMNS).contains(&saved.columns.len())
            || saved.columns.iter().any(|column| {
                !(column.width > 0. && column.width <= 1.)
                    || column
                        .stack
                        .is_some_and(|s| !(s.split >= f32::EPSILON && s.split < 1.))
                    || [Some(column.top), column.bottom()]
                        .into_iter()
                        .flatten()
                        .any(|panel| std::mem::replace(&mut seen[panel as usize], true))
            })
        {
            return Err("invalid Explore layout");
        }
        Ok(Self {
            columns: saved.columns,
            ai_visible: saved.ai_visible,
        })
    }
}

/// Huan, 2026-11-19: the Data Explorer on the left, Plots above the Guide
/// in the middle, the AI on the right, in widths of 2:1:1.
impl Default for Layout {
    fn default() -> Self {
        let mut middle = Column::single(Panel::Plots, 1. / 4.);
        Self::stack(&mut middle, Panel::Guide, false);
        Self {
            columns: vec![
                Column::single(Panel::Explorer, 2. / 4.),
                middle,
                Column::single(Panel::Ai, 1. / 4.),
            ],
            ai_visible: None,
        }
    }
}

impl Layout {
    pub(crate) fn columns(&self) -> &[Column] {
        &self.columns
    }

    fn locate(&self, panel: Panel) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.top == panel || c.bottom() == Some(panel))
    }

    pub(crate) fn is_visible(&self, panel: Panel) -> bool {
        self.locate(panel).is_some()
    }

    pub(crate) fn ai_visible(&self, ready: bool) -> bool {
        self.ai_visible.unwrap_or(ready)
    }

    pub(crate) fn choose_ai_visibility(&mut self, visible: bool) {
        self.ai_visible = Some(visible);
        self.set_visible(Panel::Ai, visible);
    }

    pub(crate) fn reset(&mut self) -> bool {
        let mut default = Self {
            ai_visible: self.ai_visible,
            ..Self::default()
        };
        default.set_visible(Panel::Ai, default.ai_visible(false));
        if *self == default {
            return false;
        }
        *self = default;
        true
    }

    /// The half to outline, which can differ from the pointer's half when
    /// adding a column would exceed the limit.
    pub(crate) fn drop_side(&self, panel: Panel, target: Panel, side: Side) -> Option<Side> {
        if panel == target {
            return None;
        }
        let source = self.locate(panel)?;
        let target_ix = self.locate(target)?;
        let source_single = self.columns[source].stack.is_none();
        let remaining = self.columns.len() - usize::from(source_single);
        let side = match side {
            Side::Left if remaining == MAX_COLUMNS => Side::Top,
            Side::Right if remaining == MAX_COLUMNS => Side::Bottom,
            _ => side,
        };
        match side {
            Side::Top | Side::Bottom => {
                if remaining < 2 || (source != target_ix && self.columns[target_ix].stack.is_some())
                {
                    return None;
                }
                if source == target_ix && (self.columns[source].top == panel) == (side == Side::Top)
                {
                    return None;
                }
            }
            Side::Left if source_single && source + 1 == target_ix => return None,
            Side::Right if source_single && target_ix + 1 == source => return None,
            _ => {}
        }
        Some(side)
    }

    pub(crate) fn move_panel(&mut self, panel: Panel, target: Panel, side: Side) -> bool {
        let Some(side) = self.drop_side(panel, target, side) else {
            return false;
        };
        let source = self.locate(panel).unwrap();
        let mut target_ix = self.locate(target).unwrap();
        if source == target_ix && matches!(side, Side::Top | Side::Bottom) {
            let column = &mut self.columns[source];
            let stack = column.stack.as_mut().unwrap();
            std::mem::swap(&mut column.top, &mut stack.bottom);
            stack.split = 1. - stack.split;
            return true;
        }
        let removed_width = self.remove(source, panel);
        if removed_width.is_some() && source < target_ix {
            target_ix -= 1;
        }
        match side {
            Side::Left | Side::Right => {
                let width = removed_width.unwrap_or_else(|| {
                    let column = &mut self.columns[source];
                    column.width /= 2.;
                    column.width
                });
                let insert = target_ix + usize::from(side == Side::Right);
                self.columns.insert(insert, Column::single(panel, width));
            }
            Side::Top | Side::Bottom => {
                let column = &mut self.columns[target_ix];
                column.width += removed_width.unwrap_or(0.);
                Self::stack(column, panel, side == Side::Top);
            }
        }
        true
    }

    fn stack(column: &mut Column, panel: Panel, above: bool) {
        let bottom = if above {
            std::mem::replace(&mut column.top, panel)
        } else {
            panel
        };
        column.stack = Some(Stack { bottom, split: 0.5 });
    }

    /// Removes the panel, returning the vacated width if its column vanished.
    fn remove(&mut self, ix: usize, panel: Panel) -> Option<f32> {
        let column = &mut self.columns[ix];
        if let Some(stack) = column.stack.take() {
            if panel == column.top {
                column.top = stack.bottom;
            }
            None
        } else {
            Some(self.columns.remove(ix).width)
        }
    }

    /// Hiding a panel gives its room back to the others. Showing it adds a
    /// column next to the Guide when possible, as wide as the Guide's (the
    /// others narrowing in proportion, so hiding then showing the AI keeps
    /// 2:1:1), or stacks it with the Guide.
    pub(crate) fn set_visible(&mut self, panel: Panel, visible: bool) -> bool {
        match (self.locate(panel), visible) {
            (None, true) => {
                let guide = self.locate(Panel::Guide).unwrap_or(self.columns.len() - 1);
                if self.columns.len() < MAX_COLUMNS {
                    let width = self.columns[guide].width;
                    self.columns.insert(guide + 1, Column::single(panel, width));
                    let total: f32 = self.columns.iter().map(|c| c.width).sum();
                    for column in &mut self.columns {
                        column.width /= total;
                    }
                } else {
                    // At most four panels exist, so with one hidden and
                    // three columns each column has just one panel.
                    Self::stack(&mut self.columns[guide], panel, false);
                }
                true
            }
            (Some(ix), false) => {
                if self.columns.len() == 2 && self.columns.iter().all(|c| c.stack.is_none()) {
                    return false;
                }
                if self.remove(ix, panel).is_some() {
                    // Keep two columns when hiding leaves a single stack.
                    if self.columns.len() == 1 {
                        let column = &mut self.columns[0];
                        let bottom = column.stack.take().unwrap().bottom;
                        column.width = 0.5;
                        self.columns.push(Column::single(bottom, 0.5));
                    } else {
                        let remaining: f32 = self.columns.iter().map(|c| c.width).sum();
                        for column in &mut self.columns {
                            column.width /= remaining;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Pixels per column, excluding the splitter gaps. Clamp columns below
    /// the minimum first, spreading the rest by the remaining width shares.
    pub(crate) fn widths(&self, room: f32) -> [f32; MAX_COLUMNS] {
        let mut widths = [0.; MAX_COLUMNS];
        let equal = room / self.columns.len() as f32;
        if equal <= MIN_PANEL {
            widths[..self.columns.len()].fill(equal);
            return widths;
        }
        let mut available = room;
        loop {
            let shares: f32 = self
                .columns
                .iter()
                .zip(widths)
                .filter(|(_, width)| *width == 0.)
                .map(|(column, _)| column.width)
                .sum();
            if let Some((_, width)) = self
                .columns
                .iter()
                .zip(&mut widths)
                .find(|(c, w)| **w == 0. && available * (c.width / shares) < MIN_PANEL)
            {
                *width = MIN_PANEL;
                available -= MIN_PANEL;
                continue;
            }
            for (column, width) in self.columns.iter().zip(&mut widths) {
                if *width == 0. {
                    *width = available * (column.width / shares);
                }
            }
            return widths;
        }
    }

    /// Resize a boundary to a position within the room, excluding gaps.
    pub(crate) fn resize_columns(&mut self, left: usize, at: f32, room: f32) -> bool {
        if left >= self.columns.len() - 1 || room < MIN_PANEL * self.columns.len() as f32 {
            return false;
        }
        let mut widths = self.widths(room);
        let pair = widths[left] + widths[left + 1];
        let leading: f32 = widths[..left].iter().sum();
        let width = (at - leading).clamp(MIN_PANEL, (pair - MIN_PANEL).max(MIN_PANEL));
        if width == widths[left] {
            return false;
        }
        widths[left] = width;
        widths[left + 1] = pair - width;
        for (column, width) in self.columns.iter_mut().zip(widths) {
            column.width = width / room;
        }
        true
    }

    pub(crate) fn resize_stack(&mut self, ix: usize, at: f32, room: f32) -> bool {
        let Some(column) = self.columns.get_mut(ix) else {
            return false;
        };
        if column.stack.is_none() || room < 2. * MIN_PANEL {
            return false;
        }
        let previous = column.top_height(room);
        let height = at.clamp(MIN_PANEL, room - MIN_PANEL);
        if height == previous {
            return false;
        }
        column.stack.as_mut().unwrap().split = height / room;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Column, Layout, MIN_PANEL, Panel::*, Side::*};
    use serde_json::{Value, json};

    fn panels(layout: &Layout) -> Vec<(super::Panel, Option<super::Panel>)> {
        layout
            .columns()
            .iter()
            .map(|c| (c.top, c.bottom()))
            .collect()
    }

    /// Three single columns and no AI, to move panels around in.
    fn three() -> Layout {
        Layout {
            columns: vec![
                Column::single(Explorer, 3. / 8.),
                Column::single(Plots, 3. / 8.),
                Column::single(Guide, 2. / 8.),
            ],
            ai_visible: None,
        }
    }

    fn rounded(widths: [f32; 3]) -> [f32; 3] {
        widths.map(f32::round)
    }

    #[test]
    fn default_layout_is_explorer_then_plots_over_guide_then_ai_two_one_one() {
        let mut layout = Layout::default();
        assert_eq!(
            panels(&layout),
            [(Explorer, None), (Plots, Some(Guide)), (Ai, None)]
        );
        assert_eq!(layout.widths(1200.), [600., 300., 300.]);
        assert_eq!(layout.widths(720.), [240.; 3]);
        // Without the AI the other two keep 2:1, and showing it again
        // brings back 2:1:1.
        assert!(layout.set_visible(Ai, false));
        assert_eq!(panels(&layout), [(Explorer, None), (Plots, Some(Guide))]);
        assert_eq!(rounded(layout.widths(1200.)), [800., 400., 0.]);
        assert!(layout.set_visible(Ai, true));
        assert_eq!(panels(&layout), panels(&Layout::default()));
        assert_eq!(rounded(layout.widths(1200.)), [600., 300., 300.]);
    }

    #[test]
    fn moves_left_or_right_into_a_column() {
        let mut layout = three();
        assert!(layout.move_panel(Guide, Explorer, Left));
        assert_eq!(
            panels(&layout),
            [(Guide, None), (Explorer, None), (Plots, None)]
        );
        assert!(layout.move_panel(Guide, Plots, Right));
        assert_eq!(panels(&layout), panels(&three()));
    }

    #[test]
    fn moves_above_or_below_into_a_stack_and_can_unstack() {
        let mut layout = three();
        assert!(layout.move_panel(Plots, Explorer, Top));
        assert_eq!(panels(&layout), [(Plots, Some(Explorer)), (Guide, None)]);
        assert!(layout.move_panel(Plots, Explorer, Bottom));
        assert_eq!(panels(&layout), [(Explorer, Some(Plots)), (Guide, None)]);
        assert!(layout.move_panel(Plots, Guide, Left));
        assert_eq!(panels(&layout), panels(&three()));
    }

    #[test]
    fn refuses_a_third_panel_in_a_column() {
        let mut layout = three();
        assert!(layout.set_visible(Ai, true));
        assert!(!layout.move_panel(Plots, Guide, Top));
        assert!(!layout.move_panel(Explorer, Ai, Bottom));
        assert_eq!(
            panels(&layout),
            [(Explorer, None), (Plots, None), (Guide, Some(Ai))]
        );
    }

    #[test]
    fn side_drops_stack_when_a_fourth_column_would_be_needed() {
        let mut layout = three();
        assert!(layout.set_visible(Ai, true));
        assert_eq!(layout.drop_side(Ai, Explorer, Left), Some(Top));
        assert!(layout.move_panel(Ai, Explorer, Left));
        assert_eq!(
            panels(&layout),
            [(Ai, Some(Explorer)), (Plots, None), (Guide, None)]
        );
        assert_eq!(layout.drop_side(Ai, Plots, Right), Some(Bottom));
        assert!(layout.move_panel(Ai, Plots, Right));
        assert_eq!(
            panels(&layout),
            [(Explorer, None), (Plots, Some(Ai)), (Guide, None)]
        );
    }

    #[test]
    fn resizing_keeps_each_panel_at_least_240_pixels_when_they_fit() {
        let mut layout = three();
        assert!(layout.resize_columns(0, -300., 1200.));
        assert_eq!(layout.widths(1200.), [MIN_PANEL, 660., 300.]);
        assert!(layout.resize_columns(0, 900., 1200.));
        assert_eq!(layout.widths(1200.), [660., MIN_PANEL, 300.]);
        assert_eq!(layout.widths(600.), [200.; 3]);
        assert!(!layout.resize_columns(0, 20., 600.));
        assert!(layout.move_panel(Plots, Explorer, Bottom));
        assert!(layout.resize_stack(0, 900., 600.));
        assert_eq!(layout.columns()[0].top_height(600.), 600. - MIN_PANEL);
        assert!(layout.resize_stack(0, -300., 600.));
        assert_eq!(layout.columns()[0].top_height(600.), MIN_PANEL);
        assert_eq!(layout.columns()[0].top_height(400.), 200.);
        assert!(!layout.resize_stack(0, 50., 400.));
    }

    #[test]
    fn shows_ai_next_to_guide_and_removes_it_wherever_it_moved() {
        let mut layout = three();
        assert!(layout.set_visible(Ai, true));
        assert_eq!(layout.columns()[2].bottom(), Some(Ai));
        assert!(layout.move_panel(Ai, Explorer, Bottom));
        assert!(layout.set_visible(Ai, false));
        assert_eq!(layout, three());
        assert!(!layout.set_visible(Ai, false));
        assert!(layout.move_panel(Explorer, Plots, Top));
        assert!(layout.set_visible(Ai, true));
        assert_eq!(
            panels(&layout),
            [(Explorer, Some(Plots)), (Guide, None), (Ai, None)]
        );
        assert!(layout.set_visible(Ai, false));
        assert_eq!(panels(&layout), [(Explorer, Some(Plots)), (Guide, None)]);
    }

    #[test]
    fn hiding_a_panel_never_leaves_one_column() {
        let mut layout = three();
        assert!(layout.move_panel(Plots, Explorer, Bottom));
        assert!(layout.set_visible(Guide, false));
        assert_eq!(panels(&layout), [(Explorer, None), (Plots, None)]);
        assert!(!layout.set_visible(Plots, false));
        assert!(!layout.move_panel(Plots, Explorer, Bottom));
    }

    #[test]
    fn reset_restores_the_default() {
        let mut layout = Layout::default();
        layout.choose_ai_visibility(true);
        assert!(layout.move_panel(Ai, Explorer, Bottom));
        assert!(layout.resize_stack(0, 350., 600.));
        assert!(layout.reset());
        assert_eq!(layout.columns(), Layout::default().columns());
        assert!(!layout.reset());
        // Never chosen, the AI starts hidden until an agent is ready.
        let mut layout = Layout::default();
        assert!(layout.reset());
        assert!(!layout.is_visible(Ai));
    }

    #[test]
    fn round_trips_a_custom_layout_and_falls_back_for_invalid_or_older_json() {
        let default = Layout::default();
        let mut custom = three();
        assert!(custom.set_visible(Ai, true));
        assert!(custom.resize_stack(2, 350., 600.));
        let saved = serde_json::to_value(&custom).unwrap();
        assert_eq!(
            serde_json::from_value::<Layout>(saved.clone()).unwrap(),
            custom
        );

        let mut duplicate = saved.clone();
        duplicate["columns"][0]["top"] = json!("plots");
        let mut zero_width = saved.clone();
        zero_width["columns"][0]["width"] = json!(0.);
        let mut invalid_split = saved;
        invalid_split["columns"][2]["stack"]["split"] = json!(1.);
        for invalid in [
            Value::Null,
            json!({"columns": [["explorer"], ["plots"], ["guide"]], "shares": [3, 3, 2]}),
            json!({"columns": [{"top": "explorer", "width": 1.}]}),
            duplicate,
            zero_width,
            invalid_split,
        ] {
            assert_eq!(
                serde_json::from_value::<Layout>(invalid).unwrap_or_default(),
                default
            );
        }
    }

    #[test]
    fn an_explicit_ai_choice_survives_reset_and_old_prefs_follow_readiness() {
        let older = serde_json::to_value(Layout::default()).unwrap();
        assert!(older.get("ai_visible").is_none());
        let mut layout = serde_json::from_value::<Layout>(older).unwrap();
        assert!(layout.ai_visible(true));
        assert!(!layout.ai_visible(false));
        for visible in [true, false] {
            layout.choose_ai_visibility(visible);
            assert_eq!(layout.is_visible(Ai), visible);
            assert!(layout.move_panel(Guide, Explorer, Left));
            assert!(layout.reset());
            let reloaded: Layout =
                serde_json::from_value(serde_json::to_value(&layout).unwrap()).unwrap();
            assert_eq!(reloaded.ai_visible(!visible), visible);
            assert_eq!(reloaded.is_visible(Ai), visible);
        }
    }
}
