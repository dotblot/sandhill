//! The kernel picker in a notebook's bar (and the Console's when no
//! document is open): one button naming the kernel, which opens a list of
//! the R kernels and the Python environments (conda and mamba ones
//! included), grouped by language.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::KernelspecInfo;

use crate::look::{self, Palette, icon, picture};
use crate::workbench::Workbench;

/// The groups the list shows, in order, by language.
const GROUPS: [(&str, &str); 2] = [("r", "R"), ("python", "Python")];

/// The kernels the list offers, grouped: R, then Python, then any other
/// language, each in the daemon's order. `only` keeps one language (a
/// script runs in its own).
pub(crate) fn grouped<'a>(
    kernels: &'a [KernelspecInfo],
    only: Option<&str>,
) -> Vec<(String, Vec<(usize, &'a KernelspecInfo)>)> {
    let mut groups: Vec<(String, Vec<(usize, &KernelspecInfo)>)> = Vec::new();
    for (ix, spec) in kernels.iter().enumerate() {
        if only.is_some_and(|l| !spec.language.eq_ignore_ascii_case(l)) {
            continue;
        }
        let language = spec.language.to_ascii_lowercase();
        let title = GROUPS
            .iter()
            .find(|(l, _)| *l == language)
            .map_or(spec.language.clone(), |(_, t)| t.to_string());
        match groups.iter_mut().find(|(t, _)| *t == title) {
            Some((_, list)) => list.push((ix, spec)),
            None => groups.push((title, vec![(ix, spec)])),
        }
    }
    let rank = |title: &str| {
        GROUPS
            .iter()
            .position(|(_, t)| *t == title)
            .unwrap_or(GROUPS.len())
    };
    groups.sort_by_key(|(t, _)| rank(t));
    groups
}

impl Workbench {
    /// The button naming the kernel, with its list under it when open.
    pub(crate) fn kernel_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let chosen = self
            .active_kernel()
            .and_then(|k| self.kernels.iter().find(|s| s.name == k));
        let hover = p.ink(8.);
        let open = self.kernel_menu;
        let button = h_flex()
            .id("kernel-picker")
            .flex_none()
            .h(px(22.))
            .gap(px(5.))
            .pl(px(4.))
            .pr(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(if chosen.is_some() { p.accent } else { p.line })
            .when(chosen.is_some(), |d| d.bg(p.accent_soft))
            .text_size(px(look::TAB_TEXT))
            .text_color(if chosen.is_some() { p.text } else { p.muted })
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .map(
                |d| match chosen.and_then(|s| look::language_icon(&s.language)) {
                    Some(lang) => d.child(picture(lang).size(px(14.))),
                    None => d.pl(px(8.)),
                },
            )
            .child(chosen.map_or("Choose a kernel".to_string(), |s| s.display_name.clone()))
            .child(icon("chevron.down").size(px(12.)).text_color(p.faint))
            .on_click(cx.listener(|this, _, _, cx| {
                this.kernel_menu = !this.kernel_menu;
                cx.notify();
            }));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when(open, |d| {
                d.child(
                    div().absolute().left_0().top(px(24.)).child(
                        deferred(
                            anchored()
                                .snap_to_window_with_margin(px(8.))
                                .child(self.kernel_list(cx)),
                        )
                        .with_priority(3),
                    ),
                )
            })
            .into_any_element()
    }

    /// The list of kernels, by language; the chosen one is ticked.
    fn kernel_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let chosen = self.active_kernel().map(str::to_string);
        // A script offers only kernels for its language.
        let only = self
            .open
            .as_ref()
            .filter(|o| o.is_script())
            .and_then(|o| o.notebook.language.clone());
        let mut rows: Vec<AnyElement> = Vec::new();
        for (title, specs) in grouped(&self.kernels, only.as_deref()) {
            rows.push(
                div()
                    .px(px(8.))
                    .pt(px(if rows.is_empty() { 2. } else { 8. }))
                    .pb(px(2.))
                    .text_color(p.faint)
                    .child(title)
                    .into_any_element(),
            );
            for (ix, spec) in specs {
                let name = spec.name.clone();
                let picked = chosen.as_deref() == Some(&spec.name);
                let hover = p.ink(7.);
                rows.push(
                    h_flex()
                        .id(("kernel-row", ix))
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(
                            div()
                                .w(px(14.))
                                .flex_none()
                                .when(picked, |d| d.child(icon("check").size(px(13.)))),
                        )
                        .map(|d| match look::language_icon(&spec.language) {
                            Some(lang) => d.child(picture(lang).size(px(14.))),
                            None => d,
                        })
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(spec.display_name.clone())
                                .when_some(spec.env.clone(), |d, env| {
                                    d.child(
                                        div()
                                            .text_color(p.faint)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(env),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.kernel_menu = false;
                            this.choose_kernel(name.clone(), cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(p.faint)
                    .child("No kernels found")
                    .into_any_element(),
            );
        }
        crate::composer_view::popup(&p)
            .id("kernel-menu")
            .occlude()
            .min_w(px(240.))
            .max_w(px(420.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.kernel_menu = false;
                cx.notify();
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::grouped;
    use sandhill_proto::KernelspecInfo;

    fn spec(name: &str, language: &str) -> KernelspecInfo {
        KernelspecInfo {
            name: name.into(),
            display_name: name.into(),
            language: language.into(),
            env: None,
        }
    }

    #[test]
    fn lists_r_kernels_then_python_environments() {
        let kernels = [
            spec("python3", "python"),
            spec("ark", "R"),
            spec("conda-env-omicverse-py", "python"),
            spec("ir", "R"),
            spec("julia", "julia"),
        ];
        let names = |only: Option<&str>| -> Vec<(String, Vec<String>)> {
            grouped(&kernels, only)
                .into_iter()
                .map(|(t, specs)| (t, specs.iter().map(|(_, s)| s.name.clone()).collect()))
                .collect()
        };
        assert_eq!(
            names(None),
            vec![
                ("R".to_string(), vec!["ark".to_string(), "ir".to_string()]),
                (
                    "Python".to_string(),
                    vec!["python3".to_string(), "conda-env-omicverse-py".to_string()]
                ),
                ("julia".to_string(), vec!["julia".to_string()]),
            ]
        );
        assert_eq!(names(Some("r")).len(), 1);
    }
}
