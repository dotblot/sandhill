//! Trigger hit regions shared by custom dropdowns.
//!
//! GPUI delivers a deferred panel's outside mousedown before the trigger's
//! click. A press on that panel's own trigger must leave state intact until
//! the click toggles it; presses on other triggers still dismiss the panel.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui_kit::{App, Bounds, Pixels, Point, Window};

#[derive(Clone, Default)]
pub(crate) struct PopupTriggers(Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>);

impl PopupTriggers {
    pub(crate) fn track(
        &self,
        id: &'static str,
    ) -> impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static {
        let regions = self.clone();
        move |bounds, _, _| {
            regions.0.borrow_mut().insert(id, bounds);
        }
    }

    pub(crate) fn contains(&self, id: &str, position: Point<Pixels>) -> bool {
        self.bounds(id)
            .is_some_and(|bounds| bounds.contains(&position))
    }

    pub(crate) fn bounds(&self, id: &str) -> Option<Bounds<Pixels>> {
        self.0.borrow().get(id).copied()
    }
}
