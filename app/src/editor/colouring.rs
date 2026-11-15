//! Choosing how a standalone text file is coloured (Huan, 2026-11-14): a
//! menu in the editor's action row. The choice is remembered for the file on
//! this Mac (`prefs::text_colouring`) and only changes colours.

use gpui_kit::base::ElementExt as _;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::editor::notebook::COLOURINGS;
use crate::look::{Palette, bar_button, icon};
use crate::workbench::Workbench;

impl Workbench {
    /// The action row's button: what the file is coloured as now.
    pub(crate) fn colouring_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.open.as_ref()?;
        if !open.notebook.can_recolour() {
            return None;
        }
        let p = *Palette::get(cx);
        let current = open.notebook.language.as_deref().unwrap_or("text");
        let label = COLOURINGS
            .iter()
            .find(|(key, _)| *key == current)
            .map_or("Plain text", |(_, label)| label);
        let button = bar_button("text-colouring", label, None, &p)
            .child(icon("chevron.down").size(px(12.)))
            .when(self.colouring_menu, |d| d.text_color(p.text))
            .on_prepaint(self.popup_triggers.track("text-colouring"))
            .tooltip(|w, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Colour this file as").build(w, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.colouring_menu = !this.colouring_menu;
                cx.notify();
            }));
        Some(
            div()
                .relative()
                .flex_none()
                .child(button)
                .when(self.colouring_menu, |d| {
                    d.child(
                        div().absolute().top(px(25.)).left_0().child(
                            deferred(
                                anchored()
                                    .snap_to_window_with_margin(px(8.))
                                    .child(self.colouring_menu(cx)),
                            )
                            .with_priority(3),
                        ),
                    )
                })
                .into_any_element(),
        )
    }

    /// The choices under the button: Automatic (the file type's own), then
    /// every colouring, a check by the one chosen.
    fn colouring_menu(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *Palette::get(cx);
        let path = self.open.as_ref().map(|open| open.notebook.path.clone());
        let chosen = path.and_then(|path| crate::prefs::text_colouring(&self.host(), &path));
        let mut rows = vec![crate::panes::tree_actions::menu_item(
            "colouring-auto",
            chosen.is_none().then_some("check"),
            "Automatic",
            "",
            &p,
            cx,
            |this, window, cx| this.set_text_colouring(None, window, cx),
        )];
        rows.push(crate::panes::tree_actions::separator(&p));
        for (key, label) in COLOURINGS {
            rows.push(crate::panes::tree_actions::menu_item(
                SharedString::from(format!("colouring-{key}")),
                (chosen.as_deref() == Some(key)).then_some("check"),
                label,
                "",
                &p,
                cx,
                move |this, window, cx| this.set_text_colouring(Some(key), window, cx),
            ));
        }
        crate::agents::composer_view::popup(&p)
            .id("colouring-menu")
            .occlude()
            .min_w(px(220.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this
                    .popup_triggers
                    .contains("text-colouring", event.position)
                {
                    this.colouring_menu = false;
                    cx.notify();
                }
            }))
    }

    /// Colours the open text file as `choice` (`None`: its type's own) and
    /// remembers it. Unsaved edits stay; the editor is built again.
    pub(crate) fn set_text_colouring(
        &mut self,
        choice: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.colouring_menu = false;
        let Some(open) = self
            .open
            .as_ref()
            .filter(|open| open.notebook.can_recolour())
        else {
            return;
        };
        let path = open.notebook.path.clone();
        let text = open.editors[0].read(cx).value().to_string();
        let (dirty, mtime_ms, truncated) = (open.dirty, open.mtime_ms, open.notebook.truncated);
        crate::prefs::save_text_colouring(&self.host(), &path, choice);
        let Ok(mut nb) = crate::editor::notebook::parse(&path, &text) else {
            return;
        };
        nb.truncated = truncated;
        self.show_notebook(nb, mtime_ms, window, cx);
        if let Some(open) = self.open.as_mut() {
            open.dirty = dirty;
        }
        cx.notify();
    }
}
